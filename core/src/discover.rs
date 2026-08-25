//! Nearby-device discovery over mDNS/DNS-SD ("local network" mode).
//!
//! Each running Dropwire advertises itself as `_dropwire._udp.local.` while a
//! nearby session is open, carrying its endpoint identity in DNS TXT records.
//! Browsing instances collect live peers into [`NearbyDevice`] snapshots that
//! the UI renders as tappable devices. Discovery alone grants nothing: a
//! transfer still requires both sides' explicit consent (see [`crate::offer`]).
//!
//! Bluetooth is the planned fallback transport for the same payload shape
//! (advertise name + fingerprint, bootstrap out-of-band); the [`NearbyDevice`]
//! vocabulary is deliberately transport-neutral so that phase slots in without
//! touching callers.
//!
//! ## One daemon, one browse, many tables
//!
//! mdns-sd keeps only ONE listener per service type per daemon
//! (`service_queriers.insert` overwrites), so a second concurrent browser —
//! another `Core` in tests, or a second app instance on the same machine —
//! would silently cut the first one's event stream off. This module therefore
//! runs ONE daemon, ONE browse, and ONE pump thread that fans every event out
//! to all registered peer tables.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use iroh::EndpointId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// DNS-SD service type Dropwire advertises/browses on the LAN.
pub(crate) const NEARBY_SERVICE: &str = "_dropwire._udp.local.";

/// TXT keys (short: mDNS TXT records should stay small).
const TXT_EID: &str = "dw_eid";
const TXT_NAME: &str = "dw_name";

/// One nearby Dropwire instance seen on the local network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyDevice {
    /// Hex endpoint id — stable identity of the other device.
    pub endpoint_id: String,
    /// Human-readable device name (sender-authored claim; show next to a
    /// fingerprint so humans can eyeball-match).
    pub name: String,
    /// Short human-checkable fingerprint derived from the endpoint id
    /// (e.g. `"k7q m2z h4w"`) — displayed in confirm dialogs on both sides.
    pub fingerprint: String,
    /// Most recent LAN socket, e.g. `"192.168.1.20:48726"` (display/debug).
    pub addr: Option<String>,
    /// Unix seconds of last sighting.
    pub seen_at: u64,
}

/// How many base32 chars the human fingerprint carries. 12 chars = 60 bits.
/// The old design emitted 9 chars straight off the *hex string* of the id,
/// which left only ~21 effective bits (a hex char carries 4 bits and its ASCII
/// high nibble is fixed), so an attacker could grind an Ed25519 keypair to a
/// matching fingerprint in minutes and defeat the human compare step. Hashing
/// the identity first spreads every key bit across the output; 60 bits makes
/// grinding a collision (≈2^60 keygens) infeasible.
const FP_CHARS: usize = 12;

impl NearbyDevice {
    /// Build the short human fingerprint: [`FP_CHARS`] base32 chars derived from
    /// a BLAKE3 hash of the identity, grouped in threes (`abc def ghi jkl`). The
    /// hash is what makes every bit of the key matter; the same algorithm runs
    /// on both sides, so two humans can compare the groups aloud before accepting.
    pub fn fingerprint_for(endpoint_id: &str) -> String {
        // Hash first: BLAKE3 over the identity so the fingerprint depends on the
        // whole key, not a grindable prefix of its hex form.
        let digest = iroh_blobs::Hash::new(endpoint_id.as_bytes());
        let mut chars: Vec<char> = Vec::with_capacity(FP_CHARS);
        let mut acc: u32 = 0;
        let mut bits = 0u32;
        for b in digest.as_bytes() {
            acc = (acc << 8) | *b as u32;
            bits += 8;
            while bits >= 5 && chars.len() < FP_CHARS {
                bits -= 5;
                let idx = ((acc >> bits) & 0x1f) as usize;
                chars.push(BASE32.as_bytes()[idx] as char);
            }
            if chars.len() == FP_CHARS {
                break;
            }
        }
        chars
            .chunks(3)
            .map(|c| c.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

const BASE32: &str = "abcdefghijklmnopqrstuvwxyz234567";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Internal peer entry: the public snapshot plus the parsed LAN socket we can
/// actually dial (mDNS SRV target + port), so consent answers prefer the LAN.
#[derive(Debug, Clone)]
pub(crate) struct PeerEntry {
    pub(crate) device: NearbyDevice,
    pub(crate) sock: Option<SocketAddr>,
    /// Full mDNS instance name we resolved this peer from, so a `ServiceRemoved`
    /// event can be matched to the exact peer that left (never a prefix of some
    /// other peer's id — see the removal handler).
    pub(crate) instance: String,
}

/// Peer tables subscribed to mDNS events (each `NearbyState.peers`).
type PeerTable = Arc<StdMutex<HashMap<String, PeerEntry>>>;

/// All live peer tables receiving fanned-out events.
static SUBSCRIBERS: OnceLock<StdMutex<Vec<PeerTable>>> = OnceLock::new();

/// The single shared mDNS daemon (the crate recommends one per process).
static DAEMON: OnceLock<ServiceDaemon> = OnceLock::new();

fn daemon() -> Result<&'static ServiceDaemon> {
    if let Some(d) = DAEMON.get() {
        return Ok(d);
    }
    let d =
        ServiceDaemon::new().map_err(|e| CoreError::Other(anyhow::anyhow!("mDNS daemon: {e}")))?;
    let _ = DAEMON.set(d);
    Ok(DAEMON.get().expect("just set"))
}

/// Ensure the shared browse + pump thread is running (idempotent).
fn ensure_browse() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.get().is_some() {
        return;
    }
    let Ok(d) = daemon() else { return };
    let Ok(receiver) = d.browse(NEARBY_SERVICE) else {
        // Browse unavailable (e.g. no multicast stack): degrade quietly —
        // advertising still works for others to see us.
        return;
    };
    if STARTED.set(()).is_err() {
        return; // someone else won the race
    }
    std::thread::Builder::new()
        .name("dropwire-mdns".into())
        .spawn(move || loop {
            // Block until the next event (flume recv blocks).
            let event = match receiver.recv() {
                Ok(ev) => ev,
                Err(_) => return, // channel closed: daemon gone
            };
            let Some(subscribers) = SUBSCRIBERS.get() else {
                continue;
            };
            let tables = subscribers
                .lock()
                .unwrap_or_else(poisoned)
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            for table in tables {
                // Never let a malformed packet abort the process (panic = abort):
                // fold each event under catch_unwind so a parse panic in this or
                // a dependency only skips that one event, not the whole app.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    apply_event(&table, &event);
                }));
            }
        })
        .ok();
}

/// Fold one mDNS event into one peer table.
fn apply_event(peers: &PeerTable, event: &ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let props = info.get_properties();
            let Some(other_eid) = props.get_property_val_str(TXT_EID).map(str::to_owned) else {
                return; // not a Dropwire v1 announcement
            };
            let name = props
                .get_property_val_str(TXT_NAME)
                .map(str::to_owned)
                .unwrap_or_else(|| "Dropwire device".into());
            let sock = info
                .get_addresses_v4()
                .into_iter()
                .next()
                .map(|ip| SocketAddr::new(IpAddr::V4(ip), info.get_port()));
            let entry = PeerEntry {
                device: NearbyDevice {
                    fingerprint: NearbyDevice::fingerprint_for(&other_eid),
                    addr: sock.as_ref().map(|s| s.to_string()),
                    endpoint_id: other_eid.clone(),
                    name,
                    seen_at: now_secs(),
                },
                sock,
                instance: info.get_fullname().to_string(),
            };
            peers
                .lock()
                .unwrap_or_else(poisoned)
                .insert(other_eid, entry);
        }
        ServiceEvent::ServiceRemoved(_ty, full_name) => {
            // Remove the ONE peer resolved from this exact instance name. The
            // earlier version matched any stored id that *started with* the
            // instance's 8-hex suffix, so a crafted or colliding instance name
            // could evict a different peer (a LAN denial-of-visibility). Match
            // the full instance string instead: unambiguous, unspoofable-against
            // a third party.
            let mut guard = peers.lock().unwrap_or_else(poisoned);
            let gone: Vec<String> = guard
                .iter()
                .filter(|(_, e)| &e.instance == full_name)
                .map(|(k, _)| k.clone())
                .collect();
            for k in gone {
                guard.remove(&k);
            }
        }
        _ => {}
    }
}

/// Recover a mutex guard even if a previous holder panicked. A poisoned lock
/// must never cascade into a second panic (fatal under `panic = "abort"`); the
/// peer table is plain data, so reading through the poison is safe.
fn poisoned<T>(e: std::sync::PoisonError<T>) -> T {
    e.into_inner()
}

/// Live state of the nearby session: what we advertise + who we can see.
#[derive(Debug)]
pub(crate) struct NearbyState {
    /// Full service instance name we registered (for unregistering).
    registered: Option<String>,
    /// Shared "discovery mode is ON" flag (gates offer-visibility checks).
    pub(crate) running: Arc<std::sync::atomic::AtomicBool>,
    /// This device's own hex endpoint id (to skip self-announcements).
    self_eid: String,
    /// Display name advertised to others.
    pub(crate) device_name: String,
    /// Live peers: hex endpoint id → entry.
    pub(crate) peers: PeerTable,
    /// This state's subscription slot in [`SUBSCRIBERS`] (dropped ⇒ removed).
    _slot: Option<SubscriptionSlot>,
}

/// Keeps this state's peer table subscribed while alive.
struct SubscriptionSlot {
    table: PeerTable,
    #[allow(dead_code)] // kept for debuggability of subscriber sets
    self_eid: String,
}

impl std::fmt::Debug for SubscriptionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionSlot")
            .field("self_eid", &self.self_eid)
            .finish_non_exhaustive()
    }
}

impl Drop for SubscriptionSlot {
    fn drop(&mut self) {
        if let Some(subscribers) = SUBSCRIBERS.get() {
            subscribers
                .lock()
                .unwrap_or_else(poisoned)
                .retain(|t| !Arc::ptr_eq(t, &self.table));
        }
    }
}

impl NearbyState {
    /// Whether an advertisement is currently live.
    pub(crate) fn is_running(&self) -> bool {
        self.registered.is_some()
    }

    /// The shared flag consumers watch (consent visibility gating).
    pub(crate) fn running_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.running.clone()
    }

    pub(crate) fn new(self_eid: String, device_name: String) -> Self {
        Self {
            registered: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            self_eid: self_eid.clone(),
            device_name,
            peers: Arc::new(StdMutex::new(HashMap::new())),
            _slot: None,
        }
    }

    /// Advertise this endpoint on the LAN and subscribe to peer events.
    pub(crate) fn start(&mut self, port: u16) -> Result<()> {
        use std::sync::atomic::Ordering;
        let d = daemon()?;
        let eid = self.self_eid.clone();
        let short = &eid[..eid.len().min(8)];
        let instance = format!("{}-{short}", sanitize_instance(&self.device_name));
        let host = format!("{short}.dropwire.local.");

        let mut props = HashMap::new();
        props.insert(TXT_EID.to_string(), eid.clone());
        props.insert(TXT_NAME.to_string(), self.device_name.clone());

        // No explicit IP: `addr_auto` announces on every interface and fills
        // the SRV target addresses itself.
        let info = ServiceInfo::new(
            NEARBY_SERVICE,
            instance.as_str(),
            host.as_str(),
            (),
            port,
            Some(props),
        )
        .map_err(|e| CoreError::Other(anyhow::anyhow!("mDNS service info: {e}")))?
        .enable_addr_auto();

        d.register(info)
            .map_err(|e| CoreError::Other(anyhow::anyhow!("mDNS register: {e}")))?;

        self.running.store(true, Ordering::Relaxed);
        self.registered = Some(format!("{instance}.{NEARBY_SERVICE}"));

        // Subscribe this state's peer table to the shared browse fan-out.
        // Self-announcements are filtered at read time (list/peer_socket),
        // so the same table can be shared without a per-event filter task.
        ensure_browse();
        let subscribers = SUBSCRIBERS.get_or_init(|| StdMutex::new(Vec::new()));
        let wrapped: PeerTable = Arc::new(StdMutex::new(HashMap::new()));
        subscribers
            .lock()
            .unwrap_or_else(poisoned)
            .push(wrapped.clone());
        let self_eid = self.self_eid.clone();
        self.peers = wrapped;
        self._slot = Some(SubscriptionSlot {
            table: self.peers.clone(),
            self_eid,
        });

        Ok(())
    }

    /// Stop advertising + forget the peer list (peers see us leave via TTL /
    /// their own browse Remove events).
    pub(crate) fn stop(&mut self) {
        use std::sync::atomic::Ordering;
        self.running.store(false, Ordering::Relaxed);
        if let Some(inst) = self.registered.take() {
            if let Ok(d) = daemon() {
                if let Ok(rx) = d.unregister(&inst) {
                    let _ = rx.recv_timeout(Duration::from_secs(2));
                }
            }
        }
        self._slot = None; // unsubscribe
        self.peers.lock().unwrap_or_else(poisoned).clear();
    }

    /// Snapshot of live peers, by display name. Self-announcements are filtered
    /// here (see `start`). Presence is driven by mDNS add/remove events, not a
    /// client-side age-out: mdns-sd refreshes a live peer's records before they
    /// expire and only emits `ServiceResolved` on genuine changes, so a stable
    /// peer would never refresh its `seen_at` and a time-based cutoff used to
    /// drop it ~30s after discovery while it was still present. A departed peer
    /// is removed on the `ServiceRemoved` event (goodbye packet or cache expiry).
    pub(crate) fn list(&self) -> Vec<NearbyDevice> {
        let mut devs: Vec<NearbyDevice> = self
            .peers
            .lock()
            .unwrap_or_else(poisoned)
            .values()
            .filter(|e| e.device.endpoint_id != self.self_eid)
            .map(|e| e.device.clone())
            .collect();
        devs.sort_by_key(|d| d.name.to_lowercase());
        devs
    }

    /// Look up one peer's LAN socket address by hex endpoint id.
    pub(crate) fn peer_socket(&self, eid_hex: &str) -> Option<SocketAddr> {
        if eid_hex == self.self_eid {
            return None;
        }
        self.peers
            .lock()
            .unwrap_or_else(poisoned)
            .get(eid_hex)
            .and_then(|e| e.sock)
    }
}

impl Drop for NearbyState {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Keep instance names friendly and DNS-label-safe (letters/digits/dash/dot).
fn sanitize_instance(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.').trim_matches('-');
    if trimmed.is_empty() {
        "device".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// Derive a default device name from the OS hostname.
pub(crate) fn default_device_name() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "My device".to_string())
}

/// Parse a hex [`EndpointId`] (as carried in `NearbyDevice.endpoint_id`).
pub(crate) fn parse_eid(hex: &str) -> Result<EndpointId> {
    use std::str::FromStr;
    EndpointId::from_str(hex).map_err(|_| CoreError::InvalidTicket(hex.to_string()))
}
