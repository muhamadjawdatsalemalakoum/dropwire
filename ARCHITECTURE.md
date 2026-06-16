# irohtransfer — Architecture (Desktop MVP)

> Status: **design draft**, 2026-06-16. Scope: the **desktop** target (Windows / macOS / Linux).
> Mobile, web, and enterprise hardening are explicitly out of scope here — see [Roadmap](#14-out-of-scope--roadmap).
>
> **Product decision (2026-06-16):** completely free (donations/grants only, no paid tier), **zero servers the
> maintainer runs**, **one-to-one transfers only**. Default infra is serverless — Mainline-DHT discovery (crate
> `iroh-mainline-address-lookup`) + n0's free public relay fallback (`Infra::Decentralized`). The self-hosted
> relay/DNS in §6 is now an **optional** advanced path, not the default.
>
> Pinned stack: **iroh 1.0.0** (stable) · **iroh-blobs 0.103.0** (0.10x line, pre-1.0 — wrapped) · **Tauri v2**.
> All API names below are from primary sources verified on 2026-06-16; items that still need confirmation
> at implementation time are collected in [§13 Verify-before-coding](#13-verify-before-coding).
> Background research: [`_research_synthesis.md`](_research_synthesis.md) (the why) and
> [`_spec_specifics.md`](_spec_specifics.md) (the code-level how).

---

## 1. Goals & non-goals

**Goal:** a polished desktop app where a non-technical user picks a file or folder, gets a short code + QR,
and the recipient enters it to receive the data — **directly peer-to-peer, end-to-end encrypted, resumable,
with no account and no central server carrying the bytes** (except the relay fallback we self-host).

**Design principles**

1. **Isolate the volatile dependency.** `iroh-blobs` is pre-1.0 and mid-rewrite. *Nothing outside one Rust crate
   may import `iroh_blobs`.* The rest of the app talks to a stable internal API we own.
2. **One core, one endpoint.** A single shared Rust core owns exactly one long-lived `iroh::Endpoint` for the
   app's lifetime. The UI is a thin shell over it.
3. **Model on `sendme`.** n0's official CLI is a working reference for this exact use case (import → Collection →
   HashSeq ticket → serve; receive → resume → export). We copy its flow and invert only the lifecycle choices
   that differ for a long-running GUI (persistence, no temp-dir-per-transfer, no Ctrl-C lifecycle).
4. **Self-host the floor.** Never depend on n0's free public relay/DNS in production (dev/test only, rate-limited,
   no SLA). We run our own relay + DNS, app-locked with an embedded token, and keep n0 as a dev/fallback toggle.

**Non-goals (MVP):** mobile, browser/WASM, multi-provider swarming, accounts/contacts, an inbox/push for offline
recipients (both peers must be online for a transfer), enterprise key lifecycle/audit.

---

## 2. Layered architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Frontend (web tech in Tauri webview)                          │  UI: pick/drop, code+QR, progress, direct/relay badge
│  - invoke() commands, Channel<TransferEvent> for progress      │
└───────────────▲──────────────────────────────┬───────────────┘
                │ IPC (commands + Channel)       │
┌───────────────┴──────────────────────────────▼───────────────┐
│  src-tauri  (THIN shell)                                       │  Tauri Builder, #[tauri::command]s, AppState,
│  - owns Tauri state, maps core Progress → TransferEvent        │  cancellation registry, dialog/drag-drop wiring
│  - depends on irohcore via a path/workspace dep                │  *** imports `irohcore` only — never `iroh_blobs` ***
└───────────────────────────────┬───────────────────────────────┘
                                 │ stable internal API (our own types)
┌────────────────────────────────▼──────────────────────────────┐
│  irohcore  (the ONLY crate that imports iroh / iroh-blobs)      │  Endpoint + FsStore + Router + transfer logic
│  - send(path) / receive(ticket,dest) / cancel / progress       │  send/receive modeled on sendme
│  - self-hosted relay + DNS wiring, identity persistence        │  *** the firewall around blobs churn ***
└───────────────────────────────┬────────────────────────────────┘
                                 │ QUIC (BLAKE3-verified streams)
                  ┌──────────────▼───────────────┐
                  │ Self-hosted infra (you run)   │  iroh-relay (token-locked) + iroh-dns-server (pkarr)
                  └───────────────────────────────┘
```

The hard rule is the line between `src-tauri` and `irohcore`: **`iroh_blobs` symbols never appear above the
`irohcore` boundary.** When blobs breaks its API (it will, on the 0.10x line) or reaches 1.0, we change one crate.

---

## 3. Repository layout

A Cargo workspace with the core split out so it's reusable by future mobile/CLI shells.

```
irohtransfer/
├─ Cargo.toml                  # [workspace] members = ["core", "src-tauri"]
├─ core/                       # crate `irohcore` — the stable wrapper (no Tauri deps)
│  ├─ Cargo.toml               # iroh, iroh-blobs, tokio, n0-future, tokio-util, serde, thiserror
│  └─ src/
│     ├─ lib.rs                # public API: Core, CoreConfig, Progress, Ticket, errors
│     ├─ endpoint.rs           # identity persistence + endpoint build (self-hosted infra wiring)
│     ├─ send.rs               # import path → Collection → HashSeq ticket → serve
│     ├─ receive.rs            # parse ticket → resume → execute_get → export
│     ├─ store.rs              # FsStore open + transfers metadata DB (resume catalog)
│     └─ progress.rs           # internal blobs progress → our Progress enum
├─ src-tauri/                  # the desktop shell
│  ├─ Cargo.toml               # tauri, tauri-plugin-dialog, irohcore = { path = "../core" }
│  ├─ tauri.conf.json
│  ├─ capabilities/default.json
│  └─ src/{main.rs, lib.rs}
├─ src/  (or ui/)              # frontend (framework TBD — see §8)
├─ infra/                      # relay + dns-server configs + deploy notes (§6)
│  ├─ iroh-relay.toml
│  └─ iroh-dns-server.toml
├─ ARCHITECTURE.md             # this file
└─ docs/research/              # (optional) move _research_synthesis.md / _spec_specifics.md here
```

---

## 4. `irohcore` — the stable internal API

This is the most important artifact in the project: the surface the rest of the app is allowed to see.
Designed so it would not change even if we swapped the entire blobs implementation underneath.

```rust
// core/src/lib.rs  (illustrative — the contract, not the impl)

pub struct CoreConfig {
    /// App data dir. Holds: node.key (identity), blobs/ (FsStore), transfers.db (resume catalog).
    pub data_dir: std::path::PathBuf,
    pub infra: Infra,
}

/// Where relay + discovery come from. Self-hosted is the product path; N0 is a dev/fallback toggle.
pub enum Infra {
    SelfHosted {
        relay_url: String,      // "https://relay.yourapp.example/"
        relay_token: String,    // embedded shared secret (matches server access.shared_token)
        pkarr_relay: String,    // "https://dns.yourapp.example/pkarr"
        origin_domain: String,  // "dns.yourapp.example" (must match dns-server [dns].origins)
    },
    N0Default,                  // presets::N0 — dev only
}

pub struct Core { /* endpoint: iroh::Endpoint, store: FsStore, router: Router, transfers: ... */ }

#[derive(Clone, Copy, PartialEq)]
pub enum TransferId(/* uuid */);

/// The ONE progress type the UI sees. Maps to a Tauri Channel event in src-tauri.
pub enum Progress {
    Importing  { id: TransferId, done: u64, total: u64 },   // send: hashing/copying into store
    Ready      { id: TransferId, ticket: String },          // send: ticket minted, now serving
    PeerJoined { id: TransferId },                           // a receiver connected (send side)
    Transferring { id: TransferId, offset: u64, total: u64, via: Route },
    Done       { id: TransferId, bytes: u64, secs: f64 },
    Error      { id: TransferId, message: String },
    Cancelled  { id: TransferId },
}

/// Surfaced in the UI as the "direct vs relayed" badge (truth comes from the connection path type).
pub enum Route { Direct, Relayed, Unknown }

impl Core {
    pub async fn start(cfg: CoreConfig) -> Result<Core, CoreError>;
    pub fn endpoint_id(&self) -> String;

    /// Import a file or directory, start serving it, and stream Progress (ending in Ready{ticket}, then
    /// PeerJoined/Transferring as receivers pull). Returns immediately with the TransferId.
    pub async fn send(&self, path: PathBuf) -> Result<(TransferId, ProgressStream), CoreError>;

    /// Parse a ticket and download (resuming if partial data exists) into `dest`. Streams Progress.
    pub async fn receive(&self, ticket: String, dest: PathBuf) -> Result<(TransferId, ProgressStream), CoreError>;

    pub async fn cancel(&self, id: TransferId);
    pub async fn shutdown(self) -> Result<(), CoreError>;
}

// ProgressStream = impl futures::Stream<Item = Progress> (e.g. wrapping a tokio mpsc receiver)
```

Everything below uses real `iroh` / `iroh_blobs` symbols; everything above uses only the types in this section.

---

## 5. Core internals

### 5.1 Identity (persistent node key)

A stable identity makes tickets/relays predictable and lets the same machine resume. Load-or-generate an
Ed25519 `SecretKey`, persist `to_bytes()` (32 bytes, `0600`) to `data_dir/node.key`.

```rust
let sk = match std::fs::read(&key_path) {
    Ok(b) if b.len() == 32 => iroh::SecretKey::from_bytes(&b.try_into().unwrap()),
    _ => { let sk = iroh::SecretKey::generate(); std::fs::write(&key_path, sk.to_bytes())?; sk }
};
```

### 5.2 Endpoint (self-hosted infra wiring)

Build **one** endpoint at `Core::start`. For the product path use `presets::Minimal` (so we do **not** silently
inherit n0's relay/DNS) and attach our own relay + address-lookup:

```rust
use iroh::{Endpoint, RelayMap, RelayConfig, RelayMode, endpoint::presets,
           address_lookup::{dns::DnsAddressLookup, pkarr::PkarrPublisher}};

let relay_cfg = RelayConfig::from(relay_url.parse::<iroh::RelayUrl>()?)
    .with_auth_token(relay_token);                 // matches server access.shared_token
let endpoint = Endpoint::builder(presets::Minimal)
    .secret_key(sk)
    .alpns(vec![iroh_blobs::ALPN.to_vec()])         // server side advertises the blobs ALPN
    .relay_mode(RelayMode::Custom(RelayMap::from_iter([relay_cfg])))
    .address_lookup(PkarrPublisher::builder(pkarr_relay.parse()?))   // publish our addr to our DNS
    .address_lookup(DnsAddressLookup::builder(origin_domain))        // resolve peers via our DNS
    .bind().await?;
let _ = endpoint.online().await;   // wait for relay handshake so endpoint.addr() is reachable
```

> **Naming note (iroh 1.0 rename):** `NodeId→EndpointId`, `NodeAddr→EndpointAddr`, and **`discovery`→`address_lookup`**.
> Any pre-June-2026 tutorial using `.discovery()` / `NodeAddr` will not compile.
> `Endpoint::builder()` now **requires a preset** argument.

### 5.3 Store

One persistent on-disk BLAKE3 store for the app lifetime — **not** sendme's temp-dir-per-run model:

```rust
let store = iroh_blobs::store::fs::FsStore::load(data_dir.join("blobs")).await?;
```

`FsStore` persistence is what makes resume real across app restarts (`MemStore` loses partials). See §10.

### 5.4 Send flow (`core/src/send.rs`)

Mirrors `sendme`:

1. Walk the path (`walkdir`), for each file `store.add_path_with_opts(AddPathOptions { path, mode: ImportMode::TryReference, format: BlobFormat::Raw })`, draining the `AddProgress` stream (`AddProgressItem::{Size, CopyProgress, CopyDone, OutboardProgress, Done(TempTag), Error}`) → emit `Progress::Importing`.
2. Fold `(relative_name, tag.hash())` pairs into a `Collection`, then `collection.store(&store).await? → TempTag` (the root **HashSeq**). *Even a single file is a 1-entry collection* (keeps the receive path uniform).
3. Register the handler with provider events for connect/request notifications:
   ```rust
   let blobs = iroh_blobs::BlobsProtocol::new(&store, Some(EventSender::new(tx, EventMask {
       connected: ConnectMode::Notify, get: provider::events::RequestMode::NotifyLog, ..EventMask::DEFAULT })));
   let router = iroh::protocol::Router::builder(endpoint.clone())
       .accept(iroh_blobs::ALPN, blobs.clone()).spawn();
   ```
   Provider events drive `Progress::PeerJoined` / sender-side `Transferring`.
4. Mint the ticket from the **(online) endpoint address**:
   ```rust
   let addr = router.endpoint().addr();                 // EndpointAddr
   let ticket = BlobTicket::new(addr, root_hash, BlobFormat::HashSeq);  // Display → "blob…"
   ```
   Emit `Progress::Ready { ticket }`. Serve until the transfer is cancelled or the app exits (we keep the
   `Router` alive in `AppState`, unlike sendme's Ctrl-C lifecycle).

### 5.5 Receive flow + resume (`core/src/receive.rs`)

```rust
let ticket: BlobTicket = ticket_str.parse()?;
let hf = ticket.hash_and_format();
let conn = endpoint.connect(ticket.addr().clone(), iroh_blobs::ALPN).await?;

// total size for the progress bar (sendme uses a 32 MiB cap on the hashseq fetch)
let (_hs, sizes) = get_hash_seq_and_sizes(&conn, &ticket.hash(), 1024*1024*32, None).await?;
let total: u64 = sizes.iter().sum();

// RESUME: request only what's missing
let local = store.remote().local(hf).await?;          // LocalInfo
if !local.is_complete() {
    let mut s = store.remote().execute_get(conn, local.missing()).stream(); // missing() = GetRequest for the gap
    while let Some(item) = s.next().await {
        match item {
            GetProgressItem::Progress(offset) => emit(Progress::Transferring { offset, total, via }),
            GetProgressItem::Done(_stats)     => break,
            GetProgressItem::Error(e)         => return Err(e.into()),
        }
    }
}

// export the collection tree to `dest`
let collection = Collection::load(ticket.hash(), store.as_ref()).await?;
for (name, hash) in collection.iter() {
    store.export_with_opts(ExportOptions { hash: *hash, target: dest.join(name), mode: ExportMode::Copy }).await?;
}
```

**Resume is explicit, not automatic.** If you request the full hash you re-download everything. The combination
`FsStore` (persists partials) + `local.missing()` (computes the gap) + `execute_get` is the whole mechanism.
This is the single behavior most likely to regress on a blobs version bump — it gets a dedicated test (§12).

### 5.6 The `via: Route` badge

`Progress::Transferring.via` should reflect whether the live connection is direct or relayed (a core selling
point and a cost signal). Source it from the connection/path info on the iroh `Connection` (the exact accessor
for path type on iroh 1.0 is in §13 to confirm); default to `Unknown` until verified rather than guessing.

---

## 6. Self-hosted infrastructure

Two small services. A relay is bandwidth-heavy / CPU-light; the DNS server is tiny. A 1–2 vCPU VPS each to start;
scale the **relay on bandwidth** (flat-egress host — Hetzner/OVH — per the cost analysis in the research).

### 6.1 Relay (`iroh-relay`, separate crate, `--features=server`)

```toml
# infra/iroh-relay.toml
enable_relay = true
enable_quic_addr_discovery = true
enable_metrics = true

[tls]
hostname   = ["relay.yourapp.example"]
cert_mode  = "LetsEncrypt"          # CertMode: Manual | LetsEncrypt | Reloading
prod_tls   = true
contact    = "ops@yourapp.example"

# Lock the relay to OUR app (invisible app-level auth; preserves "no user account").
# Prefer the env override IROH_RELAY_ACCESS_TOKEN in deploy over committing the secret.
access.shared_token = ["REPLACE_WITH_LONG_RANDOM_SECRET"]

[limits]
accept_conn_limit = 100.0
accept_conn_burst = 200
[limits.client.rx]
bytes_per_second = 1048576
max_burst_bytes  = 4194304
```

Run: `cargo run --features="server" --bin iroh-relay -- --config-path=infra/iroh-relay.toml`.
Ports: HTTP 80 (ACME challenge), HTTPS 443, QUIC UDP 9889, metrics 9090. Client side already sends the token via
`RelayConfig::with_auth_token` (§5.2), so third parties can't relay through us.

### 6.2 DNS / discovery (`iroh-dns-server`, pkarr)

```toml
# infra/iroh-dns-server.toml  (based on the shipped config.prod.toml)
pkarr_put_rate_limit = "smart"

[https]
port = 443
domains = ["dns.yourapp.example"]
cert_mode = "lets_encrypt"
letsencrypt_prod = true

[dns]
port = 53
origins = ["dns.yourapp.example", "."]   # MUST equal the client's origin_domain
rr_a  = "<server-ipv4>"
rr_ns = "ns1.dns.yourapp.example."

[mainline]
enabled = false   # PRIVATE network: do NOT bridge our discovery records to the public DHT
```

The client publishes to `https://dns.yourapp.example/pkarr` and resolves TXT at
`_iroh.<z32-endpoint-id>.dns.yourapp.example`. `PkarrPublisher` republishes periodically, so the endpoint must
stay bound to remain resolvable. Real NS/glue records must delegate the origin domain to this box on UDP/TCP 53.

> **Why both:** discovery (DNS/pkarr) lets two peers *find* each other by key; the relay is the *connection*
> fallback when hole-punching fails. You need both for the "it just works with no account" flow.

---

## 7. The Tauri v2 shell (`src-tauri`)

Kept deliberately thin. Owns the Tauri `Builder`, commands, state, and the mapping from `irohcore::Progress`
to a Tauri IPC `Channel` event.

### 7.1 State & lifecycle

```rust
// src-tauri/src/lib.rs
pub struct AppState {
    pub core: irohcore::Core,                                   // built once in setup
    pub transfers: tokio::sync::Mutex<HashMap<String, CancellationToken>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let core = tauri::async_runtime::block_on(irohcore::Core::start(load_config()))?;
            app.manage(AppState { core, transfers: Default::default() });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![start_send, start_receive, cancel_transfer])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- **Do not** create your own tokio runtime — Tauri already runs on tokio; build the endpoint inside the async
  context (`async_runtime::block_on` in `.setup`). A hand-rolled runtime fights Tauri's and panics ("no reactor").
- `[lib] crate-type = ["staticlib", "cdylib", "rlib"]` (cdylib/staticlib needed for the future mobile target;
  rlib so `main.rs` can call `app_lib::run()`).
- Async commands that borrow `State<'_, _>` **must** return `Result`.

### 7.2 Progress streaming — `Channel<T>`, not the event bus

Tauri's docs explicitly recommend `tauri::ipc::Channel<T>` for high-frequency/ordered data like transfer
progress (the global `emit`/`listen` bus can deliver out-of-order and isn't throughput-optimized). Reserve the
event bus for sparse lifecycle signals (e.g. an incoming-transfer toast) emitted where you only hold an `AppHandle`.

```rust
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "event", content = "data")]
enum TransferEvent {
    Importing { id: String, done: u64, total: u64 },
    Ready     { id: String, ticket: String },
    Progress  { id: String, offset: u64, total: u64, via: String },
    Finished  { id: String, bytes: u64, secs: f64 },
    Cancelled { id: String },
    Error     { id: String, message: String },
}

#[tauri::command]
async fn start_send(path: String, on_event: Channel<TransferEvent>, state: State<'_, AppState>)
    -> Result<String, String>
{
    let (id, mut stream) = state.core.send(path.into()).await.map_err(|e| e.to_string())?;
    let token = CancellationToken::new();
    state.transfers.lock().await.insert(id.to_string(), token.clone());
    let ch = on_event.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = token.cancelled() => { let _ = ch.send(TransferEvent::Cancelled { id: id.to_string() }); break; }
                item = stream.next() => match item {
                    Some(p) => { let _ = ch.send(map_progress(p)); }   // irohcore::Progress → TransferEvent
                    None => break,
                }
            }
        }
    });
    Ok(id.to_string())
}

#[tauri::command]
async fn cancel_transfer(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(t) = state.transfers.lock().await.remove(&id) { t.cancel(); }
    state.core.cancel(/* parse id */).await;
    Ok(())
}
```

Frontend:

```ts
import { invoke, Channel } from '@tauri-apps/api/core';
const ch = new Channel<TransferEvent>();
ch.onmessage = (m) => {
  if (m.event === 'ready')    showCodeAndQR(m.data.ticket);
  if (m.event === 'progress') updateBar(m.data.offset, m.data.total, m.data.via);
};
const id: string = await invoke('start_send', { path, onEvent: ch });
// later: await invoke('cancel_transfer', { id });
```

### 7.3 Input: pickers + drag-drop

- **Pickers** (`tauri-plugin-dialog`): `open({ multiple, directory, filters })` to choose what to send;
  `save({ defaultPath })` to choose where received files land. Needs the plugin registered **and**
  `dialog:allow-open` / `dialog:allow-save` in `capabilities/default.json` (missing capability → runtime error).
- **Drag-drop**: `getCurrentWebview().onDragDropEvent(e => …)`; on `'drop'`, `e.payload.paths` are **absolute
  filesystem paths** — feed straight to `start_send`. Tauri intercepts OS drops by default (`dragDropEnabled:true`),
  which is what gives us real paths (browser HTML5 DnD would not).
- **Never** ship file bytes over IPC. Received data is written to disk in Rust (`export_with_opts`); only small
  progress structs cross the Channel.

---

## 8. Frontend / UX (web tech in the webview)

Framework is the one open product choice (Svelte/React/Vue/vanilla — pick for team familiarity; all work with
Tauri). Screens for the MVP:

- **Home:** two big targets — "Send" (drop zone + pick button) and "Receive" (code entry / QR scan).
- **Sending:** importing progress → then the **shareable code + QR** prominently, with copy button and a live
  "waiting for receiver / receiving" state. The code is the `BlobTicket` rendered as a short, copyable string;
  the QR encodes the same. (Do **not** show the raw 52-char base32 as the primary affordance.)
- **Receiving:** code/QR input → connecting → progress bar with **direct/relayed badge** and ETA (we know `total`
  from `get_hash_seq_and_sizes`, and running `offset` from progress) → done, with "open folder".
- **Settings (minimal):** default download folder, this device's identity, infra toggle (self-hosted/dev).

UX references validated in research: LocalSend (polish), `sendme`/ARK Drop/DataBeam (the iroh flow).

---

## 9. End-to-end sequence

```
SENDER                                   INFRA                         RECEIVER
  pick/drop path                                                         
  import → Collection → HashSeq tag                                      
  Router.accept(blobs ALPN).spawn()                                      
  endpoint.online()  ───────────────►  publish addr (pkarr/DNS)         
  mint BlobTicket  → show code + QR                                      
                                          ◄──────────── enter code, parse ticket
                                       resolve sender addr (DNS) ◄────── endpoint.connect(addr, blobs ALPN)
        ◄═══════ QUIC connect: hole-punch (≈90%) OR relay fallback ═════►
                                                          get_hash_seq_and_sizes → total
                                                          remote.local() → missing()
        ═══════ BLAKE3-verified blob stream (execute_get) ═════════════► offset… offset… done
                                                          export collection → dest folder
```

---

## 10. Persistence & resume strategy

`sendme` is a one-shot CLI: temp store per run, wiped on clean exit. A GUI must do the opposite.

- **Persistent `FsStore`** at `data_dir/blobs` (not per-transfer temp dirs) so partials survive app restart.
- **Persistent identity** at `data_dir/node.key`.
- **A small `transfers.db`** (e.g. SQLite/redb) cataloguing in-flight and recent transfers: `id`, direction,
  ticket/hash, dest path, total bytes, status, timestamps. This is what lets the UI show a transfer list and
  **offer "resume" after a crash/restart** (look up the ticket, re-`receive` into the same dest — `local.missing()`
  picks up where it left off because the blob data is still in `FsStore`).
- **Resume correctness caveat:** the docs describe intended behavior; on the 0.10x line resume *must be proven
  empirically* on the pinned version (§12). The blobs DESIGN notes flag an unresolved fsync/bitfield-consistency
  question, so a crash mid-write is exactly the case to test.

---

## 11. Security model

- **E2E encryption** via QUIC/TLS 1.3 (rustls). The relay forwards encrypted datagrams only — it **cannot read
  payloads or MITM** (that would require both peers' private keys). Verified.
- **Trust boundary = the ticket.** Whoever holds the ticket can fetch the content. Treat the code like a password:
  short-lived, shown only to the intended recipient. (Future: optional per-transfer PIN / one-time tickets.)
- **App-locked relay** via shared token keeps our relay from becoming an open proxy; it is **not** user auth and
  isn't secret-grade (it ships in the binary) — rotate via the server's multi-token list.
- **Private discovery:** `mainline.enabled = false` keeps node records off the public DHT.
- **Known gap:** no published third-party audit of iroh's own handshake/relay code yet (upstream QUIC/TLS is
  well-vetted). Fine for a consumer MVP; commission an audit before the enterprise tier.

---

## 12. Testing strategy

- **Unit (core):** ticket round-trip; identity load-or-generate; collection build/load for single-file and nested
  dirs; name normalization (no path traversal on export).
- **Integration (the important one):** spin two `Core`s in one test, `send` a multi-MB folder, **kill the receiver
  mid-stream, restart, assert `local.missing()` shrank and the second run completes with matching hashes.** This
  guards the #1 regression risk across blobs bumps.
- **Infra:** round-trip a relay/dns config through the actual binaries (validates TOML field names — several are
  inferred, see §13); verify a client with the wrong token is rejected; verify resolution via the self-hosted DNS.
- **Network matrix (Phase 0 pilot):** measure **real direct-vs-relay rate and throughput** across home / mobile
  CGNAT / corporate networks before trusting the cost model. Log the `via` route per transfer.
- **E2E (Tauri):** drive the app (WebDriver/`tauri-driver`) for pick → code → receive → open-folder.

---

## 13. Verify-before-coding

Confirmed names that still need a final check against the pinned docs (don't guess — read `docs.rs` at impl time):

1. **`RelayMap` / `RelayConfig` construction** for a custom relay (`from_iter` vs `try_from_iter`, and whether
   `RelayConfig::from(RelayUrl)` defaults QUIC on — needed for hole-punching through our relay).
2. **`address_lookup` builder finalization** — whether publisher + resolver are two separate `.address_lookup()`
   calls (as written) and the exact `AddressLookupBuilder` `.build()` contract; whether `presets::Minimal` (vs
   `Empty`) is the right fully-self-hosted base.
3. **`BlobsProtocol::new` second arg** (`EventSender`) — exact `EventMask` / `ProviderMessage` variants for
   sender-side per-connection progress on 0.103.
4. **`Connection` path-type accessor** for the `Direct`/`Relayed` badge (§5.6).
5. **blobs 0.103 progress variant names** (`AddProgressItem`, `GetProgressItem`, `ExportProgressItem`) and the
   `.stream()` signatures — the rewrite changed these.
6. **`ImportMode::TryReference` semantics on Windows** (reflink/hardlink fallback) for the picker flow.
7. **Relay/DNS TOML exact keys** (`[limits.client.rx]` shape; dns `cert_mode` values `self_signed|lets_encrypt`)
   and the **correct `iroh-relay` / `iroh-dns-server` versions/git-revs** that pair with iroh 1.0.0.
8. **`AccessConfig::Http`** request/response contract (only if we later want an external auth service).

(Full list with sources in [`_spec_specifics.md`](_spec_specifics.md) → "Open Items".)

---

## 14. Out of scope / roadmap

This doc covers **desktop only**. Deferred, with the reasons captured in research:

- **Android** — same core via JNI/UniFFI; transfers under a *user-initiated-data-transfer* WorkManager job
  (not `dataSync` FGS, which Android 15 caps at 6h).
- **iOS** — foreground + screen-on + resume; background is hard (suspension reclaims sockets); Network Extension
  only as a later power-user path.
- **Web** — relay-only WASM tier (no hole-punch; in-browser resume currently broken — memstore-only); position as
  a lightweight/"download the app" funnel, and capacity-plan relays for ~100%-relayed web bytes.
- **Multi-provider swarming** (`Store::downloader` + `ContentDiscovery`), offline inbox/push, accounts/contacts,
  enterprise key lifecycle + audit + multi-region relay scaling.

---

## 15. Pinned dependencies

```toml
# core/Cargo.toml — the ONLY crate importing iroh/iroh-blobs
[dependencies]
iroh        = "=1.0.0"
iroh-blobs  = "=0.103.0"     # pre-1.0; wrapped behind irohcore. Expect breaks within 0.10x.
iroh-mainline-address-lookup = "0.4.0"   # serverless DHT discovery (default Infra::Decentralized)
n0-future   = "0.3"
tokio       = { version = "1", features = ["full"] }
tokio-util  = "0.7"          # CancellationToken
futures-lite = "2"
serde       = { version = "1", features = ["derive"] }
thiserror   = "2"
walkdir     = "2"
# iroh-relay / iroh-dns-server: pin the git rev matching iroh 1.0 (see §13.7), build in CI.

# src-tauri/Cargo.toml
[dependencies]
tauri               = { version = "2", features = [] }
tauri-plugin-dialog = "2"
irohcore            = { path = "../core" }
serde               = { version = "1", features = ["derive"] }
tokio               = { version = "1", features = ["full"] }
tokio-util          = "0.7"
```
