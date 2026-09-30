import { extract, safeName, uniqueName } from "./safety.js";
import init, { BrowserNode } from "./runtime/dropwire_browser.js";

const $ = (id) => document.getElementById(id);
const mode = location.pathname.endsWith("/receive") || new URLSearchParams(location.search).get("mode") === "receive" ? "receive" : "send";
let node, handle, ready, chosen = [], incoming = [], code = "", selected = [], busy = false, session = 0, poll;
let diskSession;
// Opt-in QA telemetry stays in this document; nothing is sent to a server.
const diagnostics = new URLSearchParams(location.search).has("diagnostics");
let memory;
function measureMemory() {
  if (diagnostics && memory) document.documentElement.dataset.wasmMemoryBytes = String(memory.buffer.byteLength);
}
async function disk(run) {
  if (!navigator.storage?.getDirectory) throw new Error("This browser cannot stream to local disk. Use a current desktop browser or the desktop app.");
  if (!diskSession) {
    const root = await navigator.storage.getDirectory();
    if (run !== session) throw new Error("Transfer stopped.");
    const name = `dropwire-${crypto.randomUUID()}`;
    const dir = await root.getDirectoryHandle(name, { create: true });
    if (run !== session) { await root.removeEntry(name, { recursive: true }); throw new Error("Transfer stopped."); }
    diskSession = { root, name, dir, writers: new Set() };
  }
  return diskSession;
}
let retryAction;
$("send-panel").hidden = mode !== "send";
$("receive-panel").hidden = mode !== "receive";
$(mode + "-tab").setAttribute("aria-current", "page");
document.title = `Dropwire · ${mode === "send" ? "send" : "receive"} files`;

function status(text, error = false) { $("status").textContent = text; $("status").dataset.error = String(error); measureMemory(); }
function size(bytes) { return bytes === 0 ? "0 B" : bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KiB` : `${(bytes / 1024 / 1024).toFixed(1)} MiB`; }
function setBusy(value) {
  busy = value;
  for (const id of ["preview", "share", "accept", "reject", "files"]) $(id).disabled = value;
  $("share").disabled = value || !chosen.length;
}
function error(err, retry) {
  status(err instanceof Error ? err.message : "The transfer could not finish. Check your connection and retry.", true);
  retryAction = retry;
  $("retry").hidden = !retry;
}
async function getNode(run) {
  status("Loading the transfer engine…");
  ready ??= init();
  memory = (await ready).memory;
  if (run !== session) throw new Error("Transfer stopped.");
  if (!node) {
    const response = await fetch("/dropwire/transfer/config.json", { cache: "no-store", credentials: "same-origin" });
    if (!response.ok) throw new Error("Browser relay configuration is unavailable. Use the desktop app.");
    const config = await response.json();
    if (!config.enabled) throw new Error(config.status === "parked" ? "Dropwire web is parked. Download the desktop app above." : "Browser transfers are unavailable. Download the desktop app above.");
    status("Starting an encrypted endpoint…");
    const created = await BrowserNode.spawn(JSON.stringify(config.relays));
    if (run !== session) { await created.close(); created.free(); throw new Error("Transfer stopped."); }
    node = created; handle = node.cancel_handle();
  }
  $("cancel").hidden = false;
  return node;
}
async function clear() {
  const wasBusy = busy;
  setBusy(true);
  session++;
  clearInterval(poll);
  $("sharing").hidden = $("manifest").hidden = $("saved").hidden = true;
  $("route").hidden = $("progress").hidden = $("retry").hidden = $("cancel").hidden = true;
  $("share-link").value = "";
  $("ticket").value = "";
  $("incoming").replaceChildren(); $("downloads").replaceChildren();
  $("qr").replaceChildren();
  code = ""; incoming = []; selected = [];
  const previous = node, cancel = handle, storage = diskSession;
  diskSession = undefined;
  node = undefined; handle = undefined;
  if (cancel) { await cancel.cancel(); cancel.free(); }
  // A pending WASM operation retains its borrow until the closed connection settles.
  if (previous && !wasBusy) { await previous.close(); previous.free(); }
  let cleanupFailed = false;
  if (storage) {
    await Promise.allSettled(Array.from(storage.writers, (writer) => writer.abort()));
    try { await storage.root.removeEntry(storage.name, { recursive: true }); } catch { cleanupFailed = true; }
  }
  setBusy(false);
  status(cleanupFailed ? "Stopped. Clear this site's storage in your browser if temporary files remain." : "Transfer stopped. Files are no longer shared. Start a new session when ready.", cleanupFailed);
}
$("cancel").addEventListener("click", () => void clear());
$("retry").addEventListener("click", () => { $("retry").hidden = true; void retryAction?.(); });

function choose(files) {
  if (busy || code) { status("Stop the current transfer before choosing other files.", true); return; }
  const list = Array.from(files);
  if (!list.length) return;
  if (list.some((file) => !Number.isSafeInteger(file.size)) || list.length > 100000 || !Number.isSafeInteger(list.reduce((n, f) => n + f.size, 0))) { status("This file list exceeds your browser's metadata or numeric safety bounds. Split the folder or use the desktop app.", true); $("files").value = ""; return; }
  chosen = list;
  $("chosen").replaceChildren(...list.map((file) => { const row = document.createElement("li"), name = document.createElement("span"), bytes = document.createElement("small"); name.textContent = file.name; bytes.textContent = size(file.size); row.append(name, bytes); return row; }));
  $("share").disabled = false;
  status(`${list.length} file${list.length === 1 ? "" : "s"} · ${size(list.reduce((n, f) => n + f.size, 0))} · ready to share.`);
}
$("files").addEventListener("change", (event) => choose(event.target.files));
$("dropzone").addEventListener("dragover", (event) => { event.preventDefault(); $("dropzone").classList.add("dragging"); });
$("dropzone").addEventListener("dragleave", () => $("dropzone").classList.remove("dragging"));
$("dropzone").addEventListener("drop", (event) => { event.preventDefault(); $("dropzone").classList.remove("dragging"); choose(event.dataTransfer.files); });

async function share() {
  if (busy || !chosen.length) return;
  const run = ++session;
  setBusy(true); status("Preparing files and connecting to an encrypted relay…");
  let current;
  try {
    current = await getNode(run);
    const storage = await disk(run);
    // A failed preparation starts over with a fresh store, never duplicates prior imports.
    const used = new Set();
    for (const file of chosen) {
      if (run !== session) return;
      const safe = safeName(file.name);
      const unique = uniqueName(safe, used); used.add(unique.toLowerCase());
      const proof = await storage.dir.getFileHandle(`proof-${used.size}`, { create: true });
      const writer = await proof.createWritable(); storage.writers.add(writer);
      status(`Verifying ${unique}… Keep this tab open.`);
      const read = async (offset, length) => {
        if (run !== session) throw new Error("Transfer stopped.");
        return new Uint8Array(await file.slice(offset, offset + length).arrayBuffer());
      };
      await current.add_file(unique, file.size, read,
        async (offset, length) => new Uint8Array(await (await proof.getFile()).slice(offset, offset + length).arrayBuffer()),
        async (offset, data) => { if (run !== session) throw new Error("Transfer stopped."); await writer.write({ type: "write", position: offset, data }); });
      await writer.close(); storage.writers.delete(writer);
    }
    status("Connecting to an encrypted relay…");
    const ticket = await current.share();
    if (run !== session) return;
    code = ticket;
    const link = `${location.origin}/dropwire/receive#${encodeURIComponent(ticket)}`;
    $("share-link").value = link;
    try { $("qr").innerHTML = current.qr_svg(link); } catch { $("qr").textContent = "Use the link below."; }
    $("sharing").hidden = false; $("dropzone").hidden = $("share").hidden = true;
    status("Waiting for the recipient. Keep this tab open.");
    poll = setInterval(() => {
      if (run !== session || busy) return;
      const state = JSON.parse(current.status());
      if (["previewing", "transferring", "done"].includes(state.phase)) $("route").hidden = false;
      const labels = { waiting: "Waiting for the recipient. Keep this tab open.", previewing: "The recipient is previewing your files.", transferring: "Sending through an encrypted relay…", done: "Selected files sent. The recipient verifies and saves them.", declined: "The recipient declined. You can share this link with someone else.", interrupted: "Transfer interrupted. Retry, or stop and create a new link if a source file changed." };
      status(labels[state.phase] || "Keep this tab open.");
      if (state.phase === "transferring" || state.phase === "done") { $("progress").hidden = false; $("progress").value = state.phase === "done" ? 100 : state.total ? state.bytes / state.total * 100 : 0; }
    }, 250);
  } catch (err) {
    if (run === session) { error(err, async () => { await clear(); $("dropzone").hidden = $("share").hidden = false; await share(); }); }
  } finally {
    if (run === session) { setBusy(false); $("share").disabled = !!code; }
    else if (current) { await current.close(); current.free(); }
  }
}
$("share").addEventListener("click", () => void share());
for (const [id, value] of [["copy-link", () => $("share-link").value], ["copy-code", () => code]]) $(id).addEventListener("click", async () => {
  try { await navigator.clipboard.writeText(value()); status("Copied. Share privately with the intended recipient."); }
  catch { $("share-link").value = value(); $("share-link").focus(); $("share-link").select(); status("Select and copy the value above. Clipboard access is unavailable."); }
});

// A fragment stays out of HTTP requests and referrers. Remove it from history immediately.
if (mode === "receive" && location.hash) {
  try { $("ticket").value = decodeURIComponent(location.hash.slice(1)); } catch { status("This link is damaged. Ask for a fresh one.", true); }
  history.replaceState(null, "", location.pathname + location.search);
}
async function preview() {
  if (busy) return;
  const run = ++session;
  let current;
  setBusy(true); status("Connecting to the sender and verifying the file list…");
  $("retry").hidden = true; $("manifest").hidden = $("saved").hidden = true;
  try {
    code = extract($("ticket").value);
    current = await getNode(run);
    const result = JSON.parse(await current.inspect(code));
    if (run !== session) return;
    incoming = result.files;
    const used = new Set();
    incoming.forEach((file) => { file.saveName = uniqueName(safeName(file.name), used); used.add(file.saveName.toLowerCase()); });
    $("incoming").replaceChildren(...incoming.map((file, index) => {
      const row = document.createElement("li"), label = document.createElement("label"), box = document.createElement("input"), name = document.createElement("span"), bytes = document.createElement("small");
      box.type = "checkbox"; box.checked = true; box.dataset.index = String(index);
      name.textContent = file.name; bytes.textContent = size(file.size); label.append(box, name); row.append(label, bytes); return row;
    }));
    $("manifest").hidden = false; $("route").hidden = false;
    status(`${incoming.length} verified files · ${size(result.total)}. Accept only what you want.`);
  } catch (err) { if (run === session) error(err, preview); }
  finally { if (run === session) setBusy(false); else if (current) { await current.close(); current.free(); } }
}
$("preview").addEventListener("click", () => void preview());
async function receive() {
  if (busy || !node) return;
  selected = Array.from($("incoming").querySelectorAll("input:checked")).map((input) => Number(input.dataset.index));
  if (!selected.length) { status("Choose at least one file.", true); return; }
  const run = session, current = node;
  setBusy(true); status("Receiving and verifying selected files…"); $("progress").hidden = false; $("progress").value = 0;
  try {
    const storage = await disk(run), outputs = new Map();
    const estimate = await navigator.storage.estimate();
    const total = selected.reduce((n, index) => n + incoming[index].size, 0);
    if (estimate.quota && total > estimate.quota - (estimate.usage || 0)) throw new Error("Your browser has insufficient local storage for these files. Choose fewer files or use the desktop app.");
    let active;
    const outputFor = async (index) => {
      if (run !== session) throw new Error("Transfer stopped.");
      if (active?.index === index) return active;
      if (active) { await active.writer.close(); storage.writers.delete(active.writer); }
      const file = await storage.dir.getFileHandle(`receive-${index}`, { create: true });
      const writer = await file.createWritable(); storage.writers.add(writer);
      active = { index, file, writer }; outputs.set(index, active); return active;
    };
    await current.receive(JSON.stringify(selected),
      async (index, offset, data) => { const output = await outputFor(index); await output.writer.write({ type: "write", position: offset, data }); },
      (done, total) => { if (run === session) { $("progress").value = total ? done / total * 100 : 0; measureMemory(); } });
    if (active) { await active.writer.close(); storage.writers.delete(active.writer); }
    // Empty files have no payload leaves, but still need a zero-byte download.
    for (const index of selected) if (!outputs.has(index)) {
      const file = await storage.dir.getFileHandle(`receive-${index}`, { create: true });
      const writer = await file.createWritable(); await writer.close(); outputs.set(index, { file });
    }
    if (run !== session) return;
    $("progress").value = 100; $("manifest").hidden = true; $("saved").hidden = false;
    $("downloads").replaceChildren(...selected.map((index) => {
      const file = incoming[index], row = document.createElement("li"), name = document.createElement("span"), button = document.createElement("button");
      name.textContent = file.saveName; button.textContent = "Save"; button.setAttribute("aria-label", `Save ${file.saveName}`);
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          const url = URL.createObjectURL(await outputs.get(index).file.getFile());
          const link = document.createElement("a"); link.href = url; link.download = file.saveName; link.rel = "noreferrer"; document.body.append(link); link.click(); link.remove();
          setTimeout(() => URL.revokeObjectURL(url), 60000);
          button.textContent = "Save again";
          status("File handed to your browser. Check Downloads to confirm it was saved.");
        } catch (err) { error(err); } finally { button.disabled = false; }
      }); row.append(name, button); return row;
    }));
    status("Selected files received. BLAKE3 integrity verified. Save each file below.");
  } catch (err) { if (run === session) { if (diskSession) { await Promise.allSettled(Array.from(diskSession.writers, (writer) => writer.abort())); diskSession.writers.clear(); } error(err, receive); } }
  finally { if (run === session) setBusy(false); else { await current.close(); current.free(); } }
}
$("accept").addEventListener("click", () => void receive());
$("reject").addEventListener("click", async () => { const current = node; if (!current || busy) return; setBusy(true); try { await current.decline(); setBusy(false); await clear(); status("Transfer declined. No files were saved."); } catch (err) { error(err); } finally { setBusy(false); } });
$("cancel").addEventListener("click", () => { $("dropzone").hidden = $("share").hidden = false; });
window.addEventListener("beforeunload", (event) => { if (node && (busy || code)) { event.preventDefault(); event.returnValue = ""; } });
