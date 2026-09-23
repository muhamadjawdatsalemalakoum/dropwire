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
//! runs ONE daemon, at most ONE browse, and one pump thread per browse that
//! fans every event out to all subscribed peer tables.
//!
//! The browse runs only while some session in the process has sharing on.
//! When the last one turns it off, the browse stops (no more queries on the
//! network, so "hidden" holds for browsing too) and the tables are emptied.
//! Turning sharing on again starts a fresh browse, which asks the network
//! anew, so the devices still around show again at once.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use iroh::EndpointId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo, UnregisterStatus};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// DNS-SD service type Dropwire advertises/browses on the LAN.
pub(crate) const NEARBY_SERVICE: &str = "_dropwire._udp.local.";

/// TXT keys (short: mDNS TXT records should stay small).
const TXT_EID: &str = "dw_eid";
const TXT_NAME: &str = "dw_name";
/// Platform of the advertising device ("windows" / "macos" / "linux").
/// Shown next to the hostname: two similar names are told apart fastest
/// by the platform, and it is honest data rather than a guess from the name.
const TXT_OS: &str = "dw_os";

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
    /// Advertised platform, e.g. `"windows"`. `None` from peers older than the
    /// TXT key, so the UI simply omits the badge.
    #[serde(default)]
    pub os: Option<String>,
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

/// Peer tables fed by the browse (each `NearbyState.peers`).
pub(crate) type PeerTable = Arc<StdMutex<HashMap<String, PeerEntry>>>;

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

/// Discovery shared by every nearby session in the process (one in the app,
/// a few in tests): the peer tables the browse feeds, and the browse itself.
struct Discovery {
    /// Every session's peer table, from its first start until it is dropped.
    tables: Vec<PeerTable>,
    /// Sessions with sharing on. The browse runs only while there are some.
    sharing: usize,
    /// The running browse, by number. Its pump applies events only while it
    /// is still the current one, so nothing it had queued lands after a stop.
    browse: Option<u64>,
    /// Number for the next browse.
    next_browse: u64,
}

static DISCOVERY: StdMutex<Discovery> = StdMutex::new(Discovery {
    tables: Vec::new(),
    sharing: 0,
    browse: None,
    next_browse: 0,
});

/// The process-wide discovery state. Lock order: this, then a peer table.
fn discovery() -> std::sync::MutexGuard<'static, Discovery> {
    DISCOVERY.lock().unwrap_or_else(poisoned)
}

impl Discovery {
    /// A session turned sharing on: follow the network into its table, and
    /// make sure the browse runs.
    fn join(&mut self, table: &PeerTable) {
        if !self.tables.iter().any(|t| Arc::ptr_eq(t, table)) {
            self.tables.push(table.clone());
        }
        self.sharing += 1;
        if self.browse.is_none() {
            self.start_browse();
        }
    }

    /// A session turned sharing off. With none left sharing, the browse
    /// stops, so a device that shows as hidden sends no Dropwire queries
    /// either, and what it saw is forgotten: nothing keeps it current now.
    /// The next browse asks the network afresh (the daemon drops its cache
    /// for the service on stop), and every device still around answers.
    fn leave(&mut self) {
        self.sharing = self.sharing.saturating_sub(1);
        if self.sharing > 0 {
            return;
        }
        if self.browse.take().is_some() {
            if let Ok(d) = daemon() {
                if let Err(e) = d.stop_browse(NEARBY_SERVICE) {
                    tracing::warn!("mDNS stop browse failed: {e}");
                }
            }
        }
        for table in &self.tables {
            table.lock().unwrap_or_else(poisoned).clear();
        }
    }

    /// Start the one browse and its pump thread. A browse that cannot start
    /// (no multicast, say) degrades quietly: advertising still lets others
    /// see us, and the next start tries again.
    fn start_browse(&mut self) {
        let Ok(d) = daemon() else { return };
        let receiver = match d.browse(NEARBY_SERVICE) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("mDNS browse unavailable: {e}");
                return;
            }
        };
        let id = self.next_browse;
        self.next_browse = self.next_browse.wrapping_add(1);
        // The pump reads `browse` under the discovery lock, which the caller
        // holds until `browse` is set below.
        let spawned = std::thread::Builder::new()
            .name("dropwire-mdns".into())
            .spawn(move || pump(receiver, id));
        match spawned {
            Ok(_) => self.browse = Some(id),
            Err(e) => {
                tracing::warn!("mDNS pump thread: {e}");
                let _ = d.stop_browse(NEARBY_SERVICE);
            }
        }
    }
}

/// Feed browse `id`'s events to every subscribed table until it stops.
fn pump(receiver: mdns_sd::Receiver<ServiceEvent>, id: u64) {
    // Blocks until the next event; ends when the daemon lets the browse go.
    while let Ok(event) = receiver.recv() {
        if let ServiceEvent::SearchStopped(_) = event {
            return;
        }
        let disc = discovery();
        if disc.browse != Some(id) {
            return; // stopped, or replaced by a newer browse
        }
        for table in &disc.tables {
            // Never let a malformed packet take discovery down: a panic while
            // folding one untrusted event (here or in a dependency) only
            // skips that event.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply_event(table, &event);
            }));
        }
    }
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
            let os = props.get_property_val_str(TXT_OS).map(str::to_owned);
            let sock = info
                .get_addresses_v4()
                .into_iter()
                .next()
                .map(|ip| SocketAddr::new(IpAddr::V4(ip), info.get_port()));
            let entry = PeerEntry {
                device: NearbyDevice {
                    fingerprint: NearbyDevice::fingerprint_for(&other_eid),
                    os,
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
    /// Full service instance name we registered (for unregistering). Set
    /// exactly while this state counts as sharing in [`Discovery`].
    registered: Option<String>,
    /// Shared "discovery mode is ON" flag (gates offer-visibility checks).
    pub(crate) running: Arc<std::sync::atomic::AtomicBool>,
    /// This device's own hex endpoint id (to skip self-announcements).
    self_eid: String,
    /// Display name advertised to others.
    pub(crate) device_name: String,
    /// Live peers: hex endpoint id to entry. One table for the life of this
    /// state, so a handle taken once, like the consent gate's, stays current.
    /// The browse fills it while sharing is on (here or in another session
    /// of this process), and it is emptied when the browse stops.
    pub(crate) peers: PeerTable,
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

    /// A handle to the live peer table (consent visibility gating). It stays
    /// current while sharing is on and is never replaced. Another session in
    /// the same process can keep it filled while this one is off, so check
    /// the running flag first.
    pub(crate) fn peer_table(&self) -> PeerTable {
        self.peers.clone()
    }

    pub(crate) fn new(self_eid: String, device_name: String) -> Self {
        Self {
            registered: None,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            self_eid,
            device_name,
            peers: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Advertise this endpoint on the LAN and follow the devices around.
    pub(crate) fn start(&mut self, port: u16) -> Result<()> {
        use std::sync::atomic::Ordering;
        if self.is_running() {
            return Ok(());
        }
        let d = daemon()?;
        let info = advertisement(&self.self_eid, &self.device_name, port)?;
        // The daemon files the service under this exact name (with any
        // escaping it applied), and unregistering must use the same one.
        let fullname = info.get_fullname().to_string();

        d.register(info)
            .map_err(|e| CoreError::Other(anyhow::anyhow!("mDNS register: {e}")))?;

        self.registered = Some(fullname);
        // Self-announcements are filtered at read time (list/peer_socket),
        // so each session's table takes every event as it comes.
        discovery().join(&self.peers);
        self.running.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Rename this device. The name is checked first ([`device_name_from`]);
    /// one that is refused changes nothing. If a session is live the
    /// advertisement is re-registered under the new name so peers see the
    /// change without the user having to toggle sharing off and on. Only the
    /// advertisement changes: the browse, and the devices it shows, carry on.
    /// If the new advertisement cannot go out, the old one is put back.
    pub(crate) fn rename(&mut self, name: &str, port: u16) -> Result<()> {
        use std::sync::atomic::Ordering;
        let name = device_name_from(name)?;
        if self.device_name == name {
            return Ok(());
        }
        if self.is_running() {
            let d = daemon()?;
            let info = advertisement(&self.self_eid, &name, port)?;
            let fullname = info.get_fullname().to_string();
            // Registering the same instance again updates it in place. A new
            // instance means the old one says goodbye first.
            if let Some(old) = self.registered.take() {
                if !old.eq_ignore_ascii_case(&fullname) {
                    unregister(d, &old);
                }
            }
            if let Err(e) = d.register(info) {
                tracing::warn!("mDNS register for a rename: {e}");
                let restored = advertisement(&self.self_eid, &self.device_name, port)
                    .ok()
                    .and_then(|info| {
                        let old = info.get_fullname().to_string();
                        d.register(info).ok().map(|()| old)
                    });
                match restored {
                    Some(old) => self.registered = Some(old),
                    None => {
                        // Not even the old name could go back out: this
                        // device is no longer advertised, so say so.
                        self.running.store(false, Ordering::Relaxed);
                        discovery().leave();
                    }
                }
                return Err(CoreError::Other(anyhow::anyhow!(RENAME_FAILED)));
            }
            self.registered = Some(fullname);
        }
        self.device_name = name;
        Ok(())
    }

    /// Stop advertising (peers see us leave via our goodbye, or their TTL).
    /// The peer list is hidden while off, and with no session in the process
    /// sharing, the browse stops too.
    pub(crate) fn stop(&mut self) {
        use std::sync::atomic::Ordering;
        self.running.store(false, Ordering::Relaxed);
        let Some(inst) = self.registered.take() else {
            return;
        };
        if let Ok(d) = daemon() {
            unregister(d, &inst);
        }
        discovery().leave();
    }

    /// Snapshot of live peers, by display name. Self-announcements are filtered
    /// here (see `start`). Presence is driven by mDNS add/remove events, not a
    /// client-side age-out: mdns-sd refreshes a live peer's records before they
    /// expire and only emits `ServiceResolved` on genuine changes, so a stable
    /// peer would never refresh its `seen_at` and a time-based cutoff used to
    /// drop it ~30s after discovery while it was still present. A departed peer
    /// is removed on the `ServiceRemoved` event (goodbye packet or cache expiry).
    /// Empty while sharing is off.
    pub(crate) fn list(&self) -> Vec<NearbyDevice> {
        if !self.is_running() {
            return Vec::new();
        }
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

    /// Look up one peer's LAN socket address by hex endpoint id. None while
    /// sharing is off: a hidden device dials no one it found on the LAN.
    pub(crate) fn peer_socket(&self, eid_hex: &str) -> Option<SocketAddr> {
        if !self.is_running() || eid_hex == self.self_eid {
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
        discovery().tables.retain(|t| !Arc::ptr_eq(t, &self.peers));
    }
}

/// Whether the process is browsing for nearby devices right now.
#[cfg(feature = "test-utils")]
pub(crate) fn browsing() -> bool {
    discovery().browse.is_some()
}

/// The advertisement for device `eid` under the display name `name`. Built
/// without touching the network, so a name that cannot be advertised is
/// caught before the live advertisement changes.
fn advertisement(eid: &str, name: &str, port: u16) -> Result<ServiceInfo> {
    let short: String = eid.chars().take(8).collect();
    let instance = format!("{}-{short}", sanitize_instance(name));
    let host = format!("{short}.dropwire.local.");

    let mut props = HashMap::new();
    props.insert(TXT_EID.to_string(), eid.to_string());
    props.insert(TXT_NAME.to_string(), fit_txt(TXT_NAME, name).to_string());
    props.insert(TXT_OS.to_string(), std::env::consts::OS.to_string());

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
    Ok(info)
}

/// Withdraw the advertisement filed under `fullname`, sending the goodbye
/// that tells peers we left. A miss is logged: it means the advertisement
/// is still out there, answering for a device that says it is hidden.
fn unregister(d: &ServiceDaemon, fullname: &str) {
    match d.unregister(fullname) {
        Ok(rx) => match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(UnregisterStatus::OK) => {}
            Ok(UnregisterStatus::NotFound) => {
                tracing::warn!(instance = %fullname, "mDNS unregister: no such service, the advertisement may linger");
            }
            Err(e) => tracing::warn!(instance = %fullname, "mDNS unregister: no answer: {e}"),
        },
        Err(e) => tracing::warn!(instance = %fullname, "mDNS unregister failed: {e}"),
    }
}

/// Keep instance names friendly and DNS-label-safe: letters, digits and
/// dashes only. A dot would have to be escaped inside the label, which some
/// resolvers mishandle; the exact display name travels in the TXT record.
fn sanitize_instance(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "device".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// Longest name this device can take, in characters. The setup screen has
/// the same limit, and even in four-byte characters it fits the 255-byte
/// TXT entry with room to spare.
pub(crate) const MAX_NAME: usize = 40;

const NAME_EMPTY: &str = "A device name cannot be empty.";
const RENAME_FAILED: &str =
    "The new name could not be shown on this network, so the old one is kept.";

/// Check and tidy a name chosen for this device: cleaned as [`clean_label`]
/// does, and refused when that leaves nothing or more than [`MAX_NAME`]
/// characters. The errors are meant to be shown as they are.
pub(crate) fn device_name_from(raw: &str) -> Result<String> {
    let name = clean_label(raw, usize::MAX);
    if name.is_empty() {
        return Err(CoreError::Other(anyhow::anyhow!(NAME_EMPTY)));
    }
    if name.chars().count() > MAX_NAME {
        return Err(CoreError::Other(anyhow::anyhow!(
            "That name is too long. Keep it to {MAX_NAME} characters or fewer."
        )));
    }
    Ok(name)
}

/// Text a device or a person wrote, made safe to show: control characters
/// and invisible formatting marks dropped (the direction overrides among
/// them can make a name read backwards), runs of whitespace turned into one
/// space, trimmed, and cut to at most `max` characters.
pub(crate) fn clean_label(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut gap = false;
    for c in s.chars() {
        if c.is_whitespace() {
            gap = !out.is_empty();
            continue;
        }
        if c.is_control() || invisible(c) {
            continue;
        }
        let need = if gap { 2 } else { 1 };
        if count + need > max {
            break;
        }
        if gap {
            out.push(' ');
            count += 1;
            gap = false;
        }
        out.push(c);
        count += 1;
    }
    out
}

/// Formatting marks that show nothing themselves but change how the text
/// around them reads: direction marks, embeddings, overrides and isolates,
/// zero-width spaces and the like. (Joiners and variation selectors stay:
/// emoji and some scripts need them.)
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// `value` cut on a character boundary so `key=value` fits one TXT entry
/// (255 bytes). A checked device name always fits; this only guards the
/// advertisement against a name that was never checked.
fn fit_txt<'a>(key: &str, value: &'a str) -> &'a str {
    let room = 255usize.saturating_sub(key.len() + 1);
    if value.len() <= room {
        return value;
    }
    let mut end = room;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// Derive a default device name from the OS hostname, without the local
/// domain a Mac adds (`Keons-MacBook-Pro.local` becomes `Keons-MacBook-Pro`),
/// tidied and cut to [`MAX_NAME`] so it is always a name the device can take.
pub(crate) fn default_device_name() -> String {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = clean_label(without_local_domain(host.trim()), MAX_NAME);
    if name.is_empty() {
        "My device".to_string()
    } else {
        name
    }
}

/// `host` without a trailing local domain (`.local`, `.lan`, `.home`,
/// `.localdomain`, any case).
fn without_local_domain(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    let lower = host.to_ascii_lowercase();
    for suffix in [".localdomain", ".local", ".lan", ".home"] {
        if lower.ends_with(suffix) {
            // The suffix is ASCII and matched byte for byte (ASCII case
            // folding keeps every byte where it is), so this is a char
            // boundary.
            return &host[..host.len() - suffix.len()];
        }
    }
    host
}

/// Parse a hex [`EndpointId`] (as carried in `NearbyDevice.endpoint_id`).
pub(crate) fn parse_eid(hex: &str) -> Result<EndpointId> {
    use std::str::FromStr;
    EndpointId::from_str(hex).map_err(|_| CoreError::InvalidTicket(hex.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EID: &str = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12";

    #[test]
    fn instance_labels_need_no_escaping() {
        assert_eq!(
            sanitize_instance("Keons-MacBook-Pro.local"),
            "Keons-MacBook-Pro-local"
        );
        assert_eq!(sanitize_instance(r"a.b\c"), "a-b-c");
        assert_eq!(sanitize_instance("..."), "device");
        for name in ["Keons-MacBook-Pro.local", "Mom's phone", r"a.b\c", "千代"] {
            let label = sanitize_instance(name);
            assert!(!label.contains('.') && !label.contains('\\'), "{label}");
        }
    }

    /// The name kept for unregistering is the one the daemon files the
    /// service under, even for a display name that has dots in it.
    #[test]
    fn a_dotted_name_is_unregistered_by_the_name_it_was_filed_under() {
        let info = advertisement(EID, "Keons-MacBook-Pro.local", 4242).unwrap();
        assert_eq!(
            info.get_fullname(),
            "Keons-MacBook-Pro-local-ab12cd34._dropwire._udp.local."
        );
        // The display name itself travels untouched in the TXT record.
        assert_eq!(
            info.get_property_val_str(TXT_NAME),
            Some("Keons-MacBook-Pro.local")
        );
    }

    #[test]
    fn labels_lose_hidden_characters_and_extra_space() {
        assert_eq!(
            clean_label("  Keon\u{202E}'s\t\r\n laptop\u{7} \u{200B}", 64),
            "Keon's laptop"
        );
        assert_eq!(clean_label("\u{2067}evil\u{2069}", 64), "evil");
        assert_eq!(clean_label("a    b", 64), "a b");
        // Cut on characters, never leaving a space at the end.
        assert_eq!(clean_label("ab cd", 3), "ab");
        assert_eq!(clean_label("千代田区", 2), "千代");
        // Emoji joiners stay.
        assert_eq!(clean_label("👩\u{200D}💻", 64), "👩\u{200D}💻");
        assert_eq!(clean_label("\u{7}\n\u{202E}", 64), "");
    }

    #[test]
    fn device_names_are_checked() {
        assert_eq!(
            device_name_from("  Keon's\nlaptop ").unwrap(),
            "Keon's laptop"
        );
        assert!(device_name_from(" \u{202E}\t").is_err());
        assert!(device_name_from(&"x".repeat(MAX_NAME)).is_ok());
        let long = device_name_from(&"x".repeat(MAX_NAME + 1)).unwrap_err();
        assert!(long.to_string().contains("40 characters"), "{long}");
        // Forty of the widest characters still make a valid advertisement.
        let wide = device_name_from(&"𝄞".repeat(MAX_NAME)).unwrap();
        let info = advertisement(EID, &wide, 4242).unwrap();
        assert_eq!(info.get_property_val_str(TXT_NAME), Some(wide.as_str()));
    }

    #[test]
    fn an_unchecked_name_still_fits_the_txt_record() {
        let name = "千".repeat(100); // 300 bytes
        let fitted = fit_txt(TXT_NAME, &name);
        assert!(TXT_NAME.len() + 1 + fitted.len() <= 255);
        assert!(fitted.chars().all(|c| c == '千'));
        assert!(advertisement(EID, &name, 4242).is_ok());
        assert_eq!(fit_txt(TXT_NAME, "short"), "short");
    }

    #[test]
    fn default_names_drop_the_local_domain() {
        assert_eq!(
            without_local_domain("Keons-MacBook-Pro.local"),
            "Keons-MacBook-Pro"
        );
        assert_eq!(
            without_local_domain("Keons-MacBook-Pro.LOCAL."),
            "Keons-MacBook-Pro"
        );
        assert_eq!(without_local_domain("box.localdomain"), "box");
        assert_eq!(without_local_domain("pi.lan"), "pi");
        assert_eq!(without_local_domain("desktop-7"), "desktop-7");
        assert_eq!(without_local_domain(".local"), "");
        assert!(!default_device_name().is_empty());
    }
}
