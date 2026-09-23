# Developing Dropwire

## Layout

```
core/        irohcore — the transfer engine (only crate that imports iroh / iroh-blobs)
src-tauri/   the desktop app shell (Tauri v2): commands, window, config, icons
ui/          the app frontend (vanilla HTML/CSS/JS; loaded by Tauri as frontendDist)
infra/       OPTIONAL self-hosted relay + DNS (not needed — the app is serverless by default)
www/         marketing landing page (static)
docs/        PRIVACY.md, this file, and other docs
```

The golden rule: **`iroh` / `iroh-blobs` types never appear outside `core/`.** The shell and UI
speak only `irohcore`'s stable API (`Core`, `Progress`, `CoreConfig`).

## Prerequisites

- **Rust** 1.91 or newer, from https://rustup.rs (`rust-toolchain.toml` selects the stable channel).
- **A C toolchain** for the native crypto/QUIC deps:
  - **Windows:** Visual Studio Build Tools with the *Desktop development with C++* workload, plus
    WebView2 (ships with Windows 11). Build from a shell that has the MSVC env loaded — either the
    "x64 Native Tools" prompt, or import `vcvars64.bat` before running cargo (see below).
  - **macOS:** Xcode command line tools.
  - **Linux:** the WebKitGTK 4.1 stack and a C compiler. On Debian/Ubuntu, the same packages CI
    installs: `sudo apt install build-essential libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev`.
- **Node** is *not* required — the UI is plain HTML/CSS/JS with no build step.

### Windows: loading the MSVC environment for cargo

```powershell
$vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cmd /c "`"$vcvars`" && set" | ForEach-Object { if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($matches[1])" -Value $matches[2] } }
# now `cargo` can find link.exe and the Windows SDK libs
```

## Engine (fast inner loop)

```sh
cargo test   -p irohcore --features test-utils                    # the full engine suite, as CI runs it
cargo test   -p irohcore --test transfer resume_after_interrupt   # just the 64 MB interrupt + resume test
cargo clippy -p irohcore --all-targets --features test-utils -- -D warnings
cargo fmt --all
```

`--features test-utils` matters. The nearby consent and relay integration tests declare it as a
required feature (`[[test]] required-features` in `core/Cargo.toml`), so without it cargo skips
those suites without a word and still reports success.

The engine tests use `Infra::LocalOnly` (loopback only, no relay or discovery) and the relay tests
run their own in-process relay, so the suite is hermetic and needs no network. Two opt-in tests
do touch a real network and are `#[ignore]`d by default:

```sh
cargo test -p irohcore --test transfer roundtrip_serverless -- --ignored                   # public DHT + n0's relay
cargo test -p irohcore --features test-utils --test nearby_mdns -- --ignored --nocapture   # live mDNS on your LAN
```

## Running the desktop app

```sh
cargo run    -p dropwire                                  # builds the shell + engine and opens the window
cargo clippy -p dropwire --all-targets -- -D warnings     # lint the shell, as CI does on all three OSes
```

No dev server is needed (the UI is static and loaded from `../ui`). The app starts with the
serverless config: Mainline-DHT discovery + n0's free public relay fallback.

## Building installers

Requires the Tauri CLI:

```sh
cargo install tauri-cli --version "^2"
cargo tauri build           # produces platform installers under target/release/bundle/
```

Code signing (Windows cert, Apple Developer ID + notarization) is a release-time step. See the
signing notes at the top of `.github/workflows/release.yml`; release builds are unsigned by default,
and unsigned local installers build fine for testing.

## Regenerating app icons

Icons in `src-tauri/icons/` are derived from `branding/icon.svg` (the lime "wire" mark on Wire
Black). To regenerate from a 1024px PNG source: `cargo tauri icon path/to/icon.png`.
