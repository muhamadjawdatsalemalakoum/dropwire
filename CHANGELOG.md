# Changelog

All notable changes to Dropwire are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0-beta.6] - 2026-09-04

Two ways to get stuck, both found on real hardware.

### Fixed
- **macOS: closing the window made Dropwire unreachable.** Close means
  "keep running so nearby devices can still find you", so the window hides
  rather than quitting. But nothing answered the dock icon, so the app was
  alive with no way back into it: it read as frozen, and the only way out
  was Cmd+Q. Clicking the dock icon now brings the window back.
- **First run could not be escaped.** The setup screen covered the whole
  window, and the window is frameless, so its own title bar was the only way
  to move or close the app and setup had hidden it. Sheets and the detail
  drawer captured the bar the same way. Overlays now start below the title
  bar and stop inside the window border, so the chrome stays live and the
  frame keeps its rounded corners.
- The tray panel's only button sat at its text width in the corner: it
  carried a class the stylesheet never defined.
- The select-all checkbox in the receive preview used the browser's default
  accent instead of the app's.
- Landing page: reversed showcase rows put the screenshot in the narrow
  column, so alternating rows showed the app at two different sizes.

### Changed
- The README and the landing page show the 0.3.0 interface. The captures
  they used were still from 0.2.

## [0.3.0-beta.5] - 2026-08-26

The interface rebuild. Dropwire now behaves like a desktop application rather
than a web page in a window: one fixed frame, its own chrome, and a layout that
never asks you to scroll or resize to finish a task.

### Added
- **Frameless window.** Dropwire draws its own title bar, window controls and
  12px corner on all three platforms, so the app looks the same everywhere
  instead of inheriting three different system frames.
- **One canvas, three segments.** The icon rail and four separate views are
  replaced by a `Send · Receive · Activity` switch. Activity carries a live
  count. Settings is a labelled control (or `Ctrl+,`), not an icon to guess at.
- **Activity.** Everything in flight and everything earlier, in one place, with
  a detail view on any row: peer, pairing code, route, start time, per-file
  breakdown, and the verified file list. History can now be cleared.
- **First-run setup.** Two screens: what Dropwire is, then name this device and
  choose whether to be visible. Nothing touches the network until you finish.
- **Trusted devices.** A device is remembered after a transfer with it
  completes. Optionally let trusted devices skip the consent dialog — they
  still confirm on their side, and you still see the file list before anything
  is written.
- **Tray.** A tray icon that reports state at a glance (idle, transferring,
  complete, needs attention) and a small panel to see what is running, drop
  something new, or paste a code. Closing the window keeps Dropwire running so
  nearby devices can still reach you.
- **Send text.** Send a snippet or your clipboard as a transfer. It travels the
  same encrypted path as any other file.
- **Notifications.** Three events, and only three: a transfer finished, an
  offer arrived while you were away, and a failure that needs a decision.
  Progress never notifies.
- Nearby devices now show their platform next to the hostname, which is the
  fastest way to tell two similarly named machines apart.

### Changed
- The share code is a one-line field you copy, with the full value one click
  away. It used to render every character at display size and push the rest of
  the screen out of view.
- Send and Receive collapse their entry area once a transfer starts, so the
  transfer is the subject of the screen rather than competing with a form.
- Nearby devices sit inside the send surface instead of a separate panel that
  fell below the fold.
- The wire is the only progress indicator in the app. Completion is green
  rather than lime, so "done" can no longer be mistaken for "still running", a
  dropped connection breaks the wire in red, and resuming is blue.
- Settings gained the theme picker, the device name, trusted devices, and the
  tray and startup options.

### Fixed
- Content no longer scrolls out of the window or hides behind the credit line.
- The pairing code is never truncated. Shortening it would quietly weaken the
  check two people make by reading it aloud.

## [0.3.0-beta.4] - 2026-08-25

### Fixed
- The consent dialog no longer stacks on top of the receive preview, and
  accepting an offer that has already expired says so instead of failing
  silently.

## [0.3.0-beta.3] - 2026-08-25

An end-to-end audit of the nearby feature before it shipped, and the fixes it
turned up.

### Fixed
- **macOS launch crash.** The always-on discovery threads could abort the whole
  app. The release profile now unwinds instead of aborting, locks recover
  rather than cascade, and panics are written to `panic.log` in the app data
  dir. The macOS bundle is also signed now, so the system treats it
  consistently across updates.
- **"Nearby sharing off" now means invisible.** Incoming offers are refused at
  the consent layer, not merely hidden. Previously anyone who knew this
  device's id could still raise a dialog, including from outside the network.
- **Pairing code strengthened.** It is derived from a hash of the whole
  identity instead of a short prefix that could be guessed at, and the receiver
  computes it from the authenticated connection rather than trusting what the
  sender claims.
- **Accepting a nearby offer** goes through the same verified preview as a code
  transfer, so the file names and sizes you approve are the ones that arrive.
- Nearby devices no longer disappear from the list about thirty seconds after
  they are found while still present.
- A crafted or colliding network name can no longer remove a different device
  from the nearby list.
- Incoming offers survive a burst instead of silently stopping for the rest of
  the session, a second "Send here" cannot cancel a transfer another device
  just accepted, a new offer no longer replaces the dialog while you are
  reading it, and an offer that lapses says so.
- **Windows firewall rules** are applied during install. The per-user installer
  could not add them before, so they silently did nothing.
- CI runs the nearby consent and relay suites, which were gated behind a
  feature the workflow never enabled, and now includes a macOS job.

## [0.3.0-beta.1] - 2026-08-23

### Added
- **Nearby devices with two-sided consent** (issue #2): Dropwire instances on
  the same network discover each other automatically over mDNS/DNS-SD and are
  offered as one-tap transfer targets. Nothing moves until BOTH sides confirm —
  the sender picks a device ("Send here") and waits, the receiver gets a consent
  dialog showing file count, total size, and a pairing fingerprint to compare.
  Declining notifies the sender instantly; with Nearby sharing off, the device
  is invisible to others. Bluetooth discovery/bootstrap is planned as the next
  transport behind the same flow.
- Engine: `irohcore` nearby module — mDNS advertise/browse (`_dropwire._udp.`),
  consent handshake over the control channel (`Offer`/`OfferAccept`/
  `OfferDecline` frames on the offer's own connection), one-to-one binding at
  offer time, and self-decline when an offer goes unanswered.
- Shell: `my_fingerprint`, `nearby_start`, `nearby_stop`, `nearby_list`,
  `nearby_offer`, `nearby_respond` commands + incoming-offer event pump.
- UI: "Nearby devices" panel (radar animation, device rows with pairing
  codes, share toggle) and the incoming-offer modal with a verify-your-pairing-
  code step between offer and accept.
- "See it in action" showcase with real Windows + macOS product screenshots,
  and a tap-to-zoom lightbox.
- Equal billing for Linux across the site (showcase, platform line, and
  AppImage / `.deb` / `.rpm` download notes).
- Production-ready SEO/GEO for the landing page: canonical + Open Graph/Twitter
  cards, JSON-LD (SoftwareApplication, FAQPage, Organization, WebSite),
  `robots.txt`, `sitemap.xml`, custom `404.html`, `llms.txt`, web manifest.
- `/guides/` section with four cornerstone articles (send large files free,
  free WeTransfer alternatives, share files Mac/Windows/Linux, what is P2P
  file transfer) plus an index.
- Issue templates and this changelog.

### Changed
- The project is now public and the landing page is live on GitHub Pages.
- Removed internal-only planning docs and the unused `dropwire.app` domain
  references; the site is GitHub-hosted.

## [0.2.3] - 2026-06-17

### Fixed
- macOS DMG: taller installer window so the background artwork isn't clipped.

## [0.2.2] - 2026-06-17

### Added
- Author and open-source credit (GitHub + LinkedIn) across the app and site.

### Fixed
- macOS DMG: tag the installer background at 72 DPI so it fills the window.

## [0.2.1] - 2026-06-17

### Added
- Branded, per-user Windows installer (NSIS) with custom header/sidebar art.
- 1200×630 `og.png` social card for link previews.

### Removed
- The WiX MSI installer (NSIS is now the Windows bundle).

## [0.2.0] - 2026-06-17

### Added
- **Preview before accept** — the receiver sees file names, sizes, and count
  and approves before any bytes download.
- **Selective download** — receive only the files you choose from a folder.
- **One-to-one** — a ticket is bound to the first device that connects; others
  are refused.
- **Two-way control channel** — live presence, instant decline, and a free
  side channel between the two devices.
- **Multiple transfers at once**, each with its own per-card live progress and
  a direct-vs-relayed badge.
- **Resend** a past send from history.
- Honest "sender offline / link expired" state.
- Relay-path transfer + resume proven on a single machine (test).
- Finalized logo system, regenerated icons, and a feature-complete landing page.

### Changed
- Seamless UI/UX pass: accessibility, contrast, finished states, and copy.

## [0.1.0] - 2026-06-16

### Added
- `irohcore` transfer engine on iroh 1.0 + iroh-blobs 0.103.
- Tauri v2 desktop app shell (commands, window, icons).
- Wire-themed desktop frontend.
- Optional self-hosted relay and DNS configs.
- Static landing page.
- CI (engine tests on Linux + Windows) and a cross-platform release workflow
  (Windows, macOS, Linux) that publishes downloads automatically.

[Unreleased]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.3.0-beta.6...HEAD
[0.3.0-beta.6]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.3.0-beta.5...v0.3.0-beta.6
[0.3.0-beta.5]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.3.0-beta.4...v0.3.0-beta.5
[0.3.0-beta.4]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.3.0-beta.3...v0.3.0-beta.4
[0.3.0-beta.3]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.3.0-beta.1...v0.3.0-beta.3
[0.3.0-beta.1]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.3...v0.3.0-beta.1
[0.2.3]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/releases/tag/v0.1.0
