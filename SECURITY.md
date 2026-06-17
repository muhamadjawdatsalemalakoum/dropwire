# Security Policy

Dropwire is a security- and privacy-sensitive product. We take reports seriously.

## Reporting a vulnerability

**Please do not open public issues for security problems.** Report privately through
**[GitHub Security Advisories](https://github.com/muhamadjawdatsalemalakoum/dropwire/security/advisories/new)**
(the repo's *Security → Report a vulnerability* button) with details and, if possible, a proof of
concept. We'll acknowledge within a few days and keep you updated through to a fix and coordinated
disclosure.

## What Dropwire guarantees

- **End-to-end encryption.** Connections use QUIC with TLS 1.3 (via `rustls`). Data is encrypted
  between the two devices; intermediaries cannot read it.
- **No server holds your files.** Transfers are peer-to-peer. When a direct connection can't be
  made, traffic falls back to a relay that **only forwards already-encrypted packets** — it cannot
  decrypt them, and Dropwire runs no relay of its own (it uses the public iroh relay network).
- **No accounts, no tracking.** There is no sign-in, no telemetry, and no analytics. Identity is a
  per-device public key; the only capability shared is the per-transfer ticket.

## Trust boundary

The transfer **ticket** is the capability: anyone who has it can fetch that content while the
sender is serving it. Treat it like a one-time password — share it only with the intended recipient.

## Known limitations (be honest)

- **No third-party security audit yet.** The underlying QUIC/TLS/`rustls` stack is widely vetted
  upstream, but iroh's own handshake/relay code and Dropwire's glue have not been independently
  audited. An audit is on the roadmap before any "enterprise-ready" claim.
- The discovery layer publishes a signed record (device public key → relay address) to the public
  Mainline DHT so peers can find you. It contains no file data, but it is, by design, public.

## Supported versions

Dropwire is pre-1.0; security fixes target the latest `main`. Pinned dependencies of note:
`iroh 1.0`, `iroh-blobs 0.10x` (pre-1.0, wrapped behind our `irohcore` API).
