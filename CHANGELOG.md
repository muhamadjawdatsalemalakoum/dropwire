# Changelog

All notable changes to Dropwire are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project aims to follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
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

[Unreleased]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.3...HEAD
[0.2.3]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/muhamadjawdatsalemalakoum/dropwire/releases/tag/v0.1.0
