# Browser transfer decision and verification

**Status: PARKED by owner decision on 30 September 2026.** The results below
are a historical verification snapshot. Source and evidence are retained on
`feature/browser-transfer`; there is no active browser production launch.

## Decision and references

Use iroh in WASM over browser WebSockets to compatible relays. Keep native
filesystem, Tauri, persistent identity, and mDNS code out of the browser crate.
Use the upstream blob protocol and BAO verifier with bounded browser readers
and OPFS writers; the upstream MemStore example is sufficient only for small
metadata, not whole large payloads.

Pinned dependencies: iroh **1.0.2**, iroh-blobs **0.103.0**, bao-tree **0.16.1**,
iroh-io **0.6.2**, wasm-bindgen **0.2.122**. Commit `browser/Cargo.lock` and build
with `--locked`. The native dependency versions were preserved.

Official references checked:

- [Browser blobs example](https://github.com/n0-computer/iroh-examples/tree/6a8cfcdccc6a633c5608cb25e776cb52fd509dd3/browser-blobs).
- [Pinned collection and ticket APIs](https://docs.rs/iroh-blobs/0.103.0/iroh_blobs/).
- [Pinned BAO streaming API](https://docs.rs/bao-tree/0.16.1/bao_tree/io/fsm/index.html).
- [iroh browser transport background](https://www.iroh.computer/blog/iroh-and-the-web).
- [Production relay guidance](https://github.com/n0-computer/docs.iroh.computer/blob/main/add-a-relay.mdx): public infrastructure is for testing and is rate limited; use dedicated or authenticated production infrastructure. No unlimited bandwidth or uptime promise is made.

The relay hostnames are normalized to remove the terminal FQDN dot for browser
compatibility. This is a connectivity adjustment, not a change to endpoint
identity or TLS verification.

## Real file-byte verification

The native peer was the repository's real `Core`, launched through the
loopback-only `core/examples/browser_peer.rs` fixture. Its HTTP forms carry only
test controls and metadata. File bodies use the real encrypted blob protocol.
This verifies the engine, not the graphical desktop application or installers.

The deployed preview was also tested both ways against unchanged `Core` source
from the published `v0.3.0-beta.6` tag (`cb8342d`, iroh 1.0.0 and iroh-blobs
0.103.0), using the same loopback fixture. Browser → published native saved five
files (2 MiB binary, empty, Arabic, and duplicate names); published native →
browser saved the selected binary, empty, and Arabic files. Every actual saved
file matched its source SHA-256. The fixture is a test harness, not an installed
desktop application.

| Scenario | Actual result |
|---|---|
| Browser → native | Passed: Arabic filename, empty file, and duplicate filenames; source and saved SHA-256 hashes matched |
| Native → browser | Passed: 2 MiB binary, empty file, and Arabic filename; actual Downloads hashes matched |
| Browser → browser | Passed: independent endpoint identities transferred 128 MiB; actual saved SHA-256 matched |
| More than 100 files | Passed: 101 files previewed, explicitly accepted, and saved by the native engine; all 101 hashes matched |
| Native → browser large receive | Passed: selected 128 MiB file saved with matching SHA-256; other manifest files remained unselected |
| Cancel during transfer | Passed: cancelled the repeated 128 MiB receive at 6.86%; completion controls disappeared, cleanup completed, and sender reported interruption |
| Second recipient | Passed: a second browser identity could not preview the collection already claimed by the native receiver |
| Source changed after preview | Passed: changed one byte in a 2 MiB source; receive failed integrity verification, no Save controls appeared, and sender reported interruption |
| Malformed ticket | Passed: invalid code produced an actionable error and no manifest |
| Filename safety | Passed focused checks for traversal-like names, Windows reserved names, control characters, Unicode, long names, and duplicate handling |
| Peer binding / decline | Passed focused Rust checks: wrong root/peer cannot claim or release the gate; decline during an active body cannot release it |
| Desktop regression | Complete native suite run; no native transfer implementation changed |
| Durable browser resume | Unsupported: identity and partial-transfer state are not persisted |
| Browser LAN/mDNS | Deferred: no direct or LAN claim is made |
| Quota exhaustion / device shutdown | Error handling implemented; real exhausted-quota and shutdown tests not performed |
| Multi-GB transfer | Not tested; do not extrapolate the 128 MiB byte result into a multi-GB compatibility claim |

The 128 MiB fixture's SHA-256 was
`a212c8b389fcee7469a2a4d6d012606c7fb592104aa67de661dde616fbb8c45c`.
No transfer codes, endpoint private keys, or login credentials are stored here.

## Memory observations

During the selected native → browser 128 MiB receive, WASM linear memory was
2,031,616 bytes after preview and 2,359,296 bytes after completion. This is a
measurement of WASM memory, not total JavaScript heap or browser process RSS.
The opt-in `?diagnostics=1` exposes the current linear-memory byte length in a
DOM data attribute; nothing is transmitted to a server.

Payload and proof readers each cache at most 1 MiB; proof preparation buffers
at most 1 MiB; receive writes apply backpressure per verified leaf. Payload RAM
does not grow with whole file size. At most two provider requests and eight
connections are retained. Manifest memory remains bounded separately. Storage
quota still applies to proof files and received files; Save can require an
additional copy in Downloads.

## Browser and deployment matrix

| Environment | Evidence |
|---|---|
| Windows in-app Chromium browser | All real byte tests above; exact engine version was not exposed by the browser tool |
| Standalone Chrome | Separate installation not available through the current browser tools; untested |
| Firefox | Unavailable in the current test environment; untested |
| Safari / iOS | Unavailable in the current Windows environment; untested |
| Mobile width | Requested 390 × 844 override did not change the actual 1,270 px browser viewport; device-width QA remains unverified |
| Website production build | Normal build and mandatory editorial/source checks passed |
| Website regression and dependencies | Full site checks passed; final audit patched the reported brace-expansion advisory |
| Deployed preview | Existing akoum.me project; local CLI authentication, deployed route/security-header checks, rendered landing page, platform switching, and Remotion playback |
| Platform downloads | All five official release assets downloaded; sizes and SHA-256 matched GitHub's published digests; installers were not executed |

## Before broad production browser availability

1. Revisit the relay strategy and operating costs. Public n0 relays are
   rate-limited and recommended for testing; dedicated capacity is an
   infrastructure recommendation, not an iroh protocol requirement. Configure
   the chosen strategy on both clients and repeat interoperability tests.
2. Run standalone browser/version and mobile device checks. Keep the browser
   preview label and desktop fallback until each environment is verified.
3. Exercise multi-GB payloads, disk-full behavior,
   prolonged outages, and background suspension on supported devices.
4. Add durable resume only after persisting and validating identity, verified
   partial chunks, transfer metadata, and cleanup/recovery semantics.
5. Evaluate an opt-in, authenticated local adapter or demonstrated browser
   transport for LAN transfers. Preserve peer binding and explicit acceptance.

## Existing desktop dependency advisory

The native root lockfile retains glib 0.18.5 and the repository has an open
medium advisory, [RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html),
for the VariantStrIter iterator implementation. The isolated browser lockfile
does not include glib. Resolve and validate the desktop dependency chain before
claiming that the native release has no known advisories; published installers
were not rebuilt or executed in this browser integration.
