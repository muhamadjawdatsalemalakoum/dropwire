/* Dropwire desktop app — Design v2 ("behaves like hardware, not a web page").
 *
 * Engine contract is UNCHANGED. Every Tauri command and every Progress event
 * kind from v1 survives: my_endpoint_id, my_fingerprint, pick_paths,
 * pick_dest_dir, default_dest_dir, qr_svg, start_send, inspect_ticket,
 * start_receive, start_receive_selected, send_control, cancel_transfer,
 * list_transfers, reveal_path, open_external, app_version, and the nearby set
 * (nearby_start/stop/list/offer/respond). The redesign changes markup and
 * motion, not the engine boundary.
 */

const TAURI = window.__TAURI__;
const HAS_TAURI = !!(TAURI && TAURI.core);
const invoke = HAS_TAURI ? TAURI.core.invoke : async () => { throw new Error('Run inside the Dropwire app.'); };
const makeChannel = () => (HAS_TAURI ? new TAURI.core.Channel() : { onmessage: null });
const RM = matchMedia('(prefers-reduced-motion: reduce)').matches;
const canAnim = !RM && typeof Element.prototype.animate === 'function';
const EASE_OUT = 'cubic-bezier(.22,1,.36,1)';
const EASE_POP = 'cubic-bezier(.34,1.2,.5,1)';

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

function fmtBytes(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${u[i]}`;
}

/* ========================= frameless window chrome ======================== */
const appWindow = (HAS_TAURI && TAURI.window && TAURI.window.getCurrentWindow) ? TAURI.window.getCurrentWindow() : null;
if (appWindow) {
  $('#win-min').addEventListener('click', () => appWindow.minimize().catch(() => {}));
  $('#win-max').addEventListener('click', () => appWindow.toggleMaximize().catch(() => {}));
  $('#win-close').addEventListener('click', () => appWindow.close().catch(() => {}));
  // Double-clicking the bar toggles maximise, the platform convention.
  $('.titlebar').addEventListener('dblclick', (e) => {
    if (e.target.closest('.tb-controls')) return;
    appWindow.toggleMaximize().catch(() => {});
  });
}

/* ============================== navigation ===============================
   One canvas, three segments. The active pill is the plate; the icon carries
   the lime. No rail, no sliding indicator to measure.
   ------------------------------------------------------------------------ */
let curPanel = 'send';
function showPanel(name) {
  if (name === curPanel) return;
  curPanel = name;
  $$('.seg-btn[data-panel]').forEach((b) => {
    const on = b.dataset.panel === name;
    b.classList.toggle('is-active', on);
    b.setAttribute('aria-selected', on ? 'true' : 'false');
  });
  $$('.panel').forEach((p) => {
    const on = p.id === 'panel-' + name;
    p.classList.toggle('is-active', on);
    p.hidden = !on;
  });
  if (name === 'activity') loadHistory();
}
$$('.seg-btn[data-panel]').forEach((b) => b.addEventListener('click', () => showPanel(b.dataset.panel)));
$$('[data-goto]').forEach((b) => b.addEventListener('click', () => showPanel(b.dataset.goto)));

/* ================================ theme ================================== */
function applyTheme(mode) {
  if (mode === 'auto') document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', mode);
  $$('#theme-toggle .seg-btn').forEach((b) => {
    const on = b.dataset.themeSet === mode;
    b.classList.toggle('is-active', on);
    b.setAttribute('aria-checked', on ? 'true' : 'false');
  });
}
(function initTheme() {
  applyTheme(localStorage.getItem('dropwire-theme') || 'auto');
  $$('#theme-toggle .seg-btn').forEach((b) => b.addEventListener('click', () => {
    const m = b.dataset.themeSet; localStorage.setItem('dropwire-theme', m); applyTheme(m);
  }));
})();

/* ============================ settings sheet ============================= */
let settingsLastFocus = null;
function openSettings() {
  settingsLastFocus = document.activeElement;
  $('#sheet-settings').classList.remove('hidden');
  invoke('my_fingerprint').then((fp) => { $('#set-fp').textContent = fp; }).catch(() => {});
  $('#settings-close').focus();
}
function closeSettings() {
  $('#sheet-settings').classList.add('hidden');
  if (settingsLastFocus && settingsLastFocus.focus) settingsLastFocus.focus();
}
$('#open-settings').addEventListener('click', openSettings);
$('#settings-close').addEventListener('click', closeSettings);
$('#sheet-settings').addEventListener('click', (e) => { if (e.target.id === 'sheet-settings') closeSettings(); });
document.addEventListener('keydown', (e) => {
  if (e.key === ',' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); openSettings(); }
});

/* ============================== status bar =============================== */
function setStatus(text, kind) {
  const t = $('#status-text'), d = $('#status-dot');
  if (t) t.textContent = text;
  if (d) { d.classList.remove('up', 'warn'); if (kind) d.classList.add(kind); }
}

/* ============================== the wire ================================= */
function firePulse(svg) {
  if (!canAnim || !svg) return;
  const p = svg.querySelector('.w-pulse');
  if (p) p.animate([{ strokeDashoffset: 1, opacity: 1 }, { strokeDashoffset: 0, opacity: 1 }], { duration: 460, easing: EASE_OUT });
}
function igniteNode(svg, sel) {
  if (!svg) return;
  const node = svg.querySelector(sel);
  if (node) {
    node.classList.add('live');
    if (canAnim) node.animate([{ opacity: .3 }, { opacity: 1 }], { duration: 380, easing: EASE_OUT });
  }
  svg.classList.add('live');
  firePulse(svg);
}
function doneSpark(svg) {
  if (!svg) return;
  svg.classList.add('done'); svg.classList.remove('live', 'connecting');
  svg.querySelectorAll('.w-node').forEach((n) => n.classList.add('done', 'live'));
  if (!canAnim) return;
  const spark = svg.querySelector('.w-spark');
  if (spark) {
    spark.style.stroke = 'var(--success)';
    spark.animate([{ r: '8px', opacity: .9 }, { r: '34px', opacity: 0 }], { duration: 640, easing: EASE_OUT });
  }
}
function setBar(fill, svg, pctEl, done, total) {
  const p = total > 0 ? Math.min(1, done / total) : 0;
  if (fill) fill.style.strokeDashoffset = String(1 - p);
  if (pctEl) pctEl.textContent = Math.round(p * 100) + '%';
  return p;
}
// The route pill never hides a relay: it states the trade-off in words.
function setRoute(el, route) {
  if (!el || !route) return;
  const label = route === 'direct' ? 'direct' : route === 'relayed' ? 'relayed · a bit slower' : 'connected';
  el.textContent = label;
  el.className = 'pill js-route ' + (route === 'direct' ? 'direct' : route === 'relayed' ? 'relayed' : '');
  el.setAttribute('aria-label', 'Connection: ' + label);
}
function removeCard(card) {
  if (!card) return;
  const done = () => { card.remove(); syncLiveState(); };
  if (canAnim) {
    const a = card.animate([{ opacity: 1 }, { opacity: 0, transform: 'translateY(-6px)' }], { duration: 200, easing: EASE_OUT });
    a.onfinish = done;
  } else done();
}
function makeCard(tplId, listId) {
  const card = document.getElementById(tplId).content.firstElementChild.cloneNode(true);
  document.getElementById(listId).prepend(card);
  syncLiveState();
  return card;
}
/* The bar/card demotion is explicit state, not a CSS :has() side-effect. */
function syncLiveState() {
  $('#panel-send').classList.toggle('panel-send-live', $('#send-list').children.length > 0);
  $('#panel-receive').classList.toggle('panel-recv-live', $('#recv-list').children.length > 0);
}

/* ========================= live transfer registry =======================
   One place both the panels and Activity read from, so "In flight" and the
   segment badge never drift from the cards.
   ------------------------------------------------------------------------ */
const live = new Map(); // id -> {dir,name,meta,pct,state,route}
function liveSet(id, patch) {
  if (!id) return;
  live.set(id, Object.assign({ dir: 'send', name: 'transfer', meta: '', pct: 0, state: 'connecting', route: '' }, live.get(id) || {}, patch));
  renderActivityLive();
}
function liveDrop(id) { if (id && live.delete(id)) renderActivityLive(); }

/* ================================= SEND ================================== */
// The most recent send that reached Ready — the transfer "Send here" offers.
let liveSend = null;
async function startSend(path) {
  const card = makeCard('tpl-send', 'send-list');
  const els = {
    code: card.querySelector('.js-code'), len: card.querySelector('.js-len'), qr: card.querySelector('.js-qr'),
    status: card.querySelector('.js-status'), copy: card.querySelector('.js-copy'), full: card.querySelector('.js-full'),
    cancel: card.querySelector('.js-cancel'), fill: card.querySelector('.w-fill'), dot: card.querySelector('.js-dot'),
    pct: card.querySelector('.js-pct'), svg: card.querySelector('svg.wire'), route: card.querySelector('.js-route'),
    meta: card.querySelector('.js-meta'), ends: card.querySelector('.js-ends'),
  };
  els.code.textContent = '…'; els.status.textContent = 'Preparing…';
  let id = null, ticket = '';
  els.copy.addEventListener('click', () => {
    if (ticket && navigator.clipboard) {
      navigator.clipboard.writeText(ticket);
      els.copy.textContent = 'Copied ✓';
      setStatus('code copied to clipboard', 'up');
      setTimeout(() => { els.copy.textContent = 'Copy code'; }, 1400);
    }
  });
  els.full.addEventListener('click', () => {
    const open = els.code.classList.toggle('full');
    els.full.textContent = open ? 'Collapse' : 'Show full code';
  });
  els.cancel.addEventListener('click', async () => {
    if (id) await invoke('cancel_transfer', { id }).catch(() => {});
    liveDrop(id); removeCard(card);
  });
  try {
    const ch = makeChannel(); ch.onmessage = (m) => onSendMsg(m, els, card, () => id, (t) => { ticket = t; });
    id = await invoke('start_send', { path, onEvent: ch });
    liveSet(id, { dir: 'send', name: path.split(/[\\/]/).pop() || 'transfer', state: 'connecting' });
  } catch (e) {
    els.status.textContent = 'Could not start the transfer. Try again.';
    console.warn(e);
  }
}
function onSendMsg(m, els, card, getId, setTicket) {
  const id = getId();
  switch (m.kind) {
    case 'importing':
      els.status.textContent = `Preparing… ${fmtBytes(m.done)} / ${fmtBytes(m.total)}`;
      break;
    case 'ready':
      setTicket(m.ticket);
      els.code.textContent = m.ticket;
      els.len.textContent = m.ticket.length + ' chars';
      els.status.textContent = 'Ready, waiting for someone to enter it. Keep the app open.';
      liveSend = { card, els, ticket: m.ticket };
      liveSet(id, { state: 'ready' });
      invoke('qr_svg', { text: m.ticket }).then((svg) => {
        els.qr.innerHTML = svg;
        els.qr.setAttribute('role', 'img');
        els.qr.setAttribute('aria-label', 'QR code — scan with the receiving device');
        if (canAnim) els.qr.animate([{ opacity: 0, transform: 'scale(.96)' }, { opacity: 1, transform: 'none' }], { duration: 380, easing: EASE_OUT, delay: 80 });
      }).catch(() => {});
      break;
    case 'peerJoined':
      if (els.svg && !els.svg.dataset.lit) { els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting'); igniteNode(els.svg, '.w-node.peer'); }
      els.status.textContent = 'Receiver connected, sending…';
      if (els.dot) els.dot.style.display = 'none';
      liveSet(id, { state: 'transferring' });
      break;
    case 'transferring': {
      const p = setBar(els.fill, els.svg, els.pct, m.offset, m.total);
      setRoute(els.route, m.route);
      els.status.textContent = `Sending… ${fmtBytes(m.offset)} / ${fmtBytes(m.total)}`;
      els.ends.textContent = `${fmtBytes(m.offset)} of ${fmtBytes(m.total)}`;
      liveSet(id, { pct: p, state: 'transferring', route: m.route, meta: `${fmtBytes(m.offset)} of ${fmtBytes(m.total)}` });
      break;
    }
    case 'done':
      setBar(els.fill, els.svg, els.pct, 1, 1);
      if (els.svg) doneSpark(els.svg);
      els.status.textContent = 'Sent ✓';
      els.cancel.textContent = 'Dismiss';
      if (els.dot) els.dot.style.display = 'none';
      if (liveSend && liveSend.card === card) liveSend = null;
      liveDrop(id);
      setStatus('transfer complete', 'up');
      break;
    case 'error':
      if (els.svg) { els.svg.classList.add('failed'); els.svg.classList.remove('connecting', 'live'); }
      els.status.setAttribute('aria-live', 'assertive');
      els.status.textContent = m.message ? `Could not send — ${m.message}` : 'Could not send.';
      if (liveSend && liveSend.card === card) liveSend = null;
      liveDrop(id);
      break;
    case 'cancelled':
      liveDrop(id);
      removeCard(card);
      if (liveSend && liveSend.card === card) liveSend = null;
      setOffering(false);
      break;
  }
}
async function pickAndSend(directory) {
  const p = await invoke('pick_paths', { directory, multiple: false }).catch(() => []);
  if (p && p.length) startSend(p[0]);
}
['#pick-file', '#pick-file-2'].forEach((s) => $(s).addEventListener('click', () => pickAndSend(false)));
['#pick-folder', '#pick-folder-2'].forEach((s) => $(s).addEventListener('click', () => pickAndSend(true)));

/* =============================== RECEIVE ================================= */
let recvDest = null;
let DEFAULT_DEST = null;
function setDestLabel(dir) {
  const l = $('#recv-dest-label');
  if (l) { l.textContent = dir; l.title = dir; }
}
function codeInputs() { return [$('#recv-code-input'), $('#recv-code-input-2')]; }
function currentCode() {
  for (const el of codeInputs()) { const v = (el.value || '').trim(); if (v) return v; }
  return '';
}
codeInputs().forEach((el) => {
  el.addEventListener('input', () => {
    const v = currentCode();
    $('#recv-start').disabled = v.length === 0;
    el.closest('.code-field').classList.remove('bad');
    $('#recv-error').textContent = '';
  });
  el.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); submitCode(); }
  });
});
async function pasteInto(el) {
  try {
    const t = await navigator.clipboard.readText();
    if (t) { el.value = t.trim(); el.dispatchEvent(new Event('input')); }
  } catch (_) { el.focus(); }
}
if ($('#recv-paste')) $('#recv-paste').addEventListener('click', () => pasteInto($('#recv-code-input')));
$('#pick-dest').addEventListener('click', async () => {
  const dir = await invoke('pick_dest_dir').catch(() => null);
  if (dir) { recvDest = dir; setDestLabel(dir); }
});
function submitCode() {
  const ticket = currentCode();
  if (!ticket) return;
  // Reject an obviously-wrong code locally: never dial for it.
  if (!/^blob/i.test(ticket)) {
    $('#recv-error').textContent = 'That code is not valid. Codes start with blob and are one line long.';
    codeInputs().forEach((el) => { if (el.value.trim()) { const f = el.closest('.code-field'); f.classList.add('bad', 'shake'); setTimeout(() => f.classList.remove('shake'), 400); } });
    return;
  }
  const dest = recvDest || localStorage.getItem('dropwire-default-dir') || null;
  openPreview(ticket, dest);
}
$('#recv-start').addEventListener('click', submitCode);
$('#recv-start-2').addEventListener('click', submitCode);

async function beginReceive(ticket, dest, selected) {
  $('#recv-error').textContent = '';
  showPanel('receive');
  const myDest = dest || DEFAULT_DEST;
  const card = makeCard('tpl-recv', 'recv-list');
  const els = {
    name: card.querySelector('.js-name'), route: card.querySelector('.js-route'), sub: card.querySelector('.js-sub'),
    fill: card.querySelector('.w-fill'), pct: card.querySelector('.js-pct'), status: card.querySelector('.js-status'),
    svg: card.querySelector('svg.wire'), open: card.querySelector('.js-open'),
    another: card.querySelector('.js-another'), cancel: card.querySelector('.js-cancel'),
  };
  els.name.textContent = 'Connecting…';
  if (myDest) els.sub.textContent = 'saving to ' + myDest;
  let id = null;
  els.cancel.addEventListener('click', async () => {
    if (id) await invoke('cancel_transfer', { id }).catch(() => {});
    liveDrop(id); removeCard(card);
  });
  els.open.addEventListener('click', async () => { if (myDest) await invoke('reveal_path', { path: myDest }).catch(() => {}); });
  els.another.addEventListener('click', () => { liveDrop(id); removeCard(card); });
  try {
    const ch = makeChannel(); ch.onmessage = (m) => onRecvMsg(m, els, card, () => id);
    id = (selected && selected.length)
      ? await invoke('start_receive_selected', { ticket, dest: myDest, selected, onEvent: ch })
      : await invoke('start_receive', { ticket, dest: myDest, onEvent: ch });
    liveSet(id, { dir: 'recv', name: 'Incoming transfer', state: 'connecting' });
  } catch (e) {
    removeCard(card);
    $('#recv-error').textContent = 'That code does not look right. Check it and try again.';
    console.warn(e);
  }
}
function onRecvMsg(m, els, card, getId) {
  const id = getId();
  switch (m.kind) {
    case 'transferring': {
      if (els.svg && !els.svg.dataset.lit) {
        els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting');
        igniteNode(els.svg, '.w-node.peer'); els.name.textContent = 'Receiving…';
      }
      setRoute(els.route, m.route);
      const p = setBar(els.fill, els.svg, els.pct, m.offset, m.total);
      els.status.textContent = `${fmtBytes(m.offset)} of ${fmtBytes(m.total)}`;
      liveSet(id, { dir: 'recv', pct: p, state: 'transferring', route: m.route, meta: `${fmtBytes(m.offset)} of ${fmtBytes(m.total)}` });
      break;
    }
    case 'done':
      if (els.svg) doneSpark(els.svg);
      setBar(els.fill, els.svg, els.pct, 1, 1);
      els.name.textContent = 'Received ✓';
      els.status.textContent = `${fmtBytes(m.stats && m.stats.bytes)} in ${((m.stats && m.stats.seconds) || 0).toFixed(1)}s`;
      els.open.classList.remove('hidden'); els.another.classList.remove('hidden'); els.cancel.classList.add('hidden');
      liveDrop(id);
      setStatus('received', 'up');
      break;
    case 'error': {
      const offline = /reach the sender|offline|link expired|unreachable/i.test(m.message || '');
      if (els.svg) { els.svg.classList.add('failed'); els.svg.classList.remove('connecting', 'live'); }
      els.status.setAttribute('aria-live', 'assertive');
      els.name.textContent = offline ? 'Sender offline or code expired' : 'Failed';
      els.status.textContent = offline ? 'Ask for a fresh code and try again.' : (m.message || '');
      els.cancel.textContent = 'Dismiss';
      liveDrop(id);
      break;
    }
    case 'cancelled':
      liveDrop(id);
      removeCard(card);
      break;
  }
}

/* ==================== preview / accept, the consent gate ================= */
let previewTicket = null, previewDest = null, previewLastFocus = null, previewFiles = [], previewLoaded = false;
function checkedIndices() {
  return $$('#preview-files .file-check').filter((c) => c.checked).map((c) => Number(c.dataset.index));
}
function updateAcceptState() {
  const idx = checkedIndices();
  const total = previewFiles.length;
  $('#preview-accept').disabled = idx.length === 0;
  $('#preview-accept').textContent = idx.length && idx.length < total ? `Accept ${idx.length} files` : 'Accept';
  $('#preview-selcount').textContent = idx.length === total ? `All ${total} selected` : `${idx.length} of ${total} selected`;
  const sum = idx.reduce((s, i) => s + (previewFiles[i] ? previewFiles[i].size : 0), 0);
  $('#preview-seltotal').textContent = fmtBytes(sum);
  const all = $('#preview-all');
  all.checked = idx.length === total; all.indeterminate = idx.length > 0 && idx.length < total;
}
function fillPreview(p) {
  setRoute($('#preview-route'), p.route);
  $('#preview-route').classList.add('pill');
  previewFiles = p.files || [];
  $('#preview-summary').textContent = `${previewFiles.length} file${previewFiles.length === 1 ? '' : 's'} · ${fmtBytes(p.totalBytes)}`;
  const ul = $('#preview-files'); ul.innerHTML = '';
  previewFiles.forEach((f, i) => {
    const li = document.createElement('li'); li.className = 'file-row';
    const cb = document.createElement('input');
    cb.type = 'checkbox'; cb.className = 'file-check'; cb.checked = true;
    cb.dataset.index = String(i); cb.setAttribute('aria-label', `Include ${f.name}`);
    cb.addEventListener('change', updateAcceptState);
    const name = document.createElement('span'); name.className = 'file-name'; name.textContent = f.name; name.title = f.name;
    const size = document.createElement('span'); size.className = 'file-size'; size.textContent = fmtBytes(f.size);
    li.append(cb, name, size); ul.appendChild(li);
  });
  updateAcceptState();
}
$('#preview-all').addEventListener('change', (e) => {
  $$('#preview-files .file-check').forEach((c) => { c.checked = e.target.checked; });
  updateAcceptState();
});
async function openPreview(ticket, dest) {
  previewTicket = ticket; previewDest = dest; previewLoaded = false; previewFiles = [];
  previewLastFocus = document.activeElement;
  $('#recv-error').textContent = '';
  $('#preview-summary').textContent = 'Connecting to the sender…';
  $('#preview-files').innerHTML = '';
  $('#preview-selcount').textContent = ''; $('#preview-seltotal').textContent = '';
  const route = $('#preview-route'); route.textContent = 'connecting'; route.className = 'pill connecting';
  const accept = $('#preview-accept'); accept.disabled = true; accept.classList.remove('hidden'); accept.textContent = 'Accept';
  const decline = $('#preview-decline'); decline.textContent = 'Cancel';
  $('#preview-note').textContent = 'Nothing is saved until you accept.';
  $('#recv-preview').classList.remove('hidden');
  decline.focus();
  try {
    const p = await invoke('inspect_ticket', { ticket });
    if (previewTicket !== ticket) return;
    fillPreview(p);
    previewLoaded = true;
    decline.textContent = 'Decline';
    $('#preview-note').textContent = 'Nothing is saved until you accept. Untick anything you do not want, only what you choose is transferred.';
    accept.focus();
  } catch (e) {
    if (previewTicket !== ticket) return;
    $('#preview-summary').textContent = 'Sender offline, or this code has expired.';
    $('#preview-note').textContent = 'We could not reach the sender. They need to be online, and the code still valid, for the transfer to start.';
    accept.classList.add('hidden');
    decline.textContent = 'Close';
  }
}
function closePreview() {
  $('#recv-preview').classList.add('hidden');
  previewTicket = null; previewDest = null; previewLoaded = false;
  if (previewLastFocus && previewLastFocus.focus) previewLastFocus.focus();
  showNextOffer();
}
$('#preview-accept').addEventListener('click', () => {
  const t = previewTicket, d = previewDest, total = previewFiles.length;
  const idx = checkedIndices();
  closePreview();
  if (!t) return;
  const selected = (idx.length > 0 && idx.length < total) ? idx : null;
  codeInputs().forEach((el) => { el.value = ''; });
  $('#recv-start').disabled = true;
  beginReceive(t, d, selected);
});
$('#preview-decline').addEventListener('click', () => {
  if (previewTicket && previewLoaded) invoke('send_control', { ticket: previewTicket, kind: 'decline' }).catch(() => {});
  closePreview();
});
$('#recv-preview').addEventListener('click', (e) => { if (e.target.id === 'recv-preview') closePreview(); });

/* ============================== ACTIVITY ================================= */
const DIR_GLYPH = {
  send: '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M12 19V5M5 12l7-7 7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  recv: '<svg viewBox="0 0 24 24" width="17" height="17" aria-hidden="true"><path d="M12 5v14M5 12l7 7 7-7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};
function actRow({ dir, name, meta, pct, pill, pillClass, actions }) {
  const row = document.getElementById('tpl-act-row').content.firstElementChild.cloneNode(true);
  const d = row.querySelector('.js-dir');
  d.className = 'act-dir js-dir ' + (dir === 'recv' ? 'recv' : 'send');
  d.innerHTML = DIR_GLYPH[dir === 'recv' ? 'recv' : 'send'];
  const n = row.querySelector('.js-name'); n.textContent = name; n.title = name;
  row.querySelector('.js-meta').textContent = meta || '';
  row.querySelector('.js-pct').textContent = pct == null ? '' : Math.round(pct * 100) + '%';
  const p = row.querySelector('.js-pill');
  if (pill) { p.textContent = pill; p.className = 'pill js-pill ' + (pillClass || ''); } else p.remove();
  const a = row.querySelector('.js-actions');
  (actions || []).forEach(({ label, onClick, cls }) => {
    const b = document.createElement('button');
    b.className = (cls || 'btn-quiet') + ' sm'; b.textContent = label;
    b.addEventListener('click', onClick); a.appendChild(b);
  });
  return row;
}
function renderActivityLive() {
  const list = $('#act-live'), section = $('#act-live-section');
  const items = [...live.entries()];
  section.hidden = items.length === 0;
  list.innerHTML = '';
  items.forEach(([, t]) => {
    list.appendChild(actRow({
      dir: t.dir, name: t.name, meta: t.meta,
      pct: t.state === 'transferring' ? t.pct : null,
      pill: t.state === 'ready' ? 'waiting' : (t.route || t.state),
      pillClass: t.route === 'direct' ? 'direct' : t.route === 'relayed' ? 'relayed' : 'connecting',
    }));
  });
  $('#act-live-meta').textContent = items.length ? `${items.length} running` : '';
  const badge = $('#activity-badge');
  badge.textContent = String(items.length);
  badge.classList.toggle('hidden', items.length === 0);
  updateActivityEmpty();
}
function updateActivityEmpty() {
  const hasLive = !$('#act-live-section').hidden;
  const hasHist = !$('#act-earlier-section').hidden;
  $('#act-empty').hidden = hasLive || hasHist;
}
const STATUS_LABEL = { active: 'In progress', done: 'Done', error: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted' };
const STATUS_PILL = { done: 'done', error: 'failed', cancelled: 'failed', interrupted: 'resuming', active: 'connecting' };
async function loadHistory() {
  const list = $('#history-list'), section = $('#act-earlier-section');
  let items = [];
  try { items = await invoke('list_transfers'); } catch (_) {}
  list.innerHTML = '';
  section.hidden = !items || !items.length;
  if (items && items.length) {
    items.forEach((t, i) => {
      const dir = (t.direction || '').toLowerCase();
      const resumable = dir === 'receive' && t.status === 'interrupted' && t.ticket && t.dest;
      const resendable = dir === 'send' && !!t.source;
      const actions = [];
      if (resumable) actions.push({ label: 'Resume', cls: 'btn-ghost', onClick: () => beginReceive(t.ticket, t.dest) });
      else if (resendable) actions.push({ label: 'Resend', cls: 'btn-ghost', onClick: () => { showPanel('send'); startSend(t.source); } });
      if (t.status === 'done' && t.dest) actions.push({ label: 'Reveal', onClick: () => invoke('reveal_path', { path: t.dest }).catch(() => {}) });
      const row = actRow({
        dir: dir === 'send' ? 'send' : 'recv',
        name: t.name || 'transfer',
        meta: `${fmtBytes(t.total_bytes)} · ${dir === 'send' ? 'sent' : 'received'}`,
        pct: null,
        pill: STATUS_LABEL[t.status] || t.status || '',
        pillClass: STATUS_PILL[t.status] || '',
        actions,
      });
      list.appendChild(row);
      if (canAnim) row.animate([{ opacity: 0, transform: 'translateY(8px)' }, { opacity: 1, transform: 'none' }], { duration: 220, delay: Math.min(i, 6) * 40, easing: EASE_OUT });
    });
  }
  updateActivityEmpty();
}

/* ============================== settings bits ============================ */
$('#copy-id').addEventListener('click', () => {
  const id = $('#endpoint-id').textContent;
  if (navigator.clipboard && id) {
    navigator.clipboard.writeText(id);
    const b = $('#copy-id'); b.textContent = 'Copied ✓'; setTimeout(() => { b.textContent = 'Copy'; }, 1400);
  }
});
$('#change-folder').addEventListener('click', async () => {
  const dir = await invoke('pick_dest_dir').catch(() => null);
  if (dir) {
    localStorage.setItem('dropwire-default-dir', dir);
    const l = $('#default-folder-label'); l.textContent = dir; l.title = dir;
    recvDest = dir; setDestLabel(dir);
  }
});
document.addEventListener('click', (e) => {
  const a = e.target.closest('.js-ext');
  if (!a) return;
  e.preventDefault();
  invoke('open_external', { url: a.dataset.url }).catch(() => {});
});

/* ======================= drag & drop (real fs paths) ===================== */
if (HAS_TAURI && TAURI.webview && TAURI.webview.getCurrentWebview) {
  TAURI.webview.getCurrentWebview().onDragDropEvent((event) => {
    const dz = $('#send-pick'); const t = event.payload.type;
    if (t === 'enter' || t === 'over') { if (curPanel !== 'send') showPanel('send'); dz.classList.add('dragover'); }
    else if (t === 'leave') dz.classList.remove('dragover');
    else if (t === 'drop') {
      dz.classList.remove('dragover');
      const p = event.payload.paths || [];
      if (p.length) {
        if (curPanel !== 'send') showPanel('send');
        startSend(p[0]);
      }
    }
  }).catch(() => {});
}

/* ===================== NEARBY: discovery + two-sided consent =============
 * Being discoverable never means being reachable: every transfer needs BOTH
 * sides to confirm. With the toggle off, this device is invisible.
 * ------------------------------------------------------------------------ */
const nearby = {
  on: localStorage.getItem('dropwire-nearby') !== 'off',
  started: false,
  devices: new Map(),
  offers: new Map(),
  queue: [],
  offering: false,
  offerTimer: null,
  countdown: null,
  poll: null,
};
const OS_LABEL = { macos: 'macOS', windows: 'Windows', linux: 'Linux', android: 'Android', ios: 'iOS' };
const OS_ICON = {
  macos: '<svg viewBox="0 0 24 24" width="17" height="17"><rect x="3" y="5" width="18" height="12" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M8 20h8" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>',
  windows: '<svg viewBox="0 0 24 24" width="17" height="17"><rect x="3" y="4" width="18" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M9 21h6" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>',
  android: '<svg viewBox="0 0 24 24" width="17" height="17"><rect x="6" y="3" width="12" height="18" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>',
  ios: '<svg viewBox="0 0 24 24" width="17" height="17"><rect x="6" y="3" width="12" height="18" rx="3" fill="none" stroke="currentColor" stroke-width="1.7"/></svg>',
  linux: '<svg viewBox="0 0 24 24" width="17" height="17"><rect x="3" y="4" width="18" height="14" rx="2" fill="none" stroke="currentColor" stroke-width="1.7"/><path d="M9 21h6" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"/></svg>',
};
function nearbySetSwitch(on) {
  [$('#nearby-toggle'), $('#nearby-toggle-2')].forEach((t) => { if (t) t.setAttribute('aria-checked', on ? 'true' : 'false'); });
  const r = $('#nearby-radar'); if (r) r.classList.toggle('on', on);
}
function nearbyStatus(text) { const el = $('#nearby-status'); if (el) el.textContent = text; }
function deviceRow(d) {
  const row = document.getElementById('tpl-device').content.firstElementChild.cloneNode(true);
  row.dataset.eid = d.endpointId;
  const name = d.name || 'Dropwire device';
  const n = row.querySelector('.js-name'); n.textContent = name; n.title = name;
  const os = (d.os || '').toLowerCase();
  if (OS_LABEL[os]) {
    const b = row.querySelector('.js-os');
    b.textContent = OS_LABEL[os]; b.classList.remove('hidden');
    row.querySelector('.js-icon').innerHTML = OS_ICON[os];
  }
  const fp = row.querySelector('.js-fp');
  fp.textContent = 'pairing code ' + (d.fingerprint || '—');
  fp.title = 'Pairing code for ' + name + ': ' + (d.fingerprint || '—') + ' — compare it with the code shown on that device before accepting';
  row.querySelector('.js-send').addEventListener('click', () => offerToDevice(d, row));
  return row;
}
function renderDevices() {
  const list = $('#nearby-devices');
  if (!list) return;
  const seen = new Set();
  for (const d of nearby.devices.values()) {
    seen.add(d.endpointId);
    if (!list.querySelector('[data-eid="' + CSS.escape(d.endpointId) + '"]')) list.appendChild(deviceRow(d));
  }
  list.querySelectorAll('.device').forEach((row) => { if (!seen.has(row.dataset.eid)) row.remove(); });
  let empty = list.querySelector('.device-empty');
  if (!seen.size) {
    if (!empty) {
      empty = document.createElement('div');
      empty.className = 'device-empty';
      list.appendChild(empty);
    }
    empty.textContent = nearby.on
      ? 'No devices yet. Both machines need Dropwire open on the same network.'
      : 'You are invisible. Turn sharing on to appear to nearby devices.';
  } else if (empty) empty.remove();
}
async function nearbyPoll() {
  try {
    const devs = await invoke('nearby_list');
    nearby.devices.clear();
    for (const d of devs || []) nearby.devices.set(d.endpointId, d);
    renderDevices();
    const n = nearby.devices.size;
    nearbyStatus(!n ? 'Looking on this network…' : (n === 1 ? '1 on this network' : n + ' on this network'));
  } catch (_) { renderDevices(); /* engine not reachable: still show the empty state */ }
}
async function nearbyStart() {
  if (nearby.started) return;
  nearby.started = true;
  nearbySetSwitch(true);
  try { await invoke('nearby_start'); } catch (_) {}
  if (!nearby.started || !nearby.on) return;
  await nearbyPoll();
  if (!nearby.started || !nearby.on) return;
  if (nearby.poll) clearInterval(nearby.poll);
  nearby.poll = setInterval(nearbyPoll, 2500);
}
function nearbyStop() {
  nearbySetSwitch(false);
  if (nearby.poll) { clearInterval(nearby.poll); nearby.poll = null; }
  nearby.devices.clear(); renderDevices();
  nearbyStatus('Sharing is off, this device is invisible.');
  if (nearby.started) invoke('nearby_stop').catch(() => {});
  nearby.started = false;
}
function toggleNearby() {
  nearby.on = !nearby.on;
  localStorage.setItem('dropwire-nearby', nearby.on ? 'on' : 'off');
  if (nearby.on) { showPanel('send'); nearbyStart(); } else nearbyStop();
}

/* ---- outgoing: tap a device to offer the live send ---- */
function setOffering(on) {
  nearby.offering = on;
  const list = $('#nearby-devices');
  if (list) list.classList.toggle('offering', on);
}
function offerToDevice(d, row) {
  if (!liveSend) {
    showPanel('send');
    nearbyStatus('Pick something to send first, then tap a device.');
    return;
  }
  if (nearby.offering) {
    nearbyStatus('Waiting on the last offer, cancel it first to pick another device.');
    return;
  }
  const card = liveSend.card, els = liveSend.els;
  setOffering(true);
  row && row.classList.add('busy');
  els.status.textContent = 'Asking ' + (d.name || 'device') + '…, waiting for them to accept.';
  const done = () => { setOffering(false); row && row.classList.remove('busy'); };
  const ch = makeChannel();
  ch.onmessage = (u) => {
    switch (u.kind) {
      case 'waiting': break;
      case 'accepted':
        done();
        els.status.textContent = (d.name || 'They') + ' accepted, sending…';
        if (els.svg && !els.svg.dataset.lit) { els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting'); igniteNode(els.svg, '.w-node.peer'); }
        break;
      case 'declined':
        done();
        els.status.setAttribute('aria-live', 'assertive');
        els.status.textContent = (d.name || 'They') + ' declined.';
        if (liveSend && liveSend.card === card) liveSend = null;
        break;
      case 'failed':
        done();
        els.status.setAttribute('aria-live', 'assertive');
        els.status.textContent = 'Could not reach ' + (d.name || 'them') + ' — ' + (u.reason || 'try again') + '.';
        if (liveSend && liveSend.card === card) liveSend = null;
        break;
    }
  };
  invoke('nearby_offer', { endpointId: d.endpointId, onUpdate: ch }).catch((e) => {
    done();
    els.status.textContent = 'Could not start the nearby offer.';
    console.warn(e);
  });
}

/* ---- incoming: the consent dialog ---- */
let offerState = null, offerLastFocus = null;
const OFFER_TTL_MS = 112000; // just inside the sender's 115s self-decline
function anyModalOpen() {
  return !$('#nearby-offer-modal').classList.contains('hidden')
    || !$('#recv-preview').classList.contains('hidden')
    || !$('#sheet-settings').classList.contains('hidden');
}
function enqueueOffer(offer) {
  if (!offer || !offer.offerId) return;
  if (nearby.offers.has(offer.offerId)) return;
  if (nearby.queue.some((o) => o.offerId === offer.offerId)) return;
  if (offerState || anyModalOpen()) { nearby.queue.push(offer); return; }
  showOfferModal(offer);
}
function showNextOffer() {
  if (offerState || anyModalOpen()) return;
  const next = nearby.queue.shift();
  if (next) showOfferModal(next);
}
function showOfferModal(offer) {
  nearby.offers.set(offer.offerId, offer);
  offerState = offer;
  offerLastFocus = document.activeElement;
  $('#offer-device').textContent = offer.deviceName || 'A nearby device';
  $('#offer-count').textContent = offer.fileCount + ' file' + (offer.fileCount === 1 ? '' : 's');
  $('#offer-size').textContent = fmtBytes(offer.totalBytes) + ' · you will see the full list before saving';
  $('#offer-fp').textContent = offer.fingerprint || '···';
  $('#offer-note').textContent = 'Nothing is received until you accept. Declining tells them instantly.';
  const acceptBtn = $('#offer-accept'); acceptBtn.disabled = false; acceptBtn.textContent = 'Accept & preview';
  invoke('my_fingerprint').then((fp) => { $('#offer-my-fp').textContent = fp; }).catch(() => {});
  $('#nearby-offer-modal').classList.remove('hidden');
  if (nearby.offerTimer) clearTimeout(nearby.offerTimer);
  nearby.offerTimer = setTimeout(() => expireOfferModal(offer.offerId), OFFER_TTL_MS);
  // "Auto-declines in m:ss" — say what will happen, not just that time passes.
  let left = Math.round(OFFER_TTL_MS / 1000);
  const tick = () => {
    if (!offerState || offerState.offerId !== offer.offerId) return;
    const m = Math.floor(left / 60), s = String(left % 60).padStart(2, '0');
    $('#offer-countdown').textContent = `Auto-declines in ${m}:${s}`;
    left--;
  };
  tick();
  if (nearby.countdown) clearInterval(nearby.countdown);
  nearby.countdown = setInterval(tick, 1000);
  $('#offer-decline').focus();
}
function expireOfferModal(offerId) {
  if (!offerState || offerState.offerId !== offerId) return;
  $('#offer-note').textContent = 'This offer expired, ask them to send again.';
  $('#offer-countdown').textContent = '';
  $('#offer-accept').disabled = true;
  setTimeout(() => { if (offerState && offerState.offerId === offerId) closeOfferModal(); }, 1600);
}
function closeOfferModal(showNext = true) {
  if (nearby.offerTimer) { clearTimeout(nearby.offerTimer); nearby.offerTimer = null; }
  if (nearby.countdown) { clearInterval(nearby.countdown); nearby.countdown = null; }
  if (offerState) nearby.offers.delete(offerState.offerId);
  $('#nearby-offer-modal').classList.add('hidden');
  $('#offer-countdown').textContent = '';
  offerState = null;
  if (offerLastFocus && offerLastFocus.focus) offerLastFocus.focus();
  if (showNext) showNextOffer();
}
$('#offer-accept').addEventListener('click', async () => {
  const offer = offerState; if (!offer) return;
  const acceptBtn = $('#offer-accept');
  acceptBtn.disabled = true; acceptBtn.textContent = 'Connecting…';
  let ok = true;
  try { await invoke('nearby_respond', { offerId: offer.offerId, accept: true }); } catch (_) { ok = false; }
  const ticket = offer.ticket;
  if (!ok) { acceptBtn.textContent = 'Accept & preview'; expireOfferModal(offer.offerId); return; }
  closeOfferModal(false);
  // Same verified preview as the code flow: names and sizes come from the
  // manifest the code commits to, not from the sender's claim in the offer.
  openPreview(ticket, recvDest || localStorage.getItem('dropwire-default-dir') || null);
});
$('#offer-decline').addEventListener('click', async () => {
  const offer = offerState; if (!offer) { closeOfferModal(); return; }
  try { await invoke('nearby_respond', { offerId: offer.offerId, accept: false }); } catch (_) {}
  closeOfferModal();
});
$('#nearby-offer-modal').addEventListener('click', (e) => { if (e.target.id === 'nearby-offer-modal') closeOfferModal(); });

/* Escape closes the topmost overlay and returns focus to its trigger. */
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (!$('#nearby-offer-modal').classList.contains('hidden')) return closeOfferModal();
  if (!$('#recv-preview').classList.contains('hidden')) return closePreview();
  if (!$('#sheet-settings').classList.contains('hidden')) return closeSettings();
});

[$('#nearby-toggle'), $('#nearby-toggle-2')].forEach((t) => { if (t) t.addEventListener('click', toggleNearby); });
if (HAS_TAURI && TAURI.event && TAURI.event.listen) {
  TAURI.event.listen('nearby-offer', (ev) => enqueueOffer(ev.payload)).catch(() => {});
}

/* ================================= init ================================== */
(async function init() {
  const dd = localStorage.getItem('dropwire-default-dir');
  if (dd) { const l = $('#default-folder-label'); l.textContent = dd; l.title = dd; recvDest = dd; setDestLabel(dd); }
  try { DEFAULT_DEST = await invoke('default_dest_dir'); if (!dd) { $('#default-folder-label').textContent = DEFAULT_DEST; setDestLabel(DEFAULT_DEST); } } catch (_) {}
  try {
    const v = await invoke('app_version');
    if (v) { $('#app-version').textContent = 'v' + v; $('#app-version-2').textContent = 'v' + v; }
  } catch (_) {}
  try {
    const eid = await invoke('my_endpoint_id');
    const el = $('#endpoint-id'); el.textContent = eid; el.title = eid;
    setStatus('discovery up, direct capable', 'up');
  } catch (_) {
    $('#endpoint-id').textContent = HAS_TAURI ? '(starting…)' : '(preview — run inside the app)';
    setStatus(HAS_TAURI ? 'starting…' : 'preview mode', '');
  }
  try { const n = await invoke('device_name'); if (n) { $('#device-name').textContent = n; $('#set-device-name').textContent = n; } } catch (_) {}
  try { const fp = await invoke('my_fingerprint'); if (fp) $('#set-fp').textContent = fp; } catch (_) {}

  if (nearby.on) nearbyStart();
  else { nearbySetSwitch(false); nearbyStatus('Sharing is off, this device is invisible.'); renderDevices(); }
  renderDevices();
  renderActivityLive();
  loadHistory();
})();
