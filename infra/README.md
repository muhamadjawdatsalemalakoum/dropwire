# Optional: self-hosted relay and discovery

**The released Dropwire app needs none of this.** It finds peers over the public Mainline DHT, and
when two devices can't connect directly it falls back to n0's free public relay. The maintainer
runs no server of any kind, and relayed transfers work out of the box.

This folder is for someone who builds their own copy of Dropwire and wants a relay and discovery
service they control instead, for example to stop depending on n0's public relays, which are
rate-limited and come with no SLA. Using it means switching the engine to `Infra::SelfHosted` and
rebuilding the app (step 5 of [`deploy.md`](deploy.md#5-point-the-app-at-your-infra)). It is not
Dropwire's own infrastructure.

Everything here is "build up to the gate": the configs and containers are ready; you provide a
server, a domain, and run one command. A relay never sees anyone's files. It only forwards
encrypted packets it cannot decrypt.

## What runs

| Service | What it does | What it replaces |
|---|---|---|
| **iroh-relay** (`relay/`) | Forwards **encrypted** packets between two peers when a direct connection can't be hole-punched. Also helps with NAT discovery. | n0's public relay, for the minority of connections that can't go direct. Only those relayed bytes cost anything to carry. |
| **iroh-dns-server** (`dns/`) | pkarr-based discovery: lets a device find another by its public key. | Discovery over the public Mainline DHT. |

Both are open-source binaries from the [iroh](https://github.com/n0-computer/iroh) monorepo, built
from a pinned tag in the Dockerfiles.

## Layout

```
infra/
├─ README.md           # this file
├─ deploy.md           # step-by-step deploy + the human-gate checklist
├─ .env.example        # the relay access token (copy to .env)
├─ relay/              # the relay service (own host, owns :443)
│  ├─ docker-compose.yml
│  ├─ Dockerfile
│  └─ relay.toml
└─ dns/                # the discovery service (own host, owns :443 + :53)
   ├─ docker-compose.yml
   ├─ Dockerfile
   └─ config.toml
```

## Why two hosts

Both services want port **443** (LetsEncrypt TLS), so the simplest correct layout is **one small
VPS each** (about $5/mo each on a flat-egress host like Hetzner; flat, unmetered egress matters
most for the relay). They can be co-located behind a reverse proxy, but the relay's QUIC (UDP 9889)
and the DNS server's UDP/TCP 53 don't proxy cleanly, so separate hosts keep it simple and robust.
Start here; scale the relay horizontally on bandwidth later.

## Lock it to your build (no user accounts)

If you build your own copy with a self-hosted relay, the relay is locked to that build with a
**shared token** (`access.shared_token` / `IROH_RELAY_ACCESS_TOKEN`). Your build embeds the same
token and presents it on connect, so third parties can't use your relay as an open proxy. This is
**app-level** access control, not a user login, so the "no account" promise holds. The released
Dropwire build embeds no such token and uses no self-hosted relay.

## Cost if you self-host

The released app costs nobody anything to run. If you run these services, the cost falls on you:
direct transfers still cost nothing, and relayed bytes are the bill, roughly **$0.03 per user per
month on metered cloud and close to $0 on flat-egress hosting**, driven by the real direct-vs-relay
rate. **Measure your actual relay rate** on representative networks before sizing; see the
`[limits]` section in `relay/relay.toml`.

Next: [`deploy.md`](deploy.md).
