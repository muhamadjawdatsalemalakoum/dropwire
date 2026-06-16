//! Endpoint construction — the one place that wires up relay + discovery.
//!
//! All iroh-1.0 naming is current as of the pin: `EndpointId` (was `NodeId`),
//! `EndpointAddr` (was `NodeAddr`), and `address_lookup` (was `discovery`).

use anyhow::Context;
use iroh::{Endpoint, SecretKey};

use crate::config::Infra;
use crate::error::Result;
use crate::store::BLOBS_ALPN;

/// Build and bind the single long-lived endpoint for the given infra.
pub async fn build(secret_key: SecretKey, infra: &Infra) -> Result<Endpoint> {
    use iroh::endpoint::presets;

    let alpns = vec![BLOBS_ALPN.to_vec()];

    let endpoint = match infra {
        // DEFAULT: Mainline-DHT (pkarr) discovery + n0's free relay fallback.
        // No server we run; works across the internet; depends on n0 only for the
        // minority of transfers that can't go direct.
        Infra::Decentralized => {
            use iroh::endpoint::RelayMode;
            use iroh_mainline_address_lookup::DhtAddressLookup;
            // Publishes + resolves our address via the public BitTorrent DHT. Must be
            // built inside a Tokio runtime (this fn is async). By default it publishes
            // only relay addresses; the endpoint must stay online to republish.
            let dht = DhtAddressLookup::builder()
                .build()
                .context("build DHT address lookup")?;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Default) // n0's free public relays (fallback only)
                .address_lookup(dht) // find peers via the public DHT (no server we run)
                .bind()
                .await
                .context("bind endpoint (decentralized: DHT + n0 relay)")?
        }

        Infra::N0Default => Endpoint::builder(presets::N0)
            .secret_key(secret_key)
            .alpns(alpns)
            .bind()
            .await
            .context("bind endpoint (n0 default)")?,

        Infra::LocalOnly => {
            use iroh::endpoint::RelayMode;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .context("bind endpoint (local only)")?
        }

        Infra::SelfHosted {
            relay_url,
            relay_token,
            pkarr_relay,
            origin_domain,
        } => {
            use iroh::address_lookup::{dns::DnsAddressLookup, pkarr::PkarrPublisher};
            use iroh::endpoint::RelayMode;
            use iroh::{RelayConfig, RelayMap, RelayUrl};

            let relay: RelayUrl = relay_url.parse().context("parse relay url")?;
            // VERIFY (ARCHITECTURE.md §13): RelayConfig::from(RelayUrl) + with_auth_token,
            // and RelayMap::from_iter — confirm constructors against docs.rs/iroh/1.0.0.
            let relay_cfg = RelayConfig::from(relay).with_auth_token(relay_token.clone());
            let relay_map = RelayMap::from_iter([relay_cfg]);

            let pkarr: url::Url = pkarr_relay.parse().context("parse pkarr relay url")?;

            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Custom(relay_map))
                .address_lookup(PkarrPublisher::builder(pkarr))
                .address_lookup(DnsAddressLookup::builder(origin_domain.clone()))
                .bind()
                .await
                .context("bind endpoint (self-hosted)")?
        }

        // TEST-ONLY: relay-only against an in-process relay. We add the custom
        // relay transport, trust its self-signed test certificate, and strip every
        // direct IP transport — so the only way to the peer is through the relay.
        // The ticket carries the relay address (no separate discovery needed).
        #[cfg(feature = "test-utils")]
        Infra::LocalRelay { relay_map } => {
            use iroh::endpoint::RelayMode;
            use iroh::tls::CaTlsConfig;
            Endpoint::builder(presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .relay_mode(RelayMode::Custom(relay_map.clone()))
                .ca_tls_config(CaTlsConfig::insecure_skip_verify())
                .clear_ip_transports()
                .bind()
                .await
                .context("bind endpoint (local relay, test-only)")?
        }
    };

    // NB: we intentionally do NOT block on `endpoint.online()` here, so app
    // startup is instant. The relay handshake is awaited (time-boxed) only when
    // it matters — right before minting a ticket in `send` — so the ticket
    // carries a reachable relay address. See `send::run_send`.
    Ok(endpoint)
}
