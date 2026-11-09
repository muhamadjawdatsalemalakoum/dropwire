<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="branding/wordmark-dark.svg" />
  <img src="branding/wordmark.svg" alt="Dropwire" width="300" />
</picture>

<br /><br />

<img src="branding/promo.png" alt="Dropwire: send any file to anyone, directly. No accounts, no limits, end-to-end encrypted, free forever. Peer-to-peer, built on iroh." width="880" />

<br /><br />

A peer-to-peer file-transfer app built on [iroh](https://iroh.computer): no accounts,
no file-size limits, no server in the middle holding your data. End-to-end encrypted,
resumable, and open source.

<br />

![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-3DA35D)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-2C333D)
![Built with iroh](https://img.shields.io/badge/built%20with-iroh-D2FF3A?labelColor=0E1116)
![Latest release](https://img.shields.io/github/v/release/muhamadjawdatsalemalakoum/dropwire?label=release&color=3DA35D)

[**Download**](https://github.com/muhamadjawdatsalemalakoum/dropwire/releases) ·
[Architecture](ARCHITECTURE.md) ·
[Privacy](docs/PRIVACY.md) ·
[Contributing](CONTRIBUTING.md)

</div>

---

> **Dropwire 1.0 is here.** The first stable release: send files and folders between
> Windows, macOS and Linux, straight from one device to the other, end-to-end encrypted.
> [**Download 1.0**](https://github.com/muhamadjawdatsalemalakoum/dropwire/releases/latest) ·
> [What's new](#whats-new-in-10) · [Full changelog](CHANGELOG.md)

## What's new in 1.0

1.0 is the result of an end-to-end audit of every path a file takes: sending, receiving,
nearby devices, the desktop shell and the release pipeline. Highlights:

- **Many files, one code.** Pick or drop several files and folders and they all go under a
  single code. Up to 100,000 files per transfer.
- **Resume that really resumes.** A dropped connection keeps the receive resumable, Resume in
  Activity downloads only what is missing, and it keeps the files you picked in the preview.
- **Saving is safe by construction.** An existing file is never overwritten, one file that
  cannot be written does not stop the rest, the folder is checked before anything downloads,
  and received files no longer leave a second copy in the app's data folder.
- **Errors in plain words.** A used code, a code cut off in a chat, an offline sender and a
  full disk are told apart and explained, instead of one generic failure.
- **Nearby, hardened.** Offers only from devices seen on your network, the device you tap is
  the one asked, an offer can be taken back, and turning Nearby off stops browsing too.
- **Security.** Received files carry the "downloaded from the internet" mark, file names are
  cleaned of reserved names and hidden direction-override characters, both windows run under
  a content security policy, and each code serves only its live send to its one device.
- **A desktop app that behaves.** Start at login works, a second launch brings the running
  window forward, the tray opens on the display you clicked, and quitting shuts down cleanly.
- **Verifiable downloads.** Every installer ships with `SHA256SUMS` and a signed
  build-provenance attestation.

Everything, item by item, is in the [changelog](CHANGELOG.md#100---2026-11-09).

## Install

Grab the installer for your system from the
[latest release](https://github.com/muhamadjawdatsalemalakoum/dropwire/releases/latest):

| System | File | Notes |
| ------ | ---- | ----- |
| Windows 10 / 11 | `Dropwire_<version>_x64-setup.exe` | Installs for your user account |
| macOS 10.15+ | `Dropwire_<version>_universal.dmg` | One app for Apple Silicon and Intel |
| Linux | `.AppImage`, `.deb` or `.rpm` | Ubuntu 22.04+, Debian 12+, Fedora and relatives |

The installers are not code-signed yet, so the first launch shows a warning. On Windows
choose **More info → Run anyway**; on macOS open **System Settings → Privacy & Security** and
choose **Open Anyway**. To check a download is the one this repository built:

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify Dropwire_<version>_x64-setup.exe --repo muhamadjawdatsalemalakoum/dropwire
```

## Why Dropwire

Most "easy" file-transfer tools make you pick a poison: LocalSend only works on the same
Wi-Fi; magic-wormhole and croc are command-line tools; Send Anywhere and WeTransfer route
your files through their servers with size caps and ads. Dropwire is the missing option:

- **Free forever.** No limits, no subscriptions, no ads.
- **Truly private.** End-to-end encrypted. No account, no sign-in, no tracking. We never
  see your files, and neither does anyone else.
- **See it before you accept.** The receiver previews exactly what's coming (file names, sizes,
  and count) and approves before a single byte downloads. Both sides see, live, when the other
  device connects.
- **Nearby devices, no code needed.** Dropwire apps on the same network find each other
  automatically and appear as one-tap targets. Tap a device, they confirm, and it sends.
  Both sides confirm every transfer, and each side shows a pairing code you can read aloud
  to check you're talking to the right device. Turn nearby sharing off and your device is
  invisible: it stops advertising and refuses incoming requests.
- **Direct, peer-to-peer.** Your file goes straight from your device to theirs. When a
  direct connection isn't possible, it falls back to an encrypted relay that still can't
  read a single byte.
- **Works across the internet**, not just your local network.
- **Resumable.** A dropped connection picks up where it left off. Only the missing
  pieces are re-sent, verified end to end as they arrive.
- **Take only what you want.** Receiving a folder? Untick the files you don't need;
  only what you choose is transferred.
- **One code, one recipient.** A code isn't a public link: it's served to the first
  device that connects, and others are refused. It also stops working once you stop
  sharing.
- **Several at once.** Run multiple sends and receives in parallel, each with its own
  live progress and a direct-vs-relayed badge.
- **A desktop app, not a web page in a window.** One fixed frame that draws its own title
  bar on all three platforms. Nothing scrolls out of view, nothing needs resizing, and one
  thing is on screen at a time.
- **Stays out of the way.** It lives in the tray, so closing the window keeps you reachable
  by the people you are already talking to. Devices you have transferred with are
  remembered, and can optionally skip the consent step. Trust never grants access on its
  own: they still confirm on their side, and you still see the file list first.
- **Open source.** Dual-licensed MIT / Apache-2.0. Audit it, fork it, self-host it.

## How it works

1. **Pick** files or a folder, or drop them on the window.
2. **Share** the one-time transfer code Dropwire gives you: copy it into any chat, and
   they paste it into Receive.
3. The other person enters the code, **previews exactly what's being sent (names, sizes,
   and count) and accepts** (or declines, and you're told instantly). Then it runs
   **directly, device to device**, with a live direct-vs-relayed badge.

On the same network, you can skip the code entirely: the other device shows up under
**Nearby devices**, you tap **Send here**, and they get a confirm dialog. Nothing moves
until both sides agree, and the receiver still previews the real file list before saving.

Under the hood: each device has a stable cryptographic identity (you "dial a key, not an
IP"); peers find each other via DNS/DHT discovery; the connection is QUIC with TLS 1.3;
content is verified end-to-end with BLAKE3 so resume and integrity come for free.

## In the app

One fixed window with its own chrome on Windows, macOS, and Linux. Nothing scrolls out of
view and nothing needs resizing. These are captures of the shipped interface at its real
size, with sample transfers in it.

### Send

![Dropwire sending a folder: the one-time transfer code with its QR, the wire at 61 percent on a direct route, and nearby devices listed underneath](www/screenshots/app-send.png)

Drop a file or a folder and you get a one-time code and a matching QR. The code is a field
you copy, with the full value one click away, rather than a wall of characters that pushes
the rest of the screen out of view. Once a transfer starts the picker collapses to a bar so
the transfer is the subject of the screen. Devices on the same network sit underneath, one
tap away, with their platform next to the hostname so two similarly named machines are easy
to tell apart.

### Receive

![The verified preview: six file names and sizes, each with a checkbox, behind direct and verified-by-the-code badges](www/screenshots/app-preview.png)

Paste the code and you see the real file list before anything is written: names, sizes, and
count, committed by the transfer code so the sender cannot fake them. Untick what you do not
want. Nothing is saved until you accept.

### Nearby devices

![An incoming offer from a device called Loft-MBP, showing both pairing codes side by side to compare](www/screenshots/app-nearby-offer.png)

On the same network you can skip the code entirely. Both sides confirm, and each shows a
pairing code you read aloud to check you are talking to the right machine. Turn sharing off
and the device stops advertising and refuses incoming requests.

### Activity

![Activity: one transfer in flight at 61 percent, and four earlier ones marked done, interrupted, and failed](www/screenshots/app-activity.png)

Everything in flight and everything earlier, in one place, with a live count on the tab.
An interrupted receive offers Resume, a send that never got through offers Retry.

![The detail view for a finished send: status, size, source, start time, BLAKE3 verification, and the full file list](www/screenshots/app-detail.png)

Any row opens a detail view: the peer, the pairing code, the route, when it started, the
per-file breakdown, and the verified file list.

### First run

![The first-run welcome screen explaining what Dropwire is, with a single Set up this device button](www/screenshots/app-welcome.png)

Two screens: what Dropwire is, then name this device and choose whether to be visible.
Nearby devices cannot see you until you finish.

### Tray

<img src="www/screenshots/app-tray.png" width="300" alt="The tray panel: a drop area, a recent list, and an Open Dropwire button" />

A tray icon that reports state at a glance, and a small panel to see what is running, drop
something new, or paste a code. Closing the window keeps Dropwire running so nearby devices
can still reach you.

### Settings, and a light theme

![Settings: device name, pairing code, device ID, destination folder, nearby sharing, and trusted devices](www/screenshots/app-settings.png)

![Dropwire in its light theme](www/screenshots/app-light.png)


## Repository layout

```
core/          # `irohcore`: the transfer engine (the only crate that imports iroh/iroh-blobs)
src-tauri/     # desktop app shell (Tauri v2)
ui/            # the desktop app's frontend (plain HTML/CSS/JS, no build step)
www/           # static landing page
docs/          # PRIVACY.md, DEVELOPING.md, and other docs
branding/      # brand assets (logo, wordmark, icons)
infra/         # optional self-hosted relay + DNS configs (the released app does not use them)
ARCHITECTURE.md
```

## Building (developers)

Requires the [Rust toolchain](https://rustup.rs) and a C toolchain (see
[`docs/DEVELOPING.md`](docs/DEVELOPING.md) for per-OS prerequisites). No Node/JS build step: the
UI is plain HTML/CSS/JS.

```sh
cargo test -p irohcore --features test-utils   # engine tests, hermetic/offline
cargo run  -p dropwire                         # build + launch the desktop app
cargo tauri build                              # build installers (needs `cargo install tauri-cli`)
```

`--features test-utils` matters: the nearby-consent and relay suites declare it as a
required feature, so plain `cargo test` compiles them out and reports success without
having run them. The live-multicast discovery test is additionally `#[ignore]`d; run it
on a real network with:

```sh
cargo test -p irohcore --features test-utils --test nearby_mdns -- --ignored --nocapture
```

Two instances on one machine need separate data dirs: `cargo run -p dropwire -- --data-dir=/tmp/dw-b`.

Full developer guide: [`docs/DEVELOPING.md`](docs/DEVELOPING.md). Design: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Privacy

Dropwire collects nothing. There are no accounts, no analytics, and no phone-home. Your
node identity and transfer history live only on your machine. The only network services
involved are discovery and the relay fallback, and the relay only ever forwards encrypted
packets it cannot decrypt. See [`docs/PRIVACY.md`](docs/PRIVACY.md).

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Built with [iroh](https://iroh.computer) by [n0](https://n0.computer).
