# Changelog

All notable changes to Dropwire are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
- Release profile now unwinds on panic instead of aborting, so a fault in a
  background discovery thread degrades nearby discovery rather than crashing the
  whole app; the desktop shell also logs panics to `panic.log` in its data dir.

### Fixed
- **macOS launch crash**: the always-on mDNS threads could abort the process
  under `panic = "abort"`; switched to unwind + poison-safe locks + a panic hook.
  The macOS bundle is now ad-hoc signed so Gatekeeper/TCC have a stable identity.
- **Nearby sharing off now truly means invisible**: incoming offers are declined
  at the consent layer when Nearby is off (previously the check was computed and
  discarded, so anyone who knew your endpoint id — over the LAN or the relay —
  could still pop a consent dialog).
- **Pairing fingerprint** is now a BLAKE3 hash of the identity (60 bits) instead
  of a grindable ~21-bit prefix of its hex form, and the receiver derives it from
  the authenticated remote id rather than a sender-supplied field.
- **Accepting a nearby offer** now goes through the same verified preview as the
  code flow (real file names/sizes from the manifest), instead of downloading on
  the sender's unverified claimed metadata.
- Nearby devices no longer vanish from the list ~30 seconds after discovery while
  still present; presence now follows mDNS add/remove events.
- A crafted or colliding mDNS instance name can no longer evict a different
  peer from the nearby list (removal matches the exact instance).
- Incoming offers survive a burst (broadcast-lag no longer permanently kills the
  offer pump), a second "Send here" can't cancel a transfer another device just
  accepted, a new offer no longer replaces the consent dialog mid-decision, and a
  lapsed offer is cleaned up and reported as expired instead of leaking.
- **Windows firewall rules** are now applied through an elevated step (the
  per-user installer could not add them before, so they silently did nothing).
- CI runs the nearby consent + relay suites (they were gated behind a feature the
  workflow never enabled) and now includes a macOS job.

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

[Unreleased]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/releases/tag/v0.1.0
