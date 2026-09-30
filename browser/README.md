# Dropwire browser prototype — PARKED

**Parked by owner decision on 30 September 2026.** This is retained work, not an
active release. PR #18 is closed without merging; `feature/browser-transfer`
keeps the source, generated runtime, fixture, and verification evidence. The
website counterpart is parked on `feature/dropwire-staging` in akoum-me.
Desktop Dropwire is unaffected. Resume only after an explicit decision to
revisit the browser transport, capacity, and operating-cost tradeoffs.

The browser adapter uses the same `BlobTicket` collection format, blob ALPN,
BLAKE3 proofs, and authenticated iroh endpoint identities as the native engine.
File bodies move between endpoints through encrypted relay connections. The
website serves the application and public configuration; it does not receive
file bodies or transfer codes.

Payloads stream in chunks. There is no 32 MiB payload cap or 100-file product
cap. Available browser storage and disk space determine what fits. Defensive
manifest limits remain: 100,000 files, 8 MiB of name metadata, names up to 1,024
UTF-8 bytes, tickets up to 8,192 bytes, and sizes representable as safe JavaScript
integers. These bounds prevent malformed manifests from exhausting memory.

Sending reads the selected File objects in bounded windows and writes their
integrity outboards to Origin Private File System (OPFS). Receiving validates
each BLAKE3 chunk before awaiting its disk write. One receive writer is active
at a time. Only verified, completed files receive Save buttons. Downloads use
OPFS File objects without converting whole files into ArrayBuffers.

Keep both endpoints open. Retry starts selected files again in the same session.
Reloading creates a new identity; durable browser resume is unavailable. Stop
transfer & clear closes the endpoint and deletes this session's temporary OPFS
directory. A crashed or closed tab can leave temporary data until the user clears
this site's storage. Downloads already saved are unaffected.

## Build

Prerequisites: Rust with `wasm32-unknown-unknown`, a WASM-capable clang, and
`wasm-bindgen-cli` 0.2.122. On Windows the checked build used WASI SDK 34 clang.
Keep toolchains and build output on a data drive.

```powershell
$env:CARGO_TARGET_DIR = 'E:\Coding Projects\dw-wt\browser-target'
$env:CC_wasm32_unknown_unknown = 'E:\Coding Projects\dw-wt\tools\wasi-sdk-34.0-x86_64-windows\bin\clang.exe'
cargo build --manifest-path browser\Cargo.toml --target wasm32-unknown-unknown --release --locked
wasm-bindgen "$env:CARGO_TARGET_DIR\wasm32-unknown-unknown\release\dropwire_browser.wasm" --target web --out-dir browser\public\runtime
cargo test --manifest-path browser\Cargo.toml --lib --locked
npm --prefix browser test
```

Copy `browser/public/*` into the website's `public/dropwire/transfer/`. Its
rewrites mount the app at `/dropwire/send` and `/dropwire/receive`. Serve over
HTTPS, or loopback HTTP for local development. Do not open the app as a local
file. The website prebuild generates `config.json`; no credentials belong there.

## Archived deployment configuration

While parked, the retained website disables browser transfer startup in local,
preview, and production builds. Relay environment variables cannot activate it.
The static prototype configuration is also disabled. Old immutable previews
remain historical test artifacts; they are not a current launch.

The earlier production gate was a conservative release policy, not an iroh
technical requirement. Both native and browser clients can use public n0 relays.
The browser prototype sends every transfer through a relay, while native
clients can connect directly. Public relays are rate-limited and recommended
for development/testing; dedicated infrastructure is a capacity and reliability
choice, with possible hosting and bandwidth costs. Before resuming, choose and
validate the intended relay strategy.
Their operators can observe endpoint identifiers, network addresses, timing,
and volume, but not plaintext files. Published native builds use their existing
relay configuration; native-to-browser transfers also require the native
sender's relay origin to be permitted by the deployed CSP. Configure and test
both clients together when moving to dedicated infrastructure.

Browser nearby discovery, native mDNS browsing, and a direct LAN path are
unavailable. A LAN design must use authenticated, opt-in native pairing or an
additional demonstrated browser transport; shared public IP is insufficient.

See [FEASIBILITY.md](FEASIBILITY.md) for verified references, measured results,
and the remaining production requirements.
