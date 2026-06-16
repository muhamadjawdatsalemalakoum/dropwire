# Privacy

Short version: **Dropwire collects nothing.** No accounts, no analytics, no phone-home, no ads.

## What never leaves your device

- **Your files** — they go directly to the recipient's device, end-to-end encrypted. Dropwire has
  no cloud and stores no copies anywhere.
- **Your transfer history** — kept in a local file (`transfers.json`) in the app's data folder, on
  your machine only.
- **Your identity key** — a per-device keypair (`node.key`), generated locally, never uploaded.

## What is visible, and to whom

- **Discovery (public DHT):** so a recipient can find you by your device's public key, Dropwire
  publishes a small *signed* record to the public Mainline DHT mapping `your public key → a relay
  address`. It contains **no file data and no personal data** — but, like all DHT records, it is
  public while you're online. It expires automatically when you go offline.
- **Relay fallback:** if a direct device-to-device connection can't be established, encrypted
  packets are routed through the public iroh relay network. The relay sees encrypted bytes and the
  two endpoints' addresses; it **cannot read your files** and stores nothing. Dropwire runs no relay
  of its own. The app shows a "direct" vs "relayed" badge so you always know.

## No tracking

There is no telemetry, no crash reporting that phones home, no usage analytics, and no third-party
SDKs. The desktop app makes no network connections except those required to find your peer and move
your bytes.

## Your control

Everything is local and inspectable (Dropwire is open source). Delete the app's data folder and
nothing remains. There is no account to close because there is no account.
