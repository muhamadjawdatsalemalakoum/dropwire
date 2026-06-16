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
  $$('.seg-btn').forEach((b) => b.classList.toggle('on', b.dataset.themeSet === mode));
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
function setBar(barSel, pctSel, done, total) {
  const fill = $(barSel);
  const p = total > 0 ? Math.min(1, done / total) : 0;
  if (fill) {
    fill.style.strokeDashoffset = String(1 - p);
    const svg = fill.closest('svg');
    const clip = svg && svg.querySelector('.lit-clip');
    if (clip) clip.setAttribute('width', String(60 + p * 480)); // lit region: x=60..540
  }
  const pe = $(pctSel);
  if (pe) pe.textContent = Math.round(p * 100) + '%';
}
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
  const self = svg.querySelector('.w-node.self') || svg.querySelector('.w-node.peer');
  if (self) self.classList.add('done', 'live');
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

/* -------------------------------- SEND --------------------------------- */
let sendId = null;
async function startSend(path) {
  try {
    $('#send-pick').classList.add('hidden');
    const card = $('#send-active'); card.classList.remove('hidden');
    $('#send-code').textContent = '…'; $('#send-qr').innerHTML = ''; $('#send-status').textContent = 'Preparing…';
    $('#send-cancel').textContent = 'Cancel';
    resetWire($('#send-bar') && $('#send-bar').closest('svg'));
    const ch = makeChannel(); ch.onmessage = onSendMsg;
    sendId = await invoke('start_send', { path, onEvent: ch });
  } catch (e) { $('#send-status').textContent = String(e); }
}
function onSendMsg(m) {
  const svg = $('#send-bar') && $('#send-bar').closest('svg');
  switch (m.kind) {
    case 'importing': $('#send-status').textContent = `Preparing… ${fmtBytes(m.done)} / ${fmtBytes(m.total)}`; break;
    case 'ready': revealTicket(m.ticket); break;
    case 'peerJoined':
      if (svg && !svg.dataset.lit) { svg.dataset.lit = '1'; svg.classList.remove('connecting'); igniteNode(svg, '.w-node.peer'); }
      $('#send-status').textContent = 'Receiver connected — sending…';
      break;
    case 'transferring':
      setBar('#send-bar', '#send-pct', m.offset, m.total);
      $('#send-status').textContent = `Sending… ${fmtBytes(m.offset)} / ${fmtBytes(m.total)}`;
      break;
    case 'done':
      setBar('#send-bar', '#send-pct', 1, 1);
      if (svg) doneSpark(svg);
      $('#send-status').textContent = 'Sent ✓';
      $('#send-cancel').textContent = 'Send another';
      break;
    case 'error': $('#send-status').textContent = 'Error: ' + m.message; break;
    case 'cancelled': resetSend(); break;
  }
}
function revealTicket(ticket) {
  const card = $('#send-active');
  $('#send-code').textContent = ticket;
  $('#send-status').textContent = "Ready — share this code. Keep the app open until it's received.";
  invoke('qr_svg', { text: ticket }).then((svg) => {
    $('#send-qr').innerHTML = svg;
    if (canAnim) $('#send-qr').animate(
      [{ opacity: 0, transform: 'scale(.96)', filter: 'blur(4px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }],
      { duration: 380, easing: EASE_OUT, delay: 80 });
  }).catch(() => {});
  if (canAnim) {
    card.animate([{ opacity: 0, transform: 'translateY(16px) scale(.94)' }, { opacity: 1, transform: 'none' }],
      { duration: 380, easing: EASE_POP });
    $('#send-code').animate([{ clipPath: 'inset(0 100% 0 0)' }, { clipPath: 'inset(0 0 0 0)' }],
      { duration: 380, easing: EASE_OUT });
  }
}
function resetSend() {
  sendId = null;
  $('#send-active').classList.add('hidden');
  $('#send-pick').classList.remove('hidden');
  $('#send-cancel').textContent = 'Cancel';
  resetWire($('#send-bar') && $('#send-bar').closest('svg'));
}
$('#pick-file').addEventListener('click', async () => {
  const p = await invoke('pick_paths', { directory: false, multiple: false }).catch(() => []);
  if (p && p.length) startSend(p[0]);
});
$('#pick-folder').addEventListener('click', async () => {
  const p = await invoke('pick_paths', { directory: true, multiple: false }).catch(() => []);
  if (p && p.length) startSend(p[0]);
});
$('#copy-code').addEventListener('click', () => {
  const code = $('#send-code').textContent;
  if (code && code !== '…' && navigator.clipboard) {
    navigator.clipboard.writeText(code);
    const btn = $('#copy-code'); const t = btn.textContent; btn.textContent = 'Copied ✓';
    setTimeout(() => { btn.textContent = t; }, 1400);
  }
});
$('#send-cancel').addEventListener('click', async () => {
  if (sendId) await invoke('cancel_transfer', { id: sendId }).catch(() => {});
  resetSend();
});

/* ------------------------------- RECEIVE ------------------------------- */
let recvId = null, recvDest = null;
$('#recv-code-input').addEventListener('input', (e) => {
  const v = e.target.value.trim();
  $('#recv-start').disabled = v.length === 0;
  const glyph = $('.code-input-glyph circle'); if (glyph) glyph.setAttribute('fill', v ? 'var(--wire)' : 'var(--wire-dim)');
});
$('#pick-dest').addEventListener('click', async () => {
  const dir = await invoke('pick_dest_dir').catch(() => null);
  if (dir) { recvDest = dir; $('#recv-dest-label').innerHTML = `Save to: <em>${dir}</em>`; }
});
async function beginReceive(ticket, dest) {
  $('#recv-error').textContent = '';
  switchView('receive');
  try {
    $('#recv-pick').classList.add('hidden');
    $('#recv-active').classList.remove('hidden');
    $('#recv-name').textContent = 'Connecting…';
    const badge = $('#recv-route'); badge.textContent = 'connecting'; badge.className = 'route-badge connecting';
    resetWire($('#recv-bar') && $('#recv-bar').closest('svg'));
    setBar('#recv-bar', '#recv-pct', 0, 1);
    $('#recv-open').classList.add('hidden'); $('#recv-another').classList.add('hidden');
    recvDest = dest || null;
    const ch = makeChannel(); ch.onmessage = onRecvMsg;
    recvId = await invoke('start_receive', { ticket, dest: dest || null, onEvent: ch });
  } catch (e) {
    $('#recv-error').textContent = String(e);
    const input = $('#recv-code-input'); if (input) { input.classList.add('shake'); setTimeout(() => input.classList.remove('shake'), 400); }
    resetRecv();
  }
}
$('#recv-start').addEventListener('click', () => {
  const ticket = $('#recv-code-input').value.trim();
  if (!ticket) return;
  const dest = recvDest || localStorage.getItem('dropwire-default-dir') || null;
  beginReceive(ticket, dest);
});
function onRecvMsg(m) {
  const svg = $('#recv-bar') && $('#recv-bar').closest('svg');
  switch (m.kind) {
    case 'transferring': {
      if (svg && !svg.dataset.lit) { svg.dataset.lit = '1'; svg.classList.remove('connecting'); igniteNode(svg, '.w-node.peer'); $('#recv-name').textContent = 'Receiving…'; }
      const b = $('#recv-route'); const r = m.route;
      b.textContent = r === 'direct' ? 'direct' : r === 'relayed' ? 'relayed · a bit slower' : 'transferring';
      b.className = 'route-badge ' + (r === 'direct' ? 'direct' : r === 'relayed' ? 'relayed' : '');
      b.setAttribute('aria-label', 'Connection: ' + b.textContent);
      setBar('#recv-bar', '#recv-pct', m.offset, m.total);
      $('#recv-status').textContent = `· ${fmtBytes(m.offset)} / ${fmtBytes(m.total)}`;
      break;
    }
    case 'done':
      setBar('#recv-bar', '#recv-pct', 1, 1);
      if (svg) doneSpark(svg);
      $('#recv-name').textContent = 'Received ✓';
      $('#recv-status').textContent = `· ${fmtBytes(m.stats && m.stats.bytes)} in ${((m.stats && m.stats.seconds) || 0).toFixed(1)}s`;
      $('#recv-open').classList.remove('hidden');
      $('#recv-another').classList.remove('hidden');
      if (canAnim) $('#recv-open').animate([{ opacity: 0, transform: 'translateY(8px) scale(.94)' }, { opacity: 1, transform: 'none' }], { duration: 380, easing: EASE_POP });
      break;
    case 'error': $('#recv-error').textContent = m.message; resetRecv(); break;
    case 'cancelled': resetRecv(); break;
  }
}
function resetRecv() {
  recvId = null;
  $('#recv-active').classList.add('hidden');
  $('#recv-pick').classList.remove('hidden');
  $('#recv-open').classList.add('hidden'); $('#recv-another').classList.add('hidden');
  resetWire($('#recv-bar') && $('#recv-bar').closest('svg'));
}
$('#recv-cancel').addEventListener('click', async () => {
  if (recvId) await invoke('cancel_transfer', { id: recvId }).catch(() => {});
  resetRecv();
});
$('#recv-open').addEventListener('click', async () => {
  if (recvDest) await invoke('reveal_path', { path: recvDest }).catch(() => {});
});
$('#recv-another').addEventListener('click', () => resetRecv());
if ($('#copy-id')) $('#copy-id').addEventListener('click', () => { const id = $('#endpoint-id').textContent; if (navigator.clipboard && id) navigator.clipboard.writeText(id); });
if ($('#change-folder')) $('#change-folder').addEventListener('click', async () => { const dir = await invoke('pick_dest_dir').catch(() => null); if (dir) { localStorage.setItem('dropwire-default-dir', dir); $('#default-folder-label').textContent = dir; } });

/* ------------------------------- HISTORY ------------------------------- */
function histGlyph(dir) {
  const send = dir === 'send';
  const lx = send ? 'var(--wire)' : 'var(--text-faint)';
  const rx = send ? 'var(--text-faint)' : 'var(--wire)';
  return `<svg class="hist-dir" viewBox="0 0 40 24" aria-hidden="true"><path d="M6 12 H34" stroke="var(--wire-dim)" stroke-width="3" stroke-linecap="round"/><circle cx="6" cy="12" r="4" fill="${lx}"/><circle cx="34" cy="12" r="3.5" fill="${rx}"/></svg>`;
}
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
    const el = document.createElement('div');
    el.className = 'hist-item';
    const right = resumable
      ? `<button class="btn-quiet sm" data-resume="1">Resume</button>`
      : `<div class="hist-meta">${dir === 'send' ? 'Sent' : 'Received'}</div>`;
    el.innerHTML = `${histGlyph(dir)}<div><div class="hist-name">${esc(t.name || 'transfer')}</div><div class="hist-meta">${fmtBytes(t.total_bytes)} · ${esc(t.status || '')}</div></div>${right}`;
    if (resumable) el.querySelector('[data-resume]').addEventListener('click', () => beginReceive(t.ticket, t.dest));
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
      if (canAnim) dz.animate([{ transform: 'scale(1.015)' }, { transform: 'scale(.99)' }, { transform: 'scale(1)' }], { duration: 240, easing: EASE_POP });
      const p = event.payload.paths || []; if (p.length) startSend(p[0]);
    }
  }).catch(() => {});
}

/* --------------------------------- init -------------------------------- */
(async function init() {
  moveIndicator($('.nav-item.is-active'));
  const dd = localStorage.getItem('dropwire-default-dir'); if (dd) $('#default-folder-label').textContent = dd;
  try { $('#endpoint-id').textContent = await invoke('my_endpoint_id'); }
  catch (_) { $('#endpoint-id').textContent = HAS_TAURI ? '(starting…)' : '(preview — run inside the app)'; }
})();
