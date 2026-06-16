//! Engine configuration.

use std::path::PathBuf;

/// How the engine is configured at startup.
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Application data directory. Holds the node identity (`node.key`), the
    /// blob store (`blobs/`), and the transfer catalog (`transfers.json`).
    pub data_dir: PathBuf,
    /// Where relay + discovery come from.
    pub infra: Infra,
}

/// Relay + discovery infrastructure selection.
///
/// Dropwire's product decision (2026-06-16): **completely free, zero servers the
/// builder runs, one-to-one transfers only.** The default is [`Infra::Decentralized`]:
/// peers find each other over the public Mainline DHT (no server), and fall back
/// to n0's *free* public relay only when a direct connection can't be made.
#[derive(Debug, Clone)]
pub enum Infra {
    /// **Default.** Discovery via the public Mainline BitTorrent DHT (pkarr) +
    /// n0's free public relays as connection fallback. Costs the builder nothing,
    /// requires no servers, works across the internet, and depends on n0 only for
    /// the minority of transfers that can't go direct.
    Decentralized,

    /// n0's public relays **and** n0's DNS discovery. The simplest free path;
    /// leans on n0 for discovery too. Handy for development.
    N0Default,

    /// No relay, no discovery — direct connections only, using the addresses
    /// embedded in the ticket. Used for LAN-only mode and hermetic tests.
    LocalOnly,

    /// **Optional / advanced.** Self-hosted relay + DNS discovery, for users or
    /// orgs who want full control (see `infra/`). Not the default — Dropwire ships
    /// serverless. The relay is locked to the app via a shared token (app-level
    /// access control, not a user account).
    SelfHosted {
        /// e.g. `"https://relay.example.org/"`
        relay_url: String,
        /// Shared secret embedded in the app build.
        relay_token: String,
        /// pkarr publish endpoint, e.g. `"https://dns.example.org/pkarr"`.
        pkarr_relay: String,
        /// DNS origin domain, must match the dns-server `[dns].origins`.
        origin_domain: String,
    },

    /// **Test-only.** Relay-only transport against an *in-process* relay, with all
    /// direct IP paths removed. This lets single-machine integration tests
    /// exercise the real production path — discovery via the ticket's relay
    /// address and bytes actually relayed through a relay server — without a
    /// network. Gated behind the `test-utils` feature; never compiled into
    /// shipped builds. See `core/tests/relay.rs`.
    #[cfg(feature = "test-utils")]
    LocalRelay {
        /// The relay map for the in-process relay, as returned by
        /// `iroh::test_utils::run_relay_server`.
        relay_map: iroh::RelayMap,
    },
}

impl CoreConfig {
    /// The recommended default: serverless — DHT discovery + n0 free relay fallback.
    pub fn serverless(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::Decentralized,
        }
    }

    /// Development configuration on n0's public infra (relays + DNS discovery).
    pub fn dev(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::N0Default,
        }
    }

    /// Local-only configuration (no relay/discovery) — used by tests and LAN mode.
    pub fn local_only(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            infra: Infra::LocalOnly,
        }
    }
}
