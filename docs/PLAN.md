# Dropwire — Metadata Control-Plane Implementation Plan

> Status: **implementation plan**, 2026-06-17. Companion to `ARCHITECTURE.md` (§5.7 is the design seed for this
> document) and `docs/DESIGN.md` (the motion/a11y/trust-visual contract). This plan turns §5.7's "designed, not
> yet implemented" capability into a sequenced, test-first build. **Build it in milestone order; every feature is
> specified test-first.**

## 0. The control-plane vision

Dropwire already does one thing: **move a BLAKE3-verified blob over a QUIC connection**. Everything in this plan is
a *control plane* clipped onto that one primitive — no new transport, no third party, no server. Two mechanisms
carry all of it:

1. **The bound manifest (in-band, BLAKE3-committed).** `send` prepends a Dropwire-owned `".dropwire/manifest.json"`
   as **the first child (child 0)** of the iroh-blobs `Collection`. Because the Collection's HashSeq root is what
   the ticket hash commits to, the manifest — and every file name/size/hash — is **cryptographically bound to the
   ticket**. The receiver fetches *only* child 0 first, renders a preview, and accepts before a single payload byte
   lands. A malicious sender cannot show one manifest and deliver different bytes.

2. **The control stream (out-of-band, custom ALPN on the SAME endpoint).** A second ALPN (`dropwire/ctrl/1`)
   registered additively on the existing `Router` (`Router::builder(ep).accept(BLOBS_ALPN, blobs).accept(CTRL_ALPN,
   ctrl).spawn()`, verified additive in the crate's `custom-protocol.rs`) carries two-way messages: presence,
   instant decline/ack, chat-lite, request-to-send. **The medium is the user's own machine** — this channel is free,
   and reading file content for previews is fine (the receiver already holds the ticket). We therefore rank features
   by **human value + on-device feasibility**, never by imagined server cost.

**The trust invariant that governs the whole UI** (from the UX-system R&D, applied identically everywhere): every
fact the user sees is rendered as either a **BOUND FACT** (derived by the *engine* from the HashSeq or from iroh's
cryptographic `EndpointId` — file names, sizes, count, total, content hashes, route, peer fingerprint → lime
"verified" lock chip) or a **SENDER CLAIM** (manifest free-text `from`/`note`, inferred file-kind icons, chat →
muted, italic, walled in a "They say" container, always plain text, never `innerHTML`). The **sender never controls
which tier a field lands in** — the engine, not the manifest, decides — so a claim can never be made to *look* bound.
This is the product's integrity made visible, and it is enforced at the type/token level.

### Locked principles (never violated by any feature here)
- No accounts, no Dropwire-run servers (serverless: Mainline DHT + n0 relay fallback), privacy-first / no telemetry,
  **one-to-one** transfers, free, open source, desktop-first.
- `iroh`/`iroh-blobs` symbols never appear above the `irohcore` boundary. The control plane lives in `core/` behind
  plain types; `src-tauri` maps them to DTOs; `ui/` is plain HTML/CSS/JS over the preserved DOM/command contract.
- Reduced-motion and screen-reader parity are the floor, not a fallback: the wire viz is `aria-hidden`; all truth
  lives in text + the single `aria-live` status line + the percentage.

---

## Build order & dependencies

The spine first (it is what everything clips onto), then trust, then consent/one-to-one, then selectivity, then the
two-way control stream, then reach/devices.

| Milestone | Theme | Depends on | Why here |
|---|---|---|---|
| **M0** | Test harness + resume proof | — | Unblocks TDD for every later layer; proves the one empirical risk (byte-perfect resume on the pre-1.0 store) before building on it. |
| **M1** | Bound manifest + two-phase receive (`inspect`/`accept`) | M0 | The manifest is child 0 of the Collection; **all** preview/trust/selective features read it. Nothing downstream exists without it. |
| **M2** | Accept modal + trust ledger + live-status mirror (UI spine) | M1 | "See → trust → accept → transfer → resume" is the core experience; it renders M1's bound facts and is the surface every later feature extends. |
| **M3** | Symmetric consent + real one-to-one enforcement (sender accept gate) | M1 (gate sees the manifest request) | Turns the always-on server into a gated, genuinely one-to-one channel; mirrors the receiver's accept on the sender side. |
| **M4** | Selective per-file/folder download + pre-flight (space/overwrite/exec warnings) | M1, M2 | Builds directly on the manifest's file list and the accept modal's file-tree picker; uses hand-built `GetRequest`s. |
| **M5** | Control stream: presence, instant decline/ack, chat-lite, request-to-send | M2 (lifecycle UI), M3 (consent grammar) | The free two-way channel; needs the lifecycle UI and consent language already in place to slot into the mirror. |
| **M6** | Reach & devices: device/peer identity chip, History (extended) + Resume affordance, dead-state cards, route badge polish | M2, M5 | The trust grammar persists post-transfer; resume (proven in M0) becomes a user-facing affordance; failures become on-brand cards. |

**Critical path:** M0 → M1 → M2 → M3 → (M4 ∥ M5) → M6. M4 and M5 are independent once M2/M3 land and may proceed in
parallel.

---

## Testing strategy (the harness that every milestone uses)

**Layers (a feature is "done" when it is green at every layer it touches):**

1. **Core unit (`core/src/*.rs` `#[cfg(test)]`)** — pure logic with no network: manifest (de)serialization & schema
   versioning, name normalization / path-traversal sanitizer, claim sanitization, selection→`GetRequest` mapping,
   space arithmetic, control-message (de)serialization. Fast, deterministic, run on every commit.
2. **Core integration (`core/tests/transfer.rs` + new `core/tests/control_plane.rs`)** — two `Core`s in one test
   over `Infra::LocalOnly` (loopback, hermetic — the existing pattern), driving real `inspect`/`accept`/gate/control
   flows end to end. This is where the manifest binding, the gate-blocks-bytes guarantee, selective download, and
   resume are proven against the *actual* iroh-blobs 0.103 behavior.
3. **Shell (`src-tauri`)** — command/DTO mapping tests: `Progress`/`TransferPreview` → DTO is total and lossless,
   extension→kind/icon derivation lives here (not in the engine), `accept` is gated behind a valid preview id.
4. **UI (plain JS, headless)** — DOM-contract + a11y tests with the existing harness: the accept modal renders bound
   vs claim tiers with the correct chips/`aria` names, `note` is never injected as HTML, focus-trap + Esc=decline,
   live-status `aria-live` announces the mirrored lifecycle, reduced-motion path applies end-states directly.
5. **E2E (Tauri driver, `#[ignore]` in CI by default)** — full pick → code → preview → accept → receive → open-folder.

**Test-first rule:** for each feature below, the **TDD** subsection lists the *failing* tests to write first, by
layer. Write the red test, then the implementation, then green. The harness has a shared helper module
(`core/tests/common.rs`, new) factoring out `make_payload`, `wait_for(predicate)`, `two_cores()`, and
`drive_to_completion()` so control-plane tests stay terse.

**Resume proof & CI:** the kill-mid-transfer resume test (M0) is the #1 regression guard across blobs version bumps.
It runs in CI on a tuned cancel-delay (M0 makes it deterministic by gating on bytes-received rather than wall-clock).
Networked tests (`roundtrip_serverless`) stay `#[ignore]`. A `cargo test -p irohcore` gate plus `cargo clippy
-D warnings` plus the JS DOM/a11y suite gate every PR.

---

## M0 — Test harness + resume proof

### Feature — Deterministic two-Core test harness
**What & Why.** A shared harness so every control-plane test is terse, hermetic, and deterministic. User benefit is
indirect but decisive: it is what lets us ship the trust features *correctly* and keep them correct across the
volatile blobs dependency.

**How it is built.**
- New `core/tests/common.rs` (a `mod common;` shared by integration test files) exposing: `two_cores() -> (Core,
  Core)` (both `Infra::LocalOnly`, tempdir-backed); `make_payload(n)` (moved from `transfer.rs`); `async
  wait_for(stream, pred)` with a timeout; `async drive_to_completion(stream)`; and `async wait_event(stream,
  matcher)` returning the matched event.
- No production-code change in M0 except optionally splitting `make_payload`/helpers out of `transfer.rs`.

**TDD (write first).**
- *Core integration:* port the two existing happy-path tests (`roundtrip_single_file`, `roundtrip_folder`) to use
  `common::two_cores()` — they must stay green (regression net for the refactor).

### Feature — Byte-perfect resume, proven and CI-gated
**What & Why.** The one open empirical item: prove resume is byte-perfect on the pre-1.0 `FsStore` via a
kill-mid-transfer test, *deterministically* (the current `resume_after_interrupt` is `#[ignore]` because it cancels
on a 40 ms wall-clock guess). User benefit: the "[Resume] picks up where it left off" promise is real and stays real.

**How it is built.**
- Make the interruption deterministic: in the test, drive the receive stream until a `Progress::Transferring` with
  `offset > 0 && offset < total/2` is observed, *then* `cancel`. This replaces the timing guess with a content
  condition, so it is stable in CI.
- Assert the mechanism, not just the result: after the first (cancelled) attempt, query the store and assert
  `store.remote().local(hf).await?.missing()` requests **strictly fewer** chunks than the full request (i.e. the
  partial survived in `FsStore`); after the second attempt assert `is_complete()` and byte-equality with source.
- Exact APIs (verified in 0.103): `store.remote().local(hf) -> LocalInfo`, `LocalInfo::is_complete()`,
  `LocalInfo::missing() -> GetRequest`, `store.remote().execute_get(conn, request)`.

**TDD (write first).**
- *Core integration (`resume_after_interrupt`, un-`ignore`d):* (1) interrupt deterministically at first partial
  progress; (2) assert `missing()` shrank between attempts (proves `FsStore` retained partial chunks); (3) assert the
  resumed download completes and bytes match. Mark CI-required.
- *Core integration:* `resume_is_noop_when_complete` — receiving an already-complete ticket emits `Done` with no
  `Transferring` (guards the `is_complete()` short-circuit).

---

## M1 — The bound manifest + two-phase receive

This is the foundation: it changes what `send` puts on the wire and splits `receive` into `inspect` (cheap, no
payload) and `accept` (the existing download), with the manifest as the BLAKE3-bound first child.

### Feature — The `.dropwire/manifest.json` v1 schema (bound first child)
**What & Why.** A Dropwire-owned, versioned manifest describing the transfer, carried *inside* the transfer so it is
atomic, resumable, and cryptographically bound. User benefit: the receiver can see exactly what they're getting, and
the facts among that description are un-spoofable.

**How it is built (layers + manifest fields).**
- New module `core/src/manifest.rs`. `#[derive(Serialize, Deserialize)] pub struct Manifest` with the §5.7 schema,
  hardened:
  ```jsonc
  {
    "v": 1,                       // schema version — receiver rejects unknown major versions
    "from": "Keon's Laptop",      // CLAIM (display only)
    "note": "vacation photos",    // CLAIM (display only, plain text)
    "created_at": 1718539200,
    "count": 2,
    "total_bytes": 86046412,
    "files": [ { "name": "IMG_2391.jpg", "size": 4823002 },
               { "name": "clip.mov",     "size": 81223410 } ]
  }
  ```
  `from`/`note` are `Option<String>` (the optional "Add a note" field, SEND-1). `name` uses forward-slash relative
  paths (the existing `collect_files` convention) so a folder tree renders later (M4).
- `Manifest::parse(bytes) -> Result<Manifest>` rejects: unknown `v`, non-UTF-8, `note`/`from` over a length cap, and
  any control characters. It does **not** trust `files[].name`/`size` as authority — those are *also* present, but
  the receiver derives the **bound** names/sizes from the HashSeq (`get_hash_seq_and_sizes` + `Collection::load`) and
  uses the manifest copy only to detect a mismatch (a malformed/lying sender → `manifest-invalid` dead state).

**TDD (write first).**
- *Core unit:* `manifest_roundtrip_v1` (serialize→parse identity); `manifest_rejects_future_version` (`v:2` →
  `Err`); `manifest_rejects_oversized_note`; `manifest_strips_control_chars`; `manifest_optional_fields_absent`
  (no `from`/`note` parses).

### Feature — `send` prepends the manifest as child 0
**What & Why.** Bind the manifest to the ticket. User benefit: the preview the receiver sees is provably the content
they will get.

**How it is built.**
- In `core/src/send.rs::run_send`, after enumerating files but before building the Collection: build a `Manifest`
  from the file list (names = the bound relative names, sizes = `file_len`, `total_bytes`, `count`, `created_at`,
  optional `from`/`note` passed via a new `SendOptions`), serialize, and **import it as a blob** the same way as a
  file: `store.add_bytes(...)` / `add_path_with_opts` on a temp file → `TempTag`. (`add_bytes` exists on the store
  api; hold its `TempTag` alongside the file tags so it isn't GC'd.)
- Insert it as **the first entry** of the Collection: `entries.insert(0, (".dropwire/manifest.json", manifest_hash))`
  before `let collection: Collection = entries.into_iter().collect()`. The Collection preserves insertion order
  (verified: `Collection` is `Vec<(String, Hash)>`), so the manifest becomes child index 0; on the wire it is HashSeq
  offset 1 (offset 0 is the HashSeq root/metadata).
- `Core::send` gains a sibling `Core::send_with(path, SendOptions { from, note })`; `send` delegates with `None`s so
  the existing signature/tests keep working.

**TDD (write first).**
- *Core integration:* `manifest_is_child_zero` — `send` a folder, on the receiver connect + `Collection::load(root)`
  and assert `collection.iter().next()` is `(".dropwire/manifest.json", _)` and the remaining entries match the sent
  files in order.
- *Core integration:* `manifest_is_bound_to_ticket` — fetch only child 0 via
  `GetRequest::builder().root(ChunkRanges::all()).child(0, ChunkRanges::all()).build(root)` through
  `execute_get`/`get_blob`, parse it, and assert its `files` names/sizes equal the HashSeq-derived names/sizes
  (binding holds).
- *Core integration:* `existing_roundtrips_still_pass` — `roundtrip_single_file`/`roundtrip_folder` still export only
  the real files to `dest` (the manifest child must be **skipped** on export — see next feature).

### Feature — Two-phase receive: `Core::inspect` then `Core::accept`
**What & Why.** Nothing downloads before the receiver sees the manifest and accepts. User benefit: "show before you
commit," honestly disclosed as a real connection (RECV-2).

**How it is built (API surface + exact iroh APIs).**
- New public types in `core/src/progress.rs` (plain, no iroh types): `PreviewId(Uuid)`; `FilePreview { name: String,
  size: u64 }`; `TransferPreview { files: Vec<FilePreview>, count: u64, total: u64, route: Route, from:
  Option<String>, note: Option<String>, sender_fingerprint: String }`. New `Progress::Manifest { id, preview:
  TransferPreview }`.
- `Core::inspect(ticket: String) -> Result<(PreviewId, TransferPreview)>`:
  1. parse `BlobTicket`; `endpoint.connect(ticket.addr(), BLOBS_ALPN)` (this is the *same* connect today's `receive`
     does — the honesty beat: it is a real dial, surfaced not faked).
  2. `get_hash_seq_and_sizes(&conn, &hash, 32<<20, None)` → `(hash_seq, sizes)` (already used in `receive`). The
     **bound facts**: `files[i].name` from `Collection`'s names, `files[i].size = sizes[i+1]` (off-by-one:
     `sizes[0]`/`hash_seq[0]` are the *manifest* blob; zip names with `sizes[1..]`), `count = files.len()`, `total =
     sizes[1..].sum()` (payload only — exclude the manifest from the user-facing total).
  3. fetch child 0: `get_blob(conn.clone(), hash_seq[0]).await` → `Manifest::parse` for the **claims** (`from`,
     `note`). On parse failure → `CoreError` mapped to the `manifest-invalid` dead state.
  4. `sender_fingerprint = short(conn.remote_id())` — the peer `EndpointId` (BOUND, cryptographic; `remote_id()`
     verified on `Connection`).
  5. **cache the live `Connection`** in a new `inner.previews: Mutex<HashMap<PreviewId, PreviewState>>` (holds the
     `conn`, the parsed manifest, the bound file list, an `Instant` for TTL). **No export.** Return the preview.
- `Core::accept(preview_id: PreviewId, selection: Option<Vec<usize>>, dest: PathBuf) -> Result<(TransferId,
  ProgressStream)>`: pull the cached `PreviewState`, **reuse `conn`** (no reconnect — the cached-connection promise,
  RECV-4), then run the existing resume/download/export, with two changes: (a) `selection` is the M4 hook (`None` =
  all); (b) **skip the manifest child on export** (don't write `.dropwire/manifest.json` to `dest`).
- A cached preview that is never accepted is dropped (and its `conn` closed) after a TTL sweep; `inspect` returning a
  preview also arms a timeout so a hung sender surfaces as `sender-unreachable`.
- `core/src/receive.rs::receive` is reimplemented as `inspect` + auto-`accept(None)` so the old one-shot path and its
  tests keep working.

**TDD (write first).**
- *Core integration:* `inspect_returns_bound_facts_no_payload` — after `inspect`, assert `preview.files`/`count`/
  `total` match the sent files, and assert **no payload child is present locally** (`store.remote().local(hf).missing()`
  still requests the file children — proving inspect didn't download bodies).
- *Core integration:* `inspect_size_offset_by_one` — a 1-file send; assert `preview.total == file_size` (not
  `file_size + manifest_size`), guarding the `sizes[1..]` off-by-one.
- *Core integration:* `accept_reuses_connection_and_completes` — `inspect` then `accept(None)`; assert `Done` and
  byte-equality, and that no second `connect` happened (e.g. instrument a connect counter or assert via timing/log).
- *Core integration:* `accept_skips_manifest_on_export` — `dest` contains the real files and **not**
  `.dropwire/manifest.json`.
- *Core integration:* `inspect_surfaces_manifest_claims` — `send_with(from, note)`; assert `preview.from`/`note`
  carry the claims verbatim.
- *Core unit:* `preview_total_excludes_manifest`.

### Feature — Live status from pull events (the happy-path mirror, no back-channel)
**What & Why.** Both sides see a plain-words lifecycle (Connected → previewing → accepted, transferring → done)
driven entirely by existing provider pull-events. User benefit: the SEND-2/SEND-3 status strip and the receiver's
RECV-3/RECV-4 line are truthful and free.

**How it is built.**
- Sender side already receives `ProviderMessage::GetRequestReceivedNotify` per request (see `send.rs`). Distinguish
  the **manifest pull** from a **payload pull** by inspecting `msg.request.ranges`: a request that only touches the
  root + child 0 is "previewing"; a request touching file children is "accepted, sending". Add a
  `ProviderEvent::Previewing` variant and emit it on the manifest pull → a new `Progress` signal the UI maps to
  "Someone connected — they're previewing."
- Receiver side maps `inspect` → "Connecting to preview…/Previewing", `accept` → "Accepted, receiving" using the
  existing `Progress` stream. No new wire protocol.
- This is the §5.7 status table, implemented; the *explicit* Decline (so the sender hears "no" instantly) is the only
  part that wants a real back-channel and is deferred to M5.

**TDD (write first).**
- *Core integration:* `sender_sees_previewing_then_sending` — receiver `inspect`s only (no accept); assert the sender
  stream emits the previewing signal and **not** a completed transfer; then `accept`, assert sender then sees
  transferring → done.
- *Core unit:* `classify_request_manifest_vs_payload` — given a `ChunkRangesSeq` for {root+child0} vs {file children},
  the classifier returns Previewing vs Sending.

---

## M2 — Accept modal, trust ledger, live-status mirror (the UI spine)

The single most important new screen and the surface every later feature extends. Built in `ui/` over the preserved
DOM/command contract, mapped through `src-tauri` DTOs.

### Feature — `inspect_ticket` / gated `start_receive` shell commands + DTOs
**What & Why.** Expose two-phase receive to the UI without leaking iroh types. User benefit: enables the whole
preview-before-download flow.

**How it is built.**
- `src-tauri`: `#[tauri::command] async fn inspect_ticket(ticket: String, state) -> Result<TransferPreviewDto,
  String>` calling `core.inspect`. `TransferPreviewDto` adds **shell-derived, claim-tagged** fields: `kind`/`icon`
  per file inferred *from extension here* (never in the engine — it's a claim), and an `is_executable` flag per file
  (for the M4 warning). Bound fields (name, size, count, total, route, `sender_fingerprint`) pass through unchanged.
- `start_receive` becomes `accept_transfer(preview_id, selection: Option<Vec<usize>>, dest, on_event: Channel<...>)`
  — gated behind a valid `preview_id`; an unknown/expired id returns an error mapped to the `sender-unreachable`/
  expired dead state.

**TDD (write first).**
- *Shell:* `dto_is_lossless` — every `TransferPreview` field appears in `TransferPreviewDto`; bound fields are
  byte-identical.
- *Shell:* `kind_derivation_is_claim_tier` — `.exe`→executable flag set, `.jpg`→image kind, and the derived
  kind/icon are marked claim-tier in the DTO (a field the UI keys on to choose styling).
- *Shell:* `accept_requires_valid_preview_id` — unknown id → `Err`.

### Feature — The Accept modal (the trust pivot)
**What & Why.** The sheet between code-entry and download where the receiver decides with full information. User
benefit: RECV-3 — the consent moment, with the file list, route, sender, and the safety pre-flight all visible
before any byte.

**How it is built (UI layer + DESIGN.md motion).**
- New modal in `ui/index.html`/`app.js`/`app.css`, opened by `Preview` (RECV-1, renamed from "Receive"). Sequence:
  press `Preview` → `inspect_ticket` (RECV-2 honesty copy: "Connecting to sender to preview… (this reaches their
  device — they must be online)") → on success the modal springs in.
- States: opening · loaded · selecting (M4) · warning (M4) · accepting (collapses to the active wire, conn reused) ·
  declining (M5) · error. Actions: **[Accept & download]** (primary, **last in tab order**), **[Decline]** (M5
  back-channel; pre-M5 it just dismisses + closes the cached preview), **[Cancel]**.
- Motion (DESIGN §4.3 sibling): sheet springs up with `--spring-pop` over `--scrim`; file rows stagger in a 40 ms
  cascade (max 6 animated, rest snap); trust-ledger fact chips settle **last** (the eye lands on "verified" before
  "they say"). Decline exits downward with `--ease-in` (receive grammar). All WAAPI gated behind the existing
  `RM`/`canAnim` guards; reduced-motion path = a single `--dur-2` opacity fade, decline dismisses instantly.
- A11y: focus-trapped (Tab cycles within; focus returns to `[Preview]` on close); Esc = Decline/Cancel; **first focus
  on the file list**, primary action last; `--glow-focus` on all controls; ≥40×40 targets.

**TDD (write first, UI/JS headless).**
- `modal_opens_on_inspect_success` and `modal_shows_error_on_inspect_failure`.
- `focus_trap_and_esc` — Tab stays within; Esc invokes decline/cancel; focus returns to `[Preview]` on close.
- `primary_action_is_last_in_tab_order` and `first_focus_is_file_list`.
- `reduced_motion_applies_end_state` — with RM on, the sheet has final opacity/transform immediately (no spring).

### Feature — The Trust Ledger (bound vs claim, un-spoofable)
**What & Why.** The un-spoofable separation of BOUND FACTS from SENDER CLAIMS — the product's integrity made visible.
User benefit: learn the grammar once (modal), trust it everywhere (header, History, sender card, chat).

**How it is built.**
- A `ui` component used in the modal, the sender Allow card (M3), and the active-transfer header. Two tiers, styled
  from **distinct CSS tokens** so the tiers can never share styling:
  - **Bound:** full `--text`, normal/strong weight, a 10px lime lock chip (reuse the wire-node dot motif) labeled
    "verified". Fields: count, total, each file name+size, content-hash on demand (click a row → reveal short BLAKE3,
    monospace, copyable), route, peer fingerprint.
  - **Claim:** `--text-muted`, italic, walled in a "They say" container one surface step down with a left hairline in
    `--text-faint` (NOT lime) and a quote-glyph "claim" chip. Fields: `from`, `note`, inferred kind icons.
- **Enforcement:** the JS that builds the ledger takes a *typed* preview (bound fields vs claim fields are separate
  object keys coming from the DTO); claim fields are rendered with `textContent` only (never `innerHTML`), and the
  lime bound-chip class is applied *only* to the bound-keys renderer. A test asserts the manifest's free-text can
  never reach the bound renderer.
- A11y: tier is in the **accessible name**, not color: bound → "verified: 2 files, 82.1 megabytes"; claim →
  "unverified label from sender: vacation photos". Color is never the only signal (every chip pairs color + text).

**TDD (write first, UI/JS).**
- `bound_facts_get_lock_chip_and_aria` — names/sizes/count/total/route/fingerprint render with the lime chip and
  "verified" accessible name.
- `claims_are_walled_and_muted` — `from`/`note` render inside "They say", italic/muted, with the claim chip and the
  "unverified label" accessible name.
- `note_is_never_html` — a `note` of `<img src=x onerror=alert(1)>` renders as literal text; no element is created.
- `claim_cannot_reach_bound_renderer` — feeding a claim field through the bound renderer is impossible by construction
  (the function only accepts bound keys); covered by a type/contract test.
- `hash_inspect_reveals_short_blake3` — clicking a file row reveals a copyable short hash.

### Feature — Live-status line (the mirrored aria-live anchor)
**What & Why.** One plain-words source of lifecycle truth, mirrored on both sides. User benefit: SEND-2/SEND-4 strip
and RECV-2/RECV-4 line; the screen-reader anchor (wire viz is decorative).

**How it is built.**
- A single `aria-live="polite"` element (errors/inbound-allow switch to `assertive`). Send states: Ready → Connected
  (previewing) → Accepted, sending → Sent ✓ / Declined / Interrupted. Recv states: Connecting to preview →
  Previewing → Accepted, receiving → Received ✓ / Couldn't reach them / Interrupted. Each maps 1:1 to a wire visual
  state. Driven by the M1 `Progress` signals (previewing/transferring/done) + (M5) control-stream presence.
- The wire `<svg>` stays `aria-hidden="true"`. Current flows toward the peer on send, toward the local node on
  receive (direction grammar). Resume shows the already-have portion as a dim `--info` segment.

**TDD (write first, UI/JS).**
- `status_line_is_single_aria_live_anchor` and `wire_is_aria_hidden`.
- `send_lifecycle_announcements` / `recv_lifecycle_announcements` — feeding the mapped `Progress` events produces the
  exact mirrored strings.
- `errors_switch_to_assertive`.

---

## M3 — Symmetric consent + real one-to-one enforcement (sender accept gate)

Turns the always-on server into a genuinely gated, one-to-one channel, mirroring the receiver's accept on the sender
side. This is the §5.7 `RequestMode::NotifyLog → Intercept` upgrade, verified against the 0.103 source.

### Feature — Sender accept gate (Allow/Decline) that blocks bytes before serving
**What & Why.** The sender can consent too: when armed, an inbound Allow-request card appears (SEND-3) listing what
the peer wants + the peer's bound fingerprint; [Allow]/[Decline]. User benefit: the symmetric consent moment and real
one-to-one enforcement (no third party can pull your content even with the ticket).

**How it is built (exact iroh-blobs APIs, verified in 0.103 `src/provider/events.rs` + `examples/limit.rs`).**
- In `core/src/lib.rs::start`, change the `EventMask` from `get: RequestMode::NotifyLog` to `get:
  RequestMode::InterceptLog` (keep `connected: ConnectMode::Notify`; optionally `Intercept` for connect-time gating
  later). With `InterceptLog`, the provider sends `ProviderMessage::GetRequestReceived` (the **intercept** variant,
  `tx: oneshot::Sender<EventResult>`) and **blocks on `rx.await??` before serving any bytes** — the gate is enforced
  *inside the library*, exactly the `limit.rs` pattern.
- In `consume_provider_events`, handle the intercept variant: read `msg.request.hash` (correlate to our serving
  transfer), read `msg.request.ranges` (manifest-only vs payload). **Always allow the manifest pull** (`Ok(())`) so
  preview works without prompting. For a **payload** pull: if the transfer's gate is *off* → `Ok(())` (auto, current
  behavior). If *on* → surface a new `Progress::AcceptRequest { id, peer_fingerprint, files, total }` to the sender
  UI and **hold the `oneshot::Sender`**; on the user's choice reply `Ok(())` (allow) or
  `Err(AbortReason::Permission)` (decline). The receiver's `execute_get` then fails with a permission error → mapped
  to the `sender-declined-gate` dead state.
- One-to-one enforcement: once a payload pull is allowed for a transfer, deny subsequent *new-peer* payload pulls for
  the same content (`Err(AbortReason::Permission)`), keeping it strictly one recipient. (Resume by the *same* peer
  is still allowed — correlate by `connection_id`/`EndpointId`.)
- Config: `SendOptions.gate: GateMode { Auto, Ask }` (default `Ask` per the symmetric-consent principle; `Auto`
  preserves the frictionless path for users who prefer it).

**TDD (write first).**
- *Core integration:* `gate_blocks_payload_until_allowed` — sender in `Ask` mode; receiver `inspect`s (manifest pull
  succeeds — preview works) then `accept`s; assert no payload bytes arrive until the sender replies allow; then bytes
  flow and complete. (Proves the `rx.await??`-before-serving guarantee on 0.103.)
- *Core integration:* `gate_decline_propagates` — sender replies `Err(Permission)`; receiver's stream ends in an
  error mapped to `sender-declined-gate`; sender catalog marks declined; **no bytes** were sent.
- *Core integration:* `manifest_pull_is_never_gated` — preview always works regardless of gate mode.
- *Core integration:* `one_to_one_second_peer_denied` — after one peer is allowed, a *second* peer's payload pull is
  denied; the second peer's manifest pull still succeeds (preview is fine, download is not).
- *Core unit:* `gate_request_classifier` — manifest-only request → allow-unconditionally; payload request → gate.

### Feature — Sender Allow card (UI mirror of the accept modal)
**What & Why.** The sender's side of symmetric consent. User benefit: SEND-3 — "Allow this device to download?" with
the bound fingerprint, mirroring the receiver's modal in the same trust grammar.

**How it is built.**
- A card in `ui` rendered on `Progress::AcceptRequest`, reusing the **Trust Ledger** component: bound facts (what the
  peer wants — count/total/files + their fingerprint with the lime chip) and no claims (there are none here).
  [Allow]/[Decline] call a new `respond_accept(id, allow: bool)` shell command that resolves the held `oneshot`.
- Motion (DESIGN §4.4): the peer-node **ignites** *as* the card slides in — cause (they connected) and effect (you're
  asked) visually linked; reduced-motion cross-fades without the ignite spring. `aria-live="assertive"` (it demands a
  decision).

**TDD (write first, UI/JS).**
- `allow_card_renders_on_accept_request` with the peer fingerprint as a bound chip.
- `allow_card_is_assertive` and `allow_card_buttons_invoke_respond_accept`.
- `reduced_motion_allow_card_crossfades` (no ignite spring; end-state applied).

---

## M4 — Selective per-file/folder download + pre-flight

Builds on the manifest's file list (M1) and the accept modal's file-tree (M2), using hand-built `GetRequest`s.

### Feature — File-tree picker with selective download
**What & Why.** Receive only the files you want. User benefit: the modal's file-tree (RECV-3) with per-file/per-folder
checkboxes; total + free-space estimate recompute live.

**How it is built (exact iroh-blobs APIs, verified in 0.103 `src/protocol.rs`).**
- UI: the manifest's `a/b/c` names build a real tree (collapsible folders, indeterminate parent checkboxes,
  `aria-checked="mixed"`, roving tabindex, Space toggles). Default all-selected; header total + space estimate tween
  with tabular-nums (DESIGN). `selection` = the set of chosen child indices (0-based into the **payload** files,
  i.e. Collection children `1..N`; the manifest is child 0 and always implicitly fetched during inspect).
- Engine: `Core::accept(preview_id, selection: Some(indices), dest)` builds a **hand-built `GetRequest`** instead of
  `local.missing()` over the full set:
  ```rust
  let mut b = GetRequest::builder().root(ChunkRanges::all());     // HashSeq root (needed to map children)
  for &i in &indices { b = b.child(i + 1, ChunkRanges::all()); }  // child 0 = manifest; payload files are i+1
  let request = b.build(root_hash);
  let request = intersect_with_missing(request, store.remote().local(hf).await?); // still resume-aware
  store.remote().execute_get(conn, request)                       // accepts ANY hand-built GetRequest
  ```
  (Verified: `GetRequestBuilder::child(c, ranges)` maps to offset `c+1`; `execute_get` takes any `GetRequest`.) On
  export, write only the selected files. Resume of a selective download still works: `local()` reflects whatever
  partial chunks exist for those children.

**TDD (write first).**
- *Core integration:* `selective_download_fetches_only_selected` — `send` a 3-file folder; `accept(Some([0,2]))`;
  assert `dest` has files 0 and 2 only, and `store.remote().local(hf).missing()` still reports file 1 as absent
  (proves we didn't over-fetch).
- *Core integration:* `selective_download_is_blake3_verified` — selected files' bytes match source (BAO-verified
  range fetch).
- *Core integration:* `selective_resume` — interrupt a selective download mid-file, resume, assert completion of only
  the selected set.
- *Core unit:* `selection_maps_child_index_plus_one` — `[0,2]` → builder offsets `{1,3}` (guards the manifest
  off-by-one).
- *UI:* `tree_renders_from_paths`; `parent_checkbox_indeterminate`; `total_recomputes_on_select` (tabular-nums, no
  reflow); `none_selected_disables_primary`; `roving_tabindex_space_toggles`.

### Feature — Safety pre-flight (executable / free-space / overwrite)
**What & Why.** Catch foot-guns before download. User benefit: the inline warning banner (RECV-3) — "clip.exe is a
program — only run files from people you trust," low-space, and overwrite checks.

**How it is built.**
- Executable warning: derived in `src-tauri` from extension (a **claim**, tagged as such) → `is_executable` on each
  `TransferPreviewDto` file; the UI shows a warning chip, never blocks.
- Free-space: `src-tauri` checks available bytes on the chosen `dest` volume vs the selected total; if short → the
  `no-space` warning/dead state. Overwrite: check `dest` for existing target names; warn + offer rename/replace.
- All three recompute live as the selection changes.

**TDD (write first).**
- *Shell:* `executable_extensions_flagged` (`.exe/.bat/.sh/...` set; `.jpg` not).
- *Shell:* `free_space_check` — selected total > available → shortfall reported; ≤ available → ok.
- *Shell:* `overwrite_detection` — existing target name detected.
- *UI:* `warning_banner_shows_for_exec_and_lowspace`; `space_estimate_recomputes_on_selection`.

---

## M5 — The control stream (free two-way coordination)

A second custom-ALPN protocol on the **same** endpoint — additive on the existing `Router`. The medium makes this
free; rank by human value, not cost.

### Feature — Control-stream protocol + presence
**What & Why.** A persistent two-way channel between the two peers for presence, instant decline/ack, chat, and
request-to-send. User benefit: "who's there," instant "no," coordination — all free, no third party.

**How it is built (exact iroh APIs, verified in 0.103 `custom-protocol.rs`).**
- New module `core/src/control.rs` defining `pub const CTRL_ALPN: &[u8] = b"dropwire/ctrl/1";` and a `Ctrl` struct
  implementing `iroh::protocol::ProtocolHandler`:
  ```rust
  impl ProtocolHandler for Ctrl {
      async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
          let (mut send, mut recv) = conn.accept_bi().await?;   // bidi control stream
          // read length-prefixed CtrlMsg JSON frames; route to the matching transfer
      }
  }
  ```
- Register additively in `Core::start`: `Router::builder(endpoint).accept(BLOBS_ALPN, blobs).accept(CTRL_ALPN,
  ctrl).spawn()` (additive `.accept()` verified — blobs ALPN coexists). The dialing side opens the control stream
  with `endpoint.connect(addr, CTRL_ALPN)` (a *second* connection on the same endpoint; or reuse via `open_bi` on a
  shared conn if we standardize one ALPN with framing — start with a dedicated ALPN for clarity).
- `#[derive(Serialize, Deserialize)] enum CtrlMsg { Hello { fingerprint }, Presence { typing: bool }, Decline {
  transfer }, Ack { transfer }, Chat { text }, Bye }`. Length-prefixed frames; `text` length-capped; control chars
  stripped. New `Progress`/event signals: `PeerPresent`, `PeerTyping`, `PeerDeclined`, `PeerLeft`, `ChatMessage`.

**TDD (write first).**
- *Core integration:* `control_stream_coexists_with_blobs` — with both ALPNs registered, a normal `send`/`receive`
  still completes (additive accept doesn't break blobs).
- *Core integration:* `presence_hello_roundtrip` — two cores exchange `Hello`/`Presence` over `CTRL_ALPN`; each sees
  the other's bound fingerprint.
- *Core unit:* `ctrlmsg_roundtrip` and `ctrlmsg_text_sanitized` (length cap + control-char strip; never HTML).

### Feature — Instant Decline / Ack
**What & Why.** The sender hears "no" instantly instead of waiting for a timeout (the one §5.7 item that wanted a real
back-channel). User benefit: DEAD-STATES — "They declined — nothing was sent" appears immediately.

**How it is built.**
- The accept modal's **[Decline]** sends `CtrlMsg::Decline { transfer }` over the control stream before dismissing
  (so the sender's status flips immediately) and closes the cached preview. The sender maps it to `PeerDeclined` →
  the `peer-declined` dead state. `Ack` confirms receipt of an accept for snappy UI.

**TDD (write first).**
- *Core integration:* `decline_reaches_sender_instantly` — receiver declines; sender observes `PeerDeclined` well
  before any blobs timeout; sender catalog marks declined; **no payload** sent.
- *UI:* `decline_button_sends_ctrl_decline_then_dismisses`.

### Feature — Chat-lite
**What & Why.** Let the two humans coordinate ("it's the big folder", "thanks!"). User benefit: a collapsed,
never-blocking chat pill (SEND-4/RECV-4) on the live control stream — free here.

**How it is built.**
- `CtrlMsg::Chat { text }` ↔ `ChatMessage` events. UI: hidden (no peer) · collapsed (pill + unread dot) · open (log
  + composer) · peer-typing · peer-left (composer disabled). Messages are claims by nature → no trust chrome, but
  **plain text only** (never `innerHTML`, no link auto-linking without the suspicion treatment). Composer is
  reachable but **never steals focus** from the transfer.

**TDD (write first).**
- *Core integration:* `chat_roundtrip` — bidirectional messages delivered in order.
- *UI:* `chat_messages_are_plain_text` (XSS payload rendered literally); `chat_does_not_steal_focus`;
  `peer_left_disables_composer`.

### Feature — Request-to-send (reverse-direction offer)
**What & Why.** A receiver can ask the other peer to send something, over the same channel. User benefit: symmetric
"sender offers X / receiver accepts" coordination without a new idiom.

**How it is built.**
- `CtrlMsg::RequestToSend { note }` → a prompt on the other side that pre-fills the Send flow. Stays within the "two
  mirrored verbs, one wire" grammar (no third idiom). Lower priority within M5; ship after chat.

**TDD (write first).**
- *Core integration:* `request_to_send_delivers_prompt`.
- *UI:* `request_to_send_prefills_send_view`.

---

## M6 — Reach & devices

The trust grammar persists post-transfer; resume becomes a user-facing affordance; every failure is an on-brand card.

### Feature — Device / peer identity chip
**What & Why.** Show WHO is on the other end as a bound fact. User benefit: the peer's short `EndpointId` fingerprint
(cryptographic, un-spoofable) beside the sender's claimed friendly name — same grammar in modal, Allow card, header,
History, Settings ("this is you").

**How it is built.**
- Bound `sender_fingerprint`/`peer_fingerprint` already flow from `Connection::remote_id()` (M1/M3). The chip always
  carries the lime bound chip; the friendly name (`from`) always carries the claim chip. Settings shows the local
  `core.endpoint_id()` with copy. (Future "mismatch warning" if a fingerprint differs from a previously-seen one for
  the same claimed name — catalog-backed, deferred.)

**TDD (write first, UI/JS):** `fingerprint_is_bound_chip`; `friendly_name_is_claim_chip`; `settings_shows_local_id`.

### Feature — History (extended) + Resume affordance
**What & Why.** Reuse the directional wire-glyph; surface peer fingerprint + claimed name post-transfer; offer
[Resume] for interrupted receives. User benefit: the trust grammar persists, and the resume promise (proven in M0)
becomes one click.

**How it is built.**
- `catalog.rs` already records direction, ticket, hash, dest, totals, and a `Status::Interrupted` (set on startup by
  `mark_stale_interrupted`). Add `peer_fingerprint` + `from` to `TransferRecord`. History rows: sent · received ·
  resumable ([Resume] → re-`inspect` + auto-`accept` into the same dest; `local.missing()` picks up the partial) ·
  failed · declined. Status dot is color **and** labeled (color never the only signal). `…` menu: Reveal / Copy code /
  Remove.

**TDD (write first).**
- *Core integration:* `interrupted_receive_is_resumable_from_history` — interrupt; restart a `Core` against the same
  data dir; assert the record is `Interrupted`; resume via the recorded ticket → completes from partial.
- *Core unit:* `record_carries_peer_and_claim_fields`.
- *UI:* `history_row_shows_bound_and_claim`; `resume_button_only_on_interrupted`.

### Feature — Dead-state cards (no blank boxes)
**What & Why.** Every failure path is a designed, on-brand surface. User benefit: DEAD-STATES — sender-unreachable,
expired/used ticket, peer-declined, sender-declined-gate, interrupted, no-space, manifest-invalid — each a broken/dim
wire glyph + plain-words cause + exactly one recovery action.

**How it is built.**
- One UI pattern keyed off the mapped error/lifecycle states from M1–M5 (`sender-unreachable` from `inspect` timeout;
  `sender-declined-gate` from the gate's permission error; `peer-declined` from `CtrlMsg::Decline`; `interrupted`
  from `Status::Interrupted`; `no-space`/`overwrite` from M4 pre-flight; `manifest-invalid` from `Manifest::parse`
  failure or name/size mismatch vs the HashSeq). The wire shows the 6px break/`--error` freeze; reduced-motion uses
  instant color state changes only.

**TDD (write first).**
- *Shell/Core:* each error path maps to a stable, testable code (`error_code_for(...)` is exhaustive).
- *UI:* `each_dead_state_renders_cause_and_single_action`; `interrupted_offers_resume`;
  `manifest_invalid_state_renders` (a corrupt/mismatched manifest → the invalid card, not a crash).

### Feature — Route badge polish
**What & Why.** Honest direct/relayed disclosure (existing, kept) as a bound fact about the path. User benefit: the
"direct" / "relayed · a bit slower" badge with the connecting-pulse state.

**How it is built.**
- Already implemented in `receive.rs` via `conn.paths_stream()`/`is_selected()`/`is_relay()`/`is_ip()` (live
  relay→direct upgrade). M6 only ensures the badge appears in the accept modal and the active-transfer header with
  the bound chip, and that `connecting` shows the breathing-dot state. Carry the same `Route` into sender-side
  `Transferring` (currently `Route::Unknown` on the send path) by reading the served connection's path.

**TDD (write first).**
- *UI:* `route_badge_states` (connecting/direct/relayed with paired text); *Core:* `sender_route_is_not_unknown`
  (the send-side `Transferring` carries a real `Route` once a peer connects).

---

## Appendix — Load-bearing API facts grounded against iroh-blobs 0.103 source (2026-06-17)

These are the exact symbols the plan depends on, confirmed by reading the registry source — record them so an
implementer doesn't re-derive:

- **Collection is ordered** (`src/format/collection.rs`): `Collection(Vec<(String, Hash)>)`, `FromIterator` preserves
  insertion order → inserting the manifest at index 0 makes it child 0. On the wire, HashSeq offset 0 is the
  root/metadata; children are offsets `1..N`.
- **Gate** (`src/provider/events.rs`): `RequestMode::Intercept`/`InterceptLog`; `ProviderMessage::GetRequestReceived`
  (intercept) carries `request: GetRequest` (`.hash`, `.ranges`, `.ranges.is_blob()`) and `tx:
  oneshot::Sender<EventResult>`; the provider blocks on `rx.await??` *before* serving (the `request()` fn in
  `EventSender`). Reply `Ok(())` / `Err(AbortReason::Permission)`. Pattern: `examples/limit.rs::limit_by_hash`.
- **Selective/range fetch** (`src/protocol.rs`): `GetRequest::builder().root(r).child(i, r).build(hash)` — `child(c)`
  maps to offset `c+1`; `GetRequest::blob_ranges(hash, ranges)`; ranges are BLAKE3/BAO-verified.
  `store.remote().execute_get(conn, request)` accepts **any** hand-built `GetRequest`
  (`src/api/remote.rs::execute_get`).
- **Resume** (`src/api/remote.rs`): `remote().local(hf) -> LocalInfo`, `LocalInfo::is_complete()`,
  `LocalInfo::missing() -> GetRequest`.
- **Preview cheap-fetch**: `get_hash_seq_and_sizes(&conn, &hash, 32<<20, None)` (already used in `receive.rs`);
  `get::request::get_blob(conn, hash)` for child 0 (`bytes()`/`bytes_and_stats()`).
- **Custom control ALPN** (`examples/custom-protocol.rs`): implement `iroh::protocol::ProtocolHandler::accept(&self,
  conn)`, use `conn.accept_bi()`; register additively `Router::builder(ep).accept(BLOBS_ALPN, blobs).accept(CTRL_ALPN,
  ctrl).spawn()`. Peer identity (bound): `conn.remote_id() -> EndpointId`.

---

## Risks & guardrails

1. **Resume on the pre-1.0 `FsStore`** is the #1 regression risk across blobs version bumps. M0 makes the
   kill-mid-transfer test deterministic (interrupt on a content condition, assert `missing()` shrank) and
   CI-required — but a future 0.10x bump can still silently break byte-perfect resume. Keep that test as the canary.
2. **Manifest-as-child-0 off-by-one.** HashSeq offset 0 is the root/metadata, the manifest is child 0 = offset 1,
   payload files are children `1..N` = offsets `2..N+1`, and selective index `i` maps to `builder.child(i+1)`.
   Mis-mapping silently fetches the wrong blob; guarded by `preview_total_excludes_manifest`,
   `selection_maps_child_index_plus_one`, and the `manifest_is_child_zero` integration test.
3. **Gate classification.** The gate must always allow the manifest pull while gating payload pulls, distinguished
   only by `GetRequest.ranges`. A misclassification either blocks preview (breaks see-before-commit) or leaks bytes
   (breaks consent). Covered by `gate_request_classifier` + `manifest_pull_is_never_gated` +
   `gate_blocks_payload_until_allowed`.
4. **Trust-tier spoofing.** Product integrity rests on the *engine* (not the manifest) deciding bound vs claim, and
   on claims never being rendered as HTML. A single `innerHTML` slip or a claim reaching the bound renderer defeats
   it; enforced structurally and tested (`note_is_never_html`, `claim_cannot_reach_bound_renderer`,
   `chat_messages_are_plain_text`).
5. **Cached-connection lifecycle in `inspect`.** Holding a live `Connection` per `PreviewId` between inspect and
   accept needs TTL sweeps + close-on-decline or it leaks sockets; a hung/offline sender must surface as
   `sender-unreachable`, not hang.
6. **Two-connection control plane.** Dialing `CTRL_ALPN` as a second connection doubles hole-punch/relay setup and
   presence can race the transfer. Start with a dedicated ALPN for clarity, verify additive `Router.accept` doesn't
   regress blobs (`control_stream_coexists_with_blobs`), and watch connection count on relay-only paths.
7. **API drift (pre-1.0 blobs).** `add_bytes`/`TempTag`, `get_blob`, `execute_get` accepting hand-built `GetRequest`,
   and the `GetRequestReceived` oneshot-before-serving guarantee are verified today but live on the volatile 0.10x
   line. Keep them exercised by integration tests so a bump fails loudly inside the one `irohcore` firewall crate.
