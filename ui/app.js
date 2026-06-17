/* Dropwire desktop app — UI logic ("The Wire is alive" redesign).
 *
 * Command contract (unchanged): my_endpoint_id, pick_paths, pick_dest_dir, qr_svg,
 * start_send, start_receive, cancel_transfer, list_transfers, reveal_path.
 * Progress events: { kind: importing|ready|peerJoined|transferring|done|error|cancelled, ... }
 * All visual enhancements are additive + reduced-motion gated.
 */

const TAURI = window.__TAURI__;
const HAS_TAURI = !!(TAURI && TAURI.core);
const invoke = HAS_TAURI ? TAURI.core.invoke : async () => { throw new Error("Run inside the Dropwire app."); };
const makeChannel = () => (HAS_TAURI ? new TAURI.core.Channel() : { onmessage: null });
const RM = matchMedia('(prefers-reduced-motion: reduce)').matches;
const canAnim = !RM && typeof Element.prototype.animate === 'function';
const EASE_OUT = 'cubic-bezier(.22,1,.36,1)';
const EASE_POP = 'cubic-bezier(.34,1.2,.5,1)';

const $ = (s) => document.querySelector(s);
const $$ = (s) => [...document.querySelectorAll(s)];

function fmtBytes(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${u[i]}`;
}

/* ----------------------- navigation + view transitions ----------------------- */
let curView = 'send';
function moveIndicator(btn) {
  const ind = $('.nav-indicator');
  if (!ind || !btn) return;
  const top = btn.offsetTop + btn.offsetHeight / 2 - 10; // 20px bar centered on icon
  ind.style.transform = `translateY(${top}px)`;
}
function applyView(view) {
  $$('.nav-item').forEach((b) => {
    const on = b.dataset.view === view;
    b.classList.toggle('is-active', on);
    if (on) { b.setAttribute('aria-current', 'page'); moveIndicator(b); }
    else b.removeAttribute('aria-current');
  });
  $$('.view').forEach((v) => v.classList.toggle('is-active', v.id === 'view-' + view));
  curView = view;
  if (view === 'history') loadHistory();
}
function switchView(view) {
  if (view === curView) return;
  const btn = $$('.nav-item').find((b) => b.dataset.view === view);
  moveIndicator(btn); // indicator leads
  if (canAnim && document.startViewTransition) document.startViewTransition(() => applyView(view));
  else applyView(view);
}
$$('.nav-item').forEach((b) => b.addEventListener('click', () => switchView(b.dataset.view)));

/* -------------------------------- theme -------------------------------- */
function applyTheme(mode) {
  if (mode === 'auto') document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', mode);
  $$('.seg-btn').forEach((b) => {
    const on = b.dataset.themeSet === mode;
    b.classList.toggle('on', on);
    b.setAttribute('aria-checked', on ? 'true' : 'false');
  });
  syncThemeToggleLabel();
}
// Keep the rail theme-toggle's label describing the action it will perform.
function syncThemeToggleLabel() {
  const attr = document.documentElement.getAttribute('data-theme');
  const isDark = attr === 'dark' || (!attr && matchMedia('(prefers-color-scheme: dark)').matches);
  const tt = $('#theme-toggle');
  if (tt) tt.setAttribute('aria-label', isDark ? 'Switch to light theme' : 'Switch to dark theme');
}
(function initTheme() {
  applyTheme(localStorage.getItem('dropwire-theme') || 'auto');
  $$('.seg-btn').forEach((b) => b.addEventListener('click', () => {
    const m = b.dataset.themeSet; localStorage.setItem('dropwire-theme', m); applyTheme(m);
  }));
  $('#theme-toggle').addEventListener('click', () => {
    const attr = document.documentElement.getAttribute('data-theme');
    const isDark = attr === 'dark' || (!attr && matchMedia('(prefers-color-scheme: dark)').matches);
    const m = isDark ? 'light' : 'dark'; localStorage.setItem('dropwire-theme', m); applyTheme(m);
  });
})();

/* --------------------------- the wire progress -------------------------- */
// A bright pulse races across the wire (the "connection established" moment).
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
    if (canAnim) node.animate([{ opacity: 0.3 }, { opacity: 1 }], { duration: 380, easing: EASE_OUT });
  }
  firePulse(svg);
}
function doneSpark(svg) {
  if (!svg) return;
  svg.classList.add('done');
  svg.querySelectorAll('.w-node').forEach((n) => n.classList.add('done', 'live'));
  if (!canAnim) return;
  const spark = svg.querySelector('.w-spark');
  if (spark) {
    spark.style.stroke = 'var(--success)';
    spark.animate([{ r: '8px', opacity: 0.9 }, { r: '34px', opacity: 0 }], { duration: 640, easing: EASE_OUT });
  }
}
function resetWire(svg) {
  if (!svg) return;
  svg.classList.add('connecting'); svg.classList.remove('done'); delete svg.dataset.lit;
  const fill = svg.querySelector('.w-fill'); if (fill) fill.style.strokeDashoffset = '1';
  const clip = svg.querySelector('.lit-clip'); if (clip) clip.setAttribute('width', '60');
  svg.querySelectorAll('.w-node').forEach((n) => n.classList.remove('live', 'done'));
}

/* ----------------- transfer cards (shared, multi-transfer) ------------- */
// Clone a card template, giving its clipPath a unique id so multiple live cards
// don't share one clip region.
function uniquifyClip(card) {
  const cp = card.querySelector('clipPath');
  if (!cp) return;
  const oldRef = `url(#${cp.id})`;
  const nid = 'lit-' + Math.random().toString(36).slice(2, 9);
  cp.id = nid;
  card.querySelectorAll('[clip-path]').forEach((g) => {
    if (g.getAttribute('clip-path') === oldRef) g.setAttribute('clip-path', `url(#${nid})`);
  });
}
function setBarEl(fill, svg, pctEl, done, total) {
  const p = total > 0 ? Math.min(1, done / total) : 0;
  if (fill) {
    fill.style.strokeDashoffset = String(1 - p);
    const clip = svg && svg.querySelector('.lit-clip');
    if (clip) clip.setAttribute('width', String(60 + p * 480));
  }
  if (pctEl) pctEl.textContent = Math.round(p * 100) + '%';
}
// Set a route badge's text / color / aria from a route value ('direct' | 'relayed' | else).
function setRouteBadge(el, route) {
  if (!el || !route) return;
  const label = route === 'direct' ? 'direct' : route === 'relayed' ? 'relayed · a bit slower' : 'connected';
  el.textContent = label;
  el.className = 'route-badge ' + (route === 'direct' ? 'direct' : route === 'relayed' ? 'relayed' : '');
  el.setAttribute('aria-label', 'Connection: ' + label);
}
function removeCard(card) {
  if (!card) return;
  if (canAnim) {
    const a = card.animate([{ opacity: 1 }, { opacity: 0, transform: 'translateY(-6px)' }], { duration: 200, easing: EASE_OUT });
    a.onfinish = () => card.remove();
  } else card.remove();
}
function makeCard(tplId, listId) {
  const card = document.getElementById(tplId).content.firstElementChild.cloneNode(true);
  uniquifyClip(card);
  document.getElementById(listId).prepend(card);
  if (canAnim) card.animate([{ opacity: 0, transform: 'translateY(12px) scale(.97)' }, { opacity: 1, transform: 'none' }], { duration: 380, easing: EASE_POP });
  return card;
}

/* -------------------------------- SEND --------------------------------- */
async function startSend(path) {
  const card = makeCard('tpl-send', 'send-list');
  const els = {
    code: card.querySelector('.js-code'), qr: card.querySelector('.js-qr'),
    status: card.querySelector('.js-status'), copy: card.querySelector('.js-copy'),
    cancel: card.querySelector('.js-cancel'), fill: card.querySelector('.w-fill'),
    pct: card.querySelector('.wire-pct'), svg: card.querySelector('svg.wire'),
    route: card.querySelector('.route-badge'),
  };
  els.code.textContent = '…'; els.status.textContent = 'Preparing…';
  let id = null;
  els.copy.addEventListener('click', () => {
    const c = els.code.textContent;
    if (c && c !== '…' && navigator.clipboard) {
      navigator.clipboard.writeText(c);
      els.copy.textContent = 'Copied ✓'; setTimeout(() => { els.copy.textContent = 'Copy code'; }, 1400);
    }
  });
  els.cancel.addEventListener('click', async () => {
    if (id) await invoke('cancel_transfer', { id }).catch(() => {});
    removeCard(card);
  });
  try {
    const ch = makeChannel(); ch.onmessage = (m) => onSendMsg(m, els, card);
    id = await invoke('start_send', { path, onEvent: ch });
  } catch (e) { els.status.textContent = "Couldn't start the transfer. Try again."; console.warn(e); }
}
function onSendMsg(m, els, card) {
  switch (m.kind) {
    case 'importing': els.status.textContent = `Preparing… ${fmtBytes(m.done)} / ${fmtBytes(m.total)}`; break;
    case 'ready':
      els.code.textContent = m.ticket;
      els.status.textContent = "Ready — share this code. Keep the app open until it's received.";
      invoke('qr_svg', { text: m.ticket }).then((svg) => {
        els.qr.innerHTML = svg;
        els.qr.setAttribute('role', 'img');
        els.qr.setAttribute('aria-label', 'QR code — scan with the receiving device');
        if (canAnim) els.qr.animate([{ opacity: 0, transform: 'scale(.96)' }, { opacity: 1, transform: 'none' }], { duration: 380, easing: EASE_OUT, delay: 80 });
      }).catch(() => {});
      if (canAnim) els.code.animate([{ clipPath: 'inset(0 100% 0 0)' }, { clipPath: 'inset(0 0 0 0)' }], { duration: 380, easing: EASE_OUT });
      break;
    case 'peerJoined':
      if (els.svg && !els.svg.dataset.lit) { els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting'); igniteNode(els.svg, '.w-node.peer'); }
      els.status.textContent = 'Receiver connected — sending…';
      break;
    case 'transferring':
      setBarEl(els.fill, els.svg, els.pct, m.offset, m.total);
      setRouteBadge(els.route, m.route);
      els.status.textContent = `Sending… ${fmtBytes(m.offset)} / ${fmtBytes(m.total)}`;
      break;
    case 'done':
      setBarEl(els.fill, els.svg, els.pct, 1, 1);
      if (els.svg) doneSpark(els.svg);
      els.status.textContent = 'Sent ✓'; els.cancel.textContent = 'Dismiss';
      break;
    case 'error':
      els.status.setAttribute('aria-live', 'assertive');
      els.status.textContent = m.message ? `Couldn't send — ${m.message}` : "Couldn't send.";
      break;
    case 'cancelled': removeCard(card); break;
  }
}
$('#pick-file').addEventListener('click', async () => {
  const p = await invoke('pick_paths', { directory: false, multiple: false }).catch(() => []);
  if (p && p.length) startSend(p[0]);
});
$('#pick-folder').addEventListener('click', async () => {
  const p = await invoke('pick_paths', { directory: true, multiple: false }).catch(() => []);
  if (p && p.length) startSend(p[0]);
});

/* ------------------------------- RECEIVE ------------------------------- */
let recvDest = null;
let DEFAULT_DEST = null; // engine's default save folder (Downloads/Dropwire), fetched at init
$('#recv-code-input').addEventListener('input', (e) => {
  const v = e.target.value.trim();
  $('#recv-start').disabled = v.length === 0;
  const glyph = $('.code-input-glyph circle'); if (glyph) glyph.setAttribute('fill', v ? 'var(--wire)' : 'var(--wire-dim)');
});
$('#recv-code-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') { e.preventDefault(); if (!$('#recv-start').disabled) $('#recv-start').click(); }
});
$('#pick-dest').addEventListener('click', async () => {
  const dir = await invoke('pick_dest_dir').catch(() => null);
  if (dir) { recvDest = dir; const l = $('#recv-dest-label'); l.innerHTML = 'Save to: <em>' + esc(dir) + '</em>'; l.title = dir; }
});
async function beginReceive(ticket, dest, selected) {
  $('#recv-error').textContent = '';
  switchView('receive');
  const myDest = dest || DEFAULT_DEST;
  const card = makeCard('tpl-recv', 'recv-list');
  const els = {
    name: card.querySelector('.js-name'), route: card.querySelector('.route-badge'),
    fill: card.querySelector('.w-fill'), pct: card.querySelector('.wire-pct'),
    status: card.querySelector('.js-status'), svg: card.querySelector('svg.wire'),
    open: card.querySelector('.js-open'), another: card.querySelector('.js-another'),
    cancel: card.querySelector('.js-cancel'),
  };
  els.name.textContent = 'Connecting…';
  let id = null;
  els.cancel.addEventListener('click', async () => { if (id) await invoke('cancel_transfer', { id }).catch(() => {}); removeCard(card); });
  els.open.addEventListener('click', async () => { if (myDest) await invoke('reveal_path', { path: myDest }).catch(() => {}); });
  els.another.addEventListener('click', () => removeCard(card));
  try {
    const ch = makeChannel(); ch.onmessage = (m) => onRecvMsg(m, els, card);
    id = (selected && selected.length)
      ? await invoke('start_receive_selected', { ticket, dest: myDest, selected, onEvent: ch })
      : await invoke('start_receive', { ticket, dest: myDest, onEvent: ch });
  } catch (e) {
    removeCard(card);
    $('#recv-error').textContent = "That code doesn't look right. Check it and try again.";
    console.warn(e);
    const input = $('#recv-code-input'); if (input) { input.classList.add('shake'); setTimeout(() => input.classList.remove('shake'), 400); }
  }
}
$('#recv-start').addEventListener('click', () => {
  const ticket = $('#recv-code-input').value.trim();
  if (!ticket) return;
  const dest = recvDest || localStorage.getItem('dropwire-default-dir') || null;
  openPreview(ticket, dest);
});

/* ---- preview / accept modal: see exactly what's coming before downloading ---- */
let previewTicket = null, previewDest = null, lastFocus = null, previewFileCount = 0, previewLoaded = false;
function showModal() {
  const scrim = $('#recv-preview');
  lastFocus = document.activeElement;
  scrim.classList.remove('hidden');
  const app = document.querySelector('.app'); if (app) app.setAttribute('inert', '');
  const sheet = scrim.querySelector('.modal-sheet');
  // Trap Tab inside the sheet (focusable set changes as Accept enables / files load).
  scrim._trap = (e) => {
    if (e.key !== 'Tab') return;
    const f = [...sheet.querySelectorAll('button:not([disabled]),input,[href],[tabindex]:not([tabindex="-1"])')]
      .filter((el) => el.offsetParent !== null);
    if (!f.length) return;
    const first = f[0], last = f[f.length - 1];
    if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
    else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
  };
  document.addEventListener('keydown', scrim._trap);
  if (canAnim) {
    scrim.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 160, easing: EASE_OUT });
    sheet.animate([{ opacity: 0, transform: 'translateY(12px) scale(.97)' }, { opacity: 1, transform: 'none' }], { duration: 240, easing: EASE_POP });
  }
}
function closeModal() {
  const scrim = $('#recv-preview');
  scrim.classList.add('hidden');
  if (scrim._trap) { document.removeEventListener('keydown', scrim._trap); scrim._trap = null; }
  const app = document.querySelector('.app'); if (app) app.removeAttribute('inert'); // before focus restore
  previewTicket = null; previewDest = null; previewLoaded = false;
  if (lastFocus && lastFocus.focus) lastFocus.focus();
}
function checkedIndices() {
  return [...document.querySelectorAll('#preview-files .file-check')]
    .filter((c) => c.checked)
    .map((c) => Number(c.dataset.index));
}
function updateAcceptState() {
  $('#preview-accept').disabled = checkedIndices().length === 0;
}
function fillPreview(p) {
  const rb = $('#preview-route');
  setRouteBadge(rb, p.route);
  previewFileCount = (p.files || []).length;
  // Route folded into the aria-live summary so it's announced (the badge isn't live).
  $('#preview-summary').textContent = `${previewFileCount} file${previewFileCount === 1 ? '' : 's'} · ${fmtBytes(p.totalBytes)} · ${rb.textContent}`;
  const ul = $('#preview-files'); ul.innerHTML = '';
  (p.files || []).forEach((f, i) => {
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
async function openPreview(ticket, dest) {
  previewTicket = ticket; previewDest = dest;
  $('#recv-error').textContent = '';
  $('#preview-summary').textContent = 'Connecting to sender to preview…';
  $('#preview-files').innerHTML = '';
  const rb = $('#preview-route'); rb.textContent = 'connecting'; rb.className = 'route-badge connecting';
  const accept = $('#preview-accept'); accept.disabled = true; accept.classList.remove('hidden');
  const decline = $('#preview-decline'); decline.textContent = 'Cancel';
  $('#preview-note').textContent = 'Nothing is saved until you accept.';
  showModal();
  decline.focus();
  try {
    const p = await invoke('inspect_ticket', { ticket });
    if (previewTicket !== ticket) return; // modal closed/replaced while inspecting
    fillPreview(p);
    previewLoaded = true;
    decline.textContent = 'Decline';
    accept.focus();
  } catch (e) {
    if (previewTicket !== ticket) return;
    $('#preview-summary').textContent = 'Sender offline, or this code has expired.';
    $('#preview-note').textContent = "We couldn't reach the sender. They need to be online — and the code still valid — for the transfer to start.";
    accept.classList.add('hidden');
    decline.textContent = 'Close';
  }
}
$('#preview-accept').addEventListener('click', () => {
  const t = previewTicket, d = previewDest, total = previewFileCount;
  const idx = checkedIndices();
  closeModal();
  if (!t) return;
  // Pass a selection only when it's a strict subset; otherwise download everything.
  const selected = (idx.length > 0 && idx.length < total) ? idx : null;
  beginReceive(t, d, selected);
});
$('#preview-decline').addEventListener('click', () => {
  // If we got a preview, tell the sender "no" instantly over the control channel.
  if (previewTicket && previewLoaded) {
    invoke('send_control', { ticket: previewTicket, kind: 'decline' }).catch(() => {});
  }
  closeModal();
});
$('#recv-preview').addEventListener('click', (e) => { if (e.target.id === 'recv-preview') closeModal(); });
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && !$('#recv-preview').classList.contains('hidden')) closeModal();
});
function onRecvMsg(m, els, card) {
  switch (m.kind) {
    case 'transferring': {
      if (els.svg && !els.svg.dataset.lit) { els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting'); igniteNode(els.svg, '.w-node.peer'); els.name.textContent = 'Receiving…'; }
      setRouteBadge(els.route, m.route);
      setBarEl(els.fill, els.svg, els.pct, m.offset, m.total);
      els.status.textContent = `${fmtBytes(m.offset)} / ${fmtBytes(m.total)}`;
      break;
    }
    case 'done':
      if (els.svg && !els.svg.dataset.lit) { els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting'); igniteNode(els.svg, '.w-node.peer'); }
      setBarEl(els.fill, els.svg, els.pct, 1, 1);
      if (els.svg) doneSpark(els.svg);
      els.name.textContent = 'Received ✓';
      els.status.textContent = `${fmtBytes(m.stats && m.stats.bytes)} in ${((m.stats && m.stats.seconds) || 0).toFixed(1)}s`;
      els.open.classList.remove('hidden'); els.another.classList.remove('hidden'); els.cancel.classList.add('hidden');
      if (canAnim) els.open.animate([{ opacity: 0, transform: 'translateY(8px) scale(.94)' }, { opacity: 1, transform: 'none' }], { duration: 380, easing: EASE_POP });
      break;
    case 'error': {
      const offline = /reach the sender|offline|link expired|unreachable/i.test(m.message || '');
      els.status.setAttribute('aria-live', 'assertive');
      els.name.textContent = offline ? 'Sender offline or link expired' : 'Failed';
      els.status.textContent = offline ? 'Ask for a fresh code and try again.' : (m.message || '');
      els.cancel.textContent = 'Dismiss';
      break;
    }
    case 'cancelled': removeCard(card); break;
  }
}
if ($('#copy-id')) $('#copy-id').addEventListener('click', () => {
  const id = $('#endpoint-id').textContent;
  if (navigator.clipboard && id) {
    navigator.clipboard.writeText(id);
    const b = $('#copy-id'); b.textContent = 'Copied ✓'; setTimeout(() => { b.textContent = 'Copy'; }, 1400);
  }
});
if ($('#change-folder')) $('#change-folder').addEventListener('click', async () => { const dir = await invoke('pick_dest_dir').catch(() => null); if (dir) { localStorage.setItem('dropwire-default-dir', dir); const l = $('#default-folder-label'); l.textContent = dir; l.title = dir; } });
// External links (credit + About): open in the real browser, never inside the webview.
document.addEventListener('click', (e) => {
  const a = e.target.closest('.js-ext');
  if (!a) return;
  e.preventDefault();
  invoke('open_external', { url: a.dataset.url }).catch(() => {});
});

/* ------------------------------- HISTORY ------------------------------- */
function histGlyph(dir) {
  const send = dir === 'send';
  const lx = send ? 'var(--wire)' : 'var(--text-faint)';
  const rx = send ? 'var(--text-faint)' : 'var(--wire)';
  return `<svg class="hist-dir" viewBox="0 0 40 24" aria-hidden="true"><path d="M6 12 H34" stroke="var(--wire-dim)" stroke-width="3" stroke-linecap="round"/><circle cx="6" cy="12" r="4" fill="${lx}"/><circle cx="34" cy="12" r="3.5" fill="${rx}"/></svg>`;
}
const STATUS_LABEL = { active: 'In progress', done: 'Done', error: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted' };
async function loadHistory() {
  const list = $('#history-list'), empty = $('#history-empty');
  let items = [];
  try { items = await invoke('list_transfers'); } catch (_) {}
  list.innerHTML = '';
  if (!items || !items.length) { empty.classList.remove('hidden'); return; }
  empty.classList.add('hidden');
  items.forEach((t, i) => {
    const dir = (t.direction || '').toLowerCase();
    const resumable = dir === 'receive' && t.status === 'interrupted' && t.ticket && t.dest;
    const resendable = dir === 'send' && !!t.source;
    const el = document.createElement('div');
    el.className = 'hist-item';
    let right;
    if (resumable) right = `<button class="btn-quiet sm" data-resume="1">Resume</button>`;
    else if (resendable) right = `<button class="btn-quiet sm" data-resend="1">Resend</button>`;
    else right = `<div class="hist-meta">${dir === 'send' ? 'Sent' : 'Received'}</div>`;
    el.innerHTML = `${histGlyph(dir)}<div><div class="hist-name">${esc(t.name || 'transfer')}</div><div class="hist-meta">${fmtBytes(t.total_bytes)} · ${STATUS_LABEL[t.status] || esc(t.status || '')}</div></div>${right}`;
    if (resumable) el.querySelector('[data-resume]').addEventListener('click', () => beginReceive(t.ticket, t.dest));
    if (resendable) el.querySelector('[data-resend]').addEventListener('click', () => { switchView('send'); startSend(t.source); });
    list.appendChild(el);
    if (canAnim) el.animate([{ opacity: 0, transform: 'translateY(8px)' }, { opacity: 1, transform: 'none' }], { duration: 220, delay: Math.min(i, 6) * 40, easing: EASE_OUT });
  });
}
function esc(s) { return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }

/* ---------------------- drag & drop (real fs paths) -------------------- */
if (HAS_TAURI && TAURI.webview && TAURI.webview.getCurrentWebview) {
  TAURI.webview.getCurrentWebview().onDragDropEvent((event) => {
    const dz = $('#send-pick'); const t = event.payload.type;
    if (t === 'enter' || t === 'over') dz.classList.add('dragover');
    else if (t === 'leave') dz.classList.remove('dragover');
    else if (t === 'drop') {
      dz.classList.remove('dragover');
      const p = event.payload.paths || [];
      if (p.length) {
        if (curView !== 'send') switchView('send'); // surface the new send card
        if (canAnim) dz.animate([{ transform: 'scale(1.015)' }, { transform: 'scale(.99)' }, { transform: 'scale(1)' }], { duration: 240, easing: EASE_POP });
        startSend(p[0]);
      }
    }
  }).catch(() => {});
}

/* --------------------------------- init -------------------------------- */
(async function init() {
  // Defer the first indicator measure past layout/font settling so it lands centered.
  requestAnimationFrame(() => moveIndicator($('.nav-item.is-active')));
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(() => moveIndicator($('.nav-item.is-active')));
  const dd = localStorage.getItem('dropwire-default-dir'); if (dd) { const l = $('#default-folder-label'); l.textContent = dd; l.title = dd; }
  try { DEFAULT_DEST = await invoke('default_dest_dir'); } catch (_) {}
  try { const v = await invoke('app_version'); if (v) $('#app-version').textContent = 'v' + v; } catch (_) {}
  try { const eid = await invoke('my_endpoint_id'); const el = $('#endpoint-id'); el.textContent = eid; el.title = eid; }
  catch (_) { $('#endpoint-id').textContent = HAS_TAURI ? '(starting…)' : '(preview — run inside the app)'; }
})();
