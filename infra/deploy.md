# Deploying Dropwire infrastructure

This is the **human-gate checklist** — the steps that need *your* domain, servers, and money
(roughly two ~$5/mo VPSes + a domain). Everything is scripted up to here; follow these once.

> Outcome: a self-hosted relay at `relay.example.org` and discovery at `dns.example.org`, with
> the desktop app pointed at them via `CoreConfig::Infra::SelfHosted`.

## 0. Prerequisites

- A domain you control (e.g. `example.org`).
- Two small Linux VPSes with public IPv4 (flat/unmetered egress strongly preferred,
  since the relay forwards bytes for transfers that can't connect directly). Call them **RELAY_HOST** and **DNS_HOST**.
- Docker + Docker Compose on each.
- A relay token: `openssl rand -hex 32`.

## 1. DNS records (at your domain registrar / DNS provider)

| Record | Type | Value | Purpose |
|---|---|---|---|
| `relay.example.org` | A | RELAY_HOST IPv4 | relay endpoint |
| `dns.example.org` | A | DNS_HOST IPv4 | discovery HTTPS endpoint |
| `ns1.example.org` | A | DNS_HOST IPv4 | glue for the nameserver |
| `dns.example.org` | NS | `ns1.example.org.` | **delegate** the discovery zone to your iroh-dns-server |

The **NS delegation** is the part people miss: it makes your `iroh-dns-server` authoritative for
`dns.example.org`, so peers can resolve `_iroh.<key>.dns.example.org` over real DNS. The
`origins` in `dns/config.toml` and `origin_domain` in the app **must** equal `dns.example.org`.

## 2. Firewall / ports

- **RELAY_HOST:** open TCP 80, TCP 443, UDP 9889. (Keep 9090 metrics private.)
- **DNS_HOST:** open TCP 80, TCP 443, UDP 53, TCP 53.

## 3. Deploy the relay (on RELAY_HOST)

```sh
git clone https://github.com/muhamadjawdatsalemalakoum/dropwire && cd dropwire/infra/relay
cp ../.env.example .env
# edit .env: set IROH_RELAY_ACCESS_TOKEN to your `openssl rand -hex 32` value
# edit relay.toml: set [tls].hostname to relay.example.org
docker compose up -d --build
docker compose logs -f         # watch for a successful LetsEncrypt cert
```

> First run obtains a TLS cert via LetsEncrypt (needs port 80 reachable). If testing, set
> `prod_tls = false` in `relay.toml` to use the staging CA and avoid rate limits.

## 4. Deploy discovery (on DNS_HOST)

```sh
git clone https://github.com/muhamadjawdatsalemalakoum/dropwire && cd dropwire/infra/dns
# edit config.toml: set domains/origins to dns.example.org, rr_a to DNS_HOST IPv4,
#                   rr_ns to ns1.example.org.
docker compose up -d --build
docker compose logs -f
```

## 5. Point the app at your infra

In the desktop app's config (built into the release — see `src-tauri`), use:

```rust
Infra::SelfHosted {
    relay_url:     "https://relay.example.org/".into(),
    relay_token:   env!("DROPWIRE_RELAY_TOKEN").into(), // same secret as the relay's .env
    pkarr_relay:   "https://dns.example.org/pkarr".into(),
    origin_domain: "dns.example.org".into(),
}
```

> The token ships embedded in the build. It is app-level access control, not user auth — rotate it
> by adding a second value to the relay's `access.shared_token` list and shipping an app update,
> then removing the old one.

## 6. Verify

1. DNS delegation: `dig NS dns.example.org` returns `ns1.example.org`.
2. Relay TLS: `curl -I https://relay.example.org/` succeeds.
3. End-to-end: build a debug app with the `SelfHosted` config and run a transfer between two
   machines on **different networks**; confirm it connects and watch the direct/relayed badge.
4. **Measure your real direct-vs-relay rate** over a week of real transfers before sizing the relay
   (`[limits]` in `relay.toml`) — this is the number that drives your bandwidth bill.

## Scaling later

- Add relays in more regions; the app can be given multiple relay URLs.
- Per-relay capacity is roughly tens of thousands of concurrent connections; scale on **bandwidth**,
  not CPU. Keep flat-egress hosting to keep the bill linear and cheap.
- Optional: a status page + metrics scrape (Prometheus) on the private 9090 endpoint.
