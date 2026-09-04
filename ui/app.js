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
    const m = b.dataset.themeSet; localStorage.setItem('dropwire-theme', m); applyTheme(m); setPref('theme', m);
  }));
})();

/* ========================= persisted settings ============================
   The shell owns these now (settings.json next to the engine data), so a
   rename, a destination or a trusted device survives a restart and is visible
   to every window, not just this webview's localStorage.
   ------------------------------------------------------------------------ */
let PREFS = {
  onboarded: false, deviceName: null, destDir: null, nearbyOn: true, theme: 'auto',
  trusted: [], skipCodeForTrusted: false, trayOnClose: true, startAtLogin: false,
};
async function loadPrefs() {
  try { PREFS = await invoke('get_settings'); } catch (_) {}
  return PREFS;
}
async function setPref(key, value) {
  try { PREFS = await invoke('set_pref', { key, value }); } catch (e) { console.warn(e); }
  return PREFS;
}
const isTrusted = (eid) => !!eid && PREFS.trusted.some((t) => t.endpointId === eid);
/* A device is remembered after a transfer completes. Trust grants nothing on
   its own: the peer still confirms, and the verified preview still gates us. */
async function rememberDevice(dev) {
  if (!dev || !dev.endpointId) return;
  try {
    PREFS = await invoke('trust_remember', { device: {
      endpointId: dev.endpointId, name: dev.name || 'Device',
      os: dev.os || null, fingerprint: dev.fingerprint || '',
    } });
    renderTrusted();
  } catch (e) { console.warn(e); }
}

/* ============================ settings sheet ============================= */
let settingsLastFocus = null;
function openSettings() {
  settingsLastFocus = document.activeElement;
  $('#sheet-settings').classList.remove('hidden');
  invoke('my_fingerprint').then((fp) => { $('#set-fp').textContent = fp; }).catch(() => {});
  loadPrefs().then(() => { renderTrusted(); syncSettingSwitches(); });
  $('#settings-close').focus();
}
function closeSettings() {
  $('#sheet-settings').classList.add('hidden');
  if (settingsLastFocus && settingsLastFocus.focus) settingsLastFocus.focus();
}
function renderTrusted() {
  const list = $('#trusted-list');
  if (!list) return;
  list.innerHTML = '';
  const items = PREFS.trusted || [];
  $('#trusted-intro').textContent = items.length
    ? `${items.length} remembered. Trust never grants access on its own: they still confirm on their side, and you still see the file list before anything is saved.`
    : 'No devices remembered yet. A device is remembered after a transfer with it completes.';
  if (!items.length) {
    const p = document.createElement('p');
    p.className = 'trusted-empty';
    p.textContent = 'Nothing here yet.';
    list.appendChild(p);
    return;
  }
  items.forEach((t) => {
    const row = document.getElementById('tpl-trusted').content.firstElementChild.cloneNode(true);
    const n = row.querySelector('.js-name'); n.textContent = t.name || 'Device'; n.title = t.name || '';
    const os = (t.os || '').toLowerCase();
    if (OS_LABEL[os]) {
      const b = row.querySelector('.js-os'); b.textContent = OS_LABEL[os]; b.classList.remove('hidden');
      row.querySelector('.js-icon').innerHTML = OS_ICON[os];
    }
    const bits = [];
    if (t.fingerprint) bits.push(t.fingerprint);
    bits.push(`${t.transfers || 0} transfer${(t.transfers || 0) === 1 ? '' : 's'}`);
    if (t.lastSeen) bits.push('last ' + new Date(t.lastSeen * 1000).toLocaleString());
    row.querySelector('.js-meta').textContent = bits.join(' \u00b7 ');
    row.querySelector('.js-forget').addEventListener('click', async () => {
      try { PREFS = await invoke('trust_forget', { endpointId: t.endpointId }); renderTrusted(); }
      catch (e) { console.warn(e); }
    });
    list.appendChild(row);
  });
}
function syncSettingSwitches() {
  const set = (sel, on) => { const el = $(sel); if (el) el.setAttribute('aria-checked', on ? 'true' : 'false'); };
  set('#skip-code-toggle', PREFS.skipCodeForTrusted);
  set('#tray-close-toggle', PREFS.trayOnClose);
  set('#start-login-toggle', PREFS.startAtLogin);
}
[['#skip-code-toggle', 'skipCodeForTrusted'], ['#tray-close-toggle', 'trayOnClose'], ['#start-login-toggle', 'startAtLogin']]
  .forEach(([sel, key]) => {
    const el = $(sel);
    if (el) el.addEventListener('click', async () => {
      await setPref(key, !PREFS[key]);
      syncSettingSwitches();
    });
  });
$('#rename-device').addEventListener('click', async () => {
  const current = $('#set-device-name').textContent;
  const next = prompt('What should nearby devices call this one?', current);
  if (next == null) return;
  try {
    PREFS = await invoke('set_device_name', { name: next });
    const n = PREFS.deviceName || next;
    $('#set-device-name').textContent = n;
    $('#device-name').textContent = n;
  } catch (e) { alert(String(e)); }
});
$('#open-settings').addEventListener('click', openSettings);
$('#settings-close').addEventListener('click', closeSettings);
$('#sheet-settings').addEventListener('click', (e) => { if (e.target.id === 'sheet-settings') closeSettings(); });
document.addEventListener('keydown', (e) => {
  if (e.key === ',' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); openSettings(); }
});

/* ====================== notifications and toasts ========================
   The design allows exactly three notifying events: a transfer completed, an
   offer that arrived while the window was hidden, and a failure that needs a
   decision. Progress never notifies. The OS notification is fire-and-forget,
   so anything actionable is mirrored as an in-app toast that keeps the button.
   ------------------------------------------------------------------------ */
const notifApi = (HAS_TAURI && TAURI.notification) ? TAURI.notification : null;
let notifAllowed = false;
async function initNotifications() {
  if (!notifApi) return;
  try {
    notifAllowed = await notifApi.isPermissionGranted();
    if (!notifAllowed) notifAllowed = (await notifApi.requestPermission()) === 'granted';
  } catch (_) { notifAllowed = false; }
}
/** True when the user cannot see the window, so an OS notification is warranted. */
async function windowIsAway() {
  if (document.hidden) return true;
  if (!appWindow) return false;
  try {
    const [vis, foc] = await Promise.all([appWindow.isVisible(), appWindow.isFocused()]);
    return !vis || !foc;
  } catch (_) { return false; }
}
function toast({ title, sub, kind, action }) {
  const el = document.getElementById('tpl-toast').content.firstElementChild.cloneNode(true);
  if (kind) el.classList.add(kind);
  el.querySelector('.js-title').textContent = title;
  const subEl = el.querySelector('.js-sub');
  subEl.textContent = sub || '';
  subEl.title = sub || '';
  const act = el.querySelector('.js-action');
  if (action) {
    act.textContent = action.label;
    act.classList.remove('hidden');
    act.addEventListener('click', () => { action.onClick(); el.remove(); });
  }
  const close = () => { if (el.isConnected) el.remove(); };
  el.querySelector('.js-close').addEventListener('click', close);
  $('#toasts').appendChild(el);
  // A toast with an action waits longer: it is asking for something.
  setTimeout(close, action ? 12000 : 6000);
}
/** One of the three allowed events. `action` stays reachable via the toast. */
async function notify({ title, sub, kind, action, always }) {
  toast({ title, sub, kind, action });
  if (!notifApi || !notifAllowed) return;
  if (!always && !(await windowIsAway())) return;
  try { notifApi.sendNotification({ title, body: sub || '' }); } catch (_) {}
}

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
  const label = route === 'direct' ? 'direct'
    : route === 'relayed' ? 'relayed · a bit slower'
    : route === 'resuming' ? 'resuming' : 'connected';
  el.textContent = label;
  el.className = 'pill js-route ' + (route === 'direct' ? 'direct'
    : route === 'relayed' ? 'relayed' : route === 'resuming' ? 'resuming' : '');
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
/* Both surfaces read one registry: the tray panel mirrors it, and the tray
   icon carries the state so the app says what it is doing while hidden. */
function broadcastLive() {
  const items = [...live.values()];
  if (HAS_TAURI && TAURI.event && TAURI.event.emit) {
    TAURI.event.emit('live-transfers', items).catch(() => {});
  }
  const anyMoving = items.some((t) => t.state === 'transferring');
  invoke('set_tray_state', { stateName: anyMoving ? 'active' : 'idle' }).catch(() => {});
}
function trayFlash(stateName) {
  invoke('set_tray_state', { stateName }).catch(() => {});
  setTimeout(() => {
    const anyMoving = [...live.values()].some((t) => t.state === 'transferring');
    invoke('set_tray_state', { stateName: anyMoving ? 'active' : 'idle' }).catch(() => {});
  }, 4000);
}

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
      // Resuming is derived from a transferring event after an error.
      if (els.svg && els.svg.classList.contains('failed')) {
        els.svg.classList.remove('failed'); els.svg.classList.add('resuming');
      }
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
      const sentName = (live.get(id) || {}).name || 'transfer';
      liveDrop(id);
      trayFlash('done');
      notify({ title: 'Sent ' + sentName, sub: els.meta.textContent || 'delivered' });
      setStatus('transfer complete', 'up');
      break;
    case 'error':
      if (els.svg) { els.svg.classList.add('failed'); els.svg.classList.remove('connecting', 'live'); }
      els.status.setAttribute('aria-live', 'assertive');
      els.status.textContent = m.message ? `Could not send — ${m.message}` : 'Could not send.';
      if (liveSend && liveSend.card === card) liveSend = null;
      liveDrop(id);
      // A failure needs a decision, so it is one of the three notifying events.
      trayFlash('attention');
      notify({ title: 'Send failed', sub: m.message || 'the transfer stopped', kind: 'error',
               action: { label: 'Activity', onClick: () => showPanel('activity') } });
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
$$('[data-send-text]').forEach((b) => b.addEventListener('click', openTextSheet));

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

async function beginReceive(ticket, dest, selected, label) {
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
  if (myDest) { els.sub.textContent = 'saving to ' + myDest; cardDest.set(card, myDest); }
  if (label) { cardLabel.set(card, label); els.name.textContent = label; }
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
    liveSet(id, { dir: 'recv', name: label || 'Incoming transfer', state: 'connecting' });
  } catch (e) {
    removeCard(card);
    $('#recv-error').textContent = 'That code does not look right. Check it and try again.';
    console.warn(e);
  }
}
/** Where a given receive card is writing (used by the completion toast). */
const cardDest = new WeakMap();
/** What the receive is called, learned from the verified preview at accept time. */
const cardLabel = new WeakMap();
const myDestOf = (card) => cardDest.get(card) || null;
function onRecvMsg(m, els, card, getId) {
  const id = getId();
  switch (m.kind) {
    case 'transferring': {
      if (els.svg && !els.svg.dataset.lit) {
        els.svg.dataset.lit = '1'; els.svg.classList.remove('connecting');
        igniteNode(els.svg, '.w-node.peer'); els.name.textContent = 'Receiving…';
      }
      // Resuming is DERIVED, not a new event kind: a transferring event that
      // arrives after an error on the same transfer is a resume.
      if (els.svg && els.svg.classList.contains('failed')) {
        els.svg.classList.remove('failed'); els.svg.classList.add('resuming');
        els.name.textContent = 'Resuming…';
        setRoute(els.route, 'resuming');
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
      trayFlash('done');
      if (lastOfferPeer) { rememberDevice(lastOfferPeer); lastOfferPeer = null; }
      notify({
        title: 'Received ' + (cardLabel.get(card) || 'files'),
        sub: `${fmtBytes(m.stats && m.stats.bytes)} · saved to ${myDestOf(card) || 'your downloads'}`,
        action: myDestOf(card) ? { label: 'Open folder', onClick: () => invoke('reveal_path', { path: myDestOf(card) }).catch(() => {}) } : null,
      });
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
      trayFlash('attention');
      notify({ title: 'Receive failed', sub: els.status.textContent, kind: 'error',
               action: { label: 'Activity', onClick: () => showPanel('activity') } });
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
  const picked = idx.length === 1 && previewFiles[idx[0]] ? previewFiles[idx[0]].name
    : `${idx.length} file${idx.length === 1 ? '' : 's'}`;
  codeInputs().forEach((el) => { el.value = ''; });
  $('#recv-start').disabled = true;
  beginReceive(t, d, selected, picked);
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
function actRow({ dir, name, meta, pct, pill, pillClass, actions, onOpen }) {
  const row = document.getElementById('tpl-act-row').content.firstElementChild.cloneNode(true);
  const d = row.querySelector('.js-dir');
  d.className = 'act-dir js-dir ' + (dir === 'recv' ? 'recv' : 'send');
  d.innerHTML = DIR_GLYPH[dir === 'recv' ? 'recv' : 'send'];
  const n = row.querySelector('.js-name'); n.textContent = name; n.title = name;
  row.querySelector('.js-meta').textContent = meta || '';
  row.querySelector('.js-pct').textContent = pct == null ? '' : Math.round(pct * 100) + '%';
  const p = row.querySelector('.js-pill');
  if (pill) { p.textContent = pill; p.className = 'pill js-pill ' + (pillClass || ''); } else p.remove();
  if (onOpen) {
    row.tabIndex = 0;
    row.addEventListener('click', (e) => { if (!e.target.closest('button')) onOpen(); });
    row.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpen(); } });
  }
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
  items.forEach(([id, t]) => {
    list.appendChild(actRow({
      dir: t.dir, name: t.name, meta: t.meta,
      pct: t.state === 'transferring' ? t.pct : null,
      pill: t.state === 'ready' ? 'waiting' : (t.route || t.state),
      pillClass: t.route === 'direct' ? 'direct' : t.route === 'relayed' ? 'relayed' : 'connecting',
      onOpen: () => openDrawer({
        kind: t.dir === 'recv' ? 'Receiving' : 'Sending',
        title: t.name, sub: t.meta, pct: t.pct, id,
        pill: t.route || t.state, pillClass: t.route === 'direct' ? 'direct' : t.route === 'relayed' ? 'relayed' : 'connecting',
        rate: t.meta, canCancel: true, ticket: t.ticket,
        facts: [['Peer', t.peer], ['Route', t.route || 'connecting'], ['Started', t.startedAt], ['Verified', 'BLAKE3, per chunk']],
      }),
    }));
  });
  $('#act-live-meta').textContent = items.length ? `${items.length} running` : '';
  broadcastLive();
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
  const clearBtn = $('#clear-history');
  let items = [];
  try { items = await invoke('list_transfers'); } catch (_) {}
  list.innerHTML = '';
  section.hidden = !items || !items.length;
  if (clearBtn) clearBtn.classList.toggle('hidden', !items || !items.length);
  if (items && items.length) {
    items.forEach((t, i) => {
      const dir = (t.direction || '').toLowerCase();
      const resumable = dir === 'receive' && t.status === 'interrupted' && t.ticket && t.dest;
      const resendable = dir === 'send' && !!t.source;
      const actions = [];
      const failedish = t.status === 'error' || t.status === 'cancelled';
      if (resumable) actions.push({ label: 'Resume', cls: 'btn-ghost', onClick: () => beginReceive(t.ticket, t.dest) });
      else if (resendable) actions.push({
        // "Retry" when it never got through, "Resend" when it did: same call,
        // but the label should say which situation the user is in.
        label: failedish ? 'Retry' : 'Resend', cls: 'btn-ghost',
        onClick: () => { showPanel('send'); startSend(t.source); },
      });
      if (t.status === 'done' && t.dest) actions.push({ label: 'Reveal', onClick: () => invoke('reveal_path', { path: t.dest }).catch(() => {}) });
      const row = actRow({
        dir: dir === 'send' ? 'send' : 'recv',
        name: t.name || 'transfer',
        meta: `${fmtBytes(t.total_bytes)} · ${dir === 'send' ? 'sent' : 'received'}`,
        pct: null,
        pill: STATUS_LABEL[t.status] || t.status || '',
        pillClass: STATUS_PILL[t.status] || '',
        actions,
        onOpen: () => openDrawer({
          kind: dir === 'send' ? 'Sent' : 'Received',
          title: t.name || 'transfer',
          sub: `${t.file_count || 0} file${(t.file_count || 0) === 1 ? '' : 's'} · ${fmtBytes(t.total_bytes)}`,
          pct: t.total_bytes ? Math.min(1, (t.transferred || 0) / t.total_bytes) : null,
          pill: STATUS_LABEL[t.status] || t.status, pillClass: STATUS_PILL[t.status] || '',
          finished: t.status === 'done', ticket: t.ticket, canCancel: false,
          facts: [
            ['Status', STATUS_LABEL[t.status] || t.status],
            ['Size', fmtBytes(t.total_bytes)],
            ['Saved to', t.dest],
            ['Source', t.source],
            ['Started', t.created_at ? new Date(t.created_at * 1000).toLocaleString() : ''],
            ['Verified', 'BLAKE3, per chunk'],
          ],
        }),
      });
      list.appendChild(row);
      if (canAnim) row.animate([{ opacity: 0, transform: 'translateY(8px)' }, { opacity: 1, transform: 'none' }], { duration: 220, delay: Math.min(i, 6) * 40, easing: EASE_OUT });
    });
  }
  updateActivityEmpty();
}

/* ========================== E2: the detail drawer ========================
   One transfer, everything about it. Opened from any Activity row.
   ------------------------------------------------------------------------ */
let drawerCtx = null, drawerLastFocus = null;
function openDrawer(ctx) {
  drawerCtx = ctx;
  drawerLastFocus = document.activeElement;
  $('#drawer-kind').textContent = ctx.kind;
  $('#drawer-title').textContent = ctx.title;
  $('#drawer-sub').textContent = ctx.sub || '';
  $('#drawer-pct').textContent = ctx.pct == null ? '—' : Math.round(ctx.pct * 100) + '%';
  const pill = $('#drawer-pill');
  pill.textContent = ctx.pill || '';
  pill.className = 'pill ' + (ctx.pillClass || '');
  pill.classList.toggle('hidden', !ctx.pill);
  $('#drawer-rate').textContent = ctx.rate || '';
  const fill = $('#drawer .w-fill');
  if (fill) fill.style.strokeDashoffset = String(1 - (ctx.pct || 0));
  $$('#drawer .w-node').forEach((n) => n.classList.toggle('done', ctx.finished === true));

  const dl = $('#drawer-facts'); dl.innerHTML = '';
  (ctx.facts || []).forEach(([k, v]) => {
    if (v == null || v === '') return;
    const dt = document.createElement('dt'); dt.textContent = k;
    const dd = document.createElement('dd'); dd.textContent = v; dd.title = v;
    dl.append(dt, dd);
  });

  const ul = $('#drawer-files'); ul.innerHTML = '';
  const setFiles = (files) => {
    ul.innerHTML = '';
    (files || []).forEach((f) => {
      const li = document.createElement('li'); li.className = 'file-row';
      const n = document.createElement('span'); n.className = 'file-name'; n.textContent = f.name; n.title = f.name;
      const sz = document.createElement('span'); sz.className = 'file-size'; sz.textContent = fmtBytes(f.size);
      li.append(n, sz); ul.appendChild(li);
    });
    if (!files || !files.length) {
      const li = document.createElement('li'); li.className = 'file-row';
      li.innerHTML = '<span class="file-name">File list unavailable for this transfer.</span>';
      ul.appendChild(li);
    }
  };
  setFiles(null);
  // Names and sizes come from the manifest the code commits to, so the drawer
  // shows verified content rather than anything a peer claimed.
  if (ctx.ticket) {
    invoke('inspect_ticket', { ticket: ctx.ticket })
      .then((p) => { if (drawerCtx === ctx) setFiles(p.files); })
      .catch(() => { if (drawerCtx === ctx) setFiles([]); });
  }

  $('#drawer-copy').classList.toggle('hidden', !ctx.ticket);
  $('#drawer-cancel').textContent = ctx.canCancel ? 'Cancel' : 'Close';
  $('#drawer').classList.remove('hidden');
  $('#drawer-close').focus();
}
function closeDrawer() {
  $('#drawer').classList.add('hidden');
  drawerCtx = null;
  if (drawerLastFocus && drawerLastFocus.focus) drawerLastFocus.focus();
}
$('#drawer-close').addEventListener('click', closeDrawer);
$('#drawer').addEventListener('click', (e) => { if (e.target.id === 'drawer') closeDrawer(); });
$('#drawer-copy').addEventListener('click', () => {
  if (drawerCtx && drawerCtx.ticket && navigator.clipboard) {
    navigator.clipboard.writeText(drawerCtx.ticket);
    const b = $('#drawer-copy'); b.textContent = 'Copied ✓';
    setTimeout(() => { b.textContent = 'Copy code again'; }, 1400);
  }
});
$('#drawer-cancel').addEventListener('click', async () => {
  if (drawerCtx && drawerCtx.canCancel && drawerCtx.id) {
    await invoke('cancel_transfer', { id: drawerCtx.id }).catch(() => {});
  }
  closeDrawer();
});

/* ======================== G2: send text or clipboard ==================== */
function textStats() {
  const v = $('#text-body').value;
  const lines = v ? v.split(/\r?\n/).length : 0;
  const bytes = new TextEncoder().encode(v).length;
  $('#text-count').textContent = `${lines} line${lines === 1 ? '' : 's'} · ${bytes} byte${bytes === 1 ? '' : 's'}`;
  $('#text-send').disabled = v.trim().length === 0;
}
function openTextSheet() {
  $('#text-body').value = '';
  textStats();
  $('#sheet-text').classList.remove('hidden');
  $('#text-body').focus();
}
function closeTextSheet() { $('#sheet-text').classList.add('hidden'); }
$('#text-body').addEventListener('input', textStats);
$('#text-close').addEventListener('click', closeTextSheet);
$('#text-cancel').addEventListener('click', closeTextSheet);
$('#sheet-text').addEventListener('click', (e) => { if (e.target.id === 'sheet-text') closeTextSheet(); });
$('#text-clipboard').addEventListener('click', async () => {
  try { $('#text-body').value = await navigator.clipboard.readText(); textStats(); }
  catch (_) { $('#text-body').focus(); }
});
$('#text-send').addEventListener('click', async () => {
  const text = $('#text-body').value;
  if (!text.trim()) return;
  const btn = $('#text-send'); btn.disabled = true; btn.textContent = 'Preparing…';
  try {
    // Text is not a second protocol: it becomes a small file and takes the
    // ordinary send path, code and all.
    const path = await invoke('write_text_file', { text });
    closeTextSheet();
    showPanel('send');
    startSend(path);
  } catch (e) { alert(String(e)); }
  btn.disabled = false; btn.textContent = 'Get a code';
});

$('#clear-history').addEventListener('click', async () => {
  if (!confirm('Clear the transfer history on this device? Files already received are not deleted.')) return;
  try { await invoke('clear_transfers'); } catch (e) { console.warn(e); }
  loadHistory();
});

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
    setPref('destDir', dir);
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
  setPref('nearbyOn', nearby.on);
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
        rememberDevice(d);
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
/* The peer an in-flight receive came from, so a completed transfer can
   remember the device it actually happened with. */
let lastOfferPeer = null;
const OFFER_TTL_MS = 112000; // just inside the sender's 115s self-decline
function anyModalOpen() {
  return !$('#nearby-offer-modal').classList.contains('hidden')
    || !$('#recv-preview').classList.contains('hidden')
    || !$('#sheet-settings').classList.contains('hidden')
    || !$('#sheet-text').classList.contains('hidden')
    || !$('#drawer').classList.contains('hidden')
    || !$('#onboarding').classList.contains('hidden');
}
function enqueueOffer(offer) {
  if (!offer || !offer.offerId) return;
  // Trusted + skip-the-code: go straight to the verified preview. They still
  // confirmed on their side, and the file list still gates what is written.
  if (PREFS.skipCodeForTrusted && isTrusted(offer.fromEndpointId)) {
    nearby.offers.set(offer.offerId, offer);
    lastOfferPeer = { endpointId: offer.fromEndpointId, name: offer.deviceName, fingerprint: offer.fingerprint };
    invoke('nearby_respond', { offerId: offer.offerId, accept: true })
      .then(() => openPreview(offer.ticket, recvDest || PREFS.destDir || null))
      .catch(() => { nearby.offers.delete(offer.offerId); showOfferModal(offer); });
    return;
  }
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
  // An offer is the one thing that needs a decision: say so on the tray icon.
  invoke('set_tray_state', { stateName: 'attention' }).catch(() => {});
  // …and notify, but only if the user could not have seen the dialog.
  notify({
    title: (offer.deviceName || 'A nearby device') + ' wants to send you files',
    sub: `${offer.fileCount} file${offer.fileCount === 1 ? '' : 's'} · ${fmtBytes(offer.totalBytes)}`,
    kind: 'warn',
    action: { label: 'Review', onClick: () => invoke('show_main').catch(() => {}) },
  });
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
  broadcastLive();
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
  lastOfferPeer = { endpointId: offer.fromEndpointId, name: offer.deviceName, fingerprint: offer.fingerprint };
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
  if (!$('#sheet-text').classList.contains('hidden')) return closeTextSheet();
  if (!$('#drawer').classList.contains('hidden')) return closeDrawer();
  if (!$('#sheet-settings').classList.contains('hidden')) return closeSettings();
});

[$('#nearby-toggle'), $('#nearby-toggle-2')].forEach((t) => { if (t) t.addEventListener('click', toggleNearby); });
if (HAS_TAURI && TAURI.event && TAURI.event.listen) {
  TAURI.event.listen('nearby-offer', (ev) => enqueueOffer(ev.payload)).catch(() => {});
}

/* ====================== A1 / A2: first-run setup =========================
   Owns the window until it is done. Nothing touches the network from these
   screens: the name and preferences are written first, then the app starts.
   ------------------------------------------------------------------------ */
function showOnboarding(step) {
  $('#onboarding').classList.remove('hidden');
  $$('.onboard-step').forEach((el) => {
    const on = el.id === 'onboard-' + step;
    el.classList.toggle('is-active', on);
    el.hidden = !on;
  });
  if (step === 2) setTimeout(() => $('#onboard-name').focus(), 40);
}
function hideOnboarding() { $('#onboarding').classList.add('hidden'); }
$('#onboard-start').addEventListener('click', () => showOnboarding(2));
$('#onboard-back').addEventListener('click', () => showOnboarding(1));
$('#onboard-nearby').addEventListener('click', (e) => {
  const b = e.currentTarget;
  b.setAttribute('aria-checked', b.getAttribute('aria-checked') === 'true' ? 'false' : 'true');
});
$('#onboard-dest-change').addEventListener('click', async () => {
  const dir = await invoke('pick_dest_dir').catch(() => null);
  if (dir) { $('#onboard-dest').textContent = dir; $('#onboard-dest').title = dir; }
});
$('#onboard-finish').addEventListener('click', async () => {
  const btn = $('#onboard-finish'); btn.disabled = true;
  const name = ($('#onboard-name').value || '').trim();
  const on = $('#onboard-nearby').getAttribute('aria-checked') === 'true';
  const dest = $('#onboard-dest').title || $('#onboard-dest').textContent;
  try {
    if (name) await invoke('set_device_name', { name });
    await setPref('nearbyOn', on);
    if (dest && dest !== 'Downloads') await setPref('destDir', dest);
    await setPref('onboarded', true);
  } catch (e) { console.warn(e); }
  await loadPrefs();
  hideOnboarding();
  applyPrefsToUi();
  nearby.on = PREFS.nearbyOn;
  if (nearby.on) nearbyStart(); else { nearbySetSwitch(false); nearbyStatus('Sharing is off, this device is invisible.'); renderDevices(); }
  btn.disabled = false;
});

/* Push loaded preferences into the chrome that displays them. */
function applyPrefsToUi() {
  const dest = PREFS.destDir || DEFAULT_DEST;
  if (dest) {
    const l = $('#default-folder-label'); l.textContent = dest; l.title = dest;
    recvDest = dest; setDestLabel(dest);
  }
  if (PREFS.deviceName) {
    $('#device-name').textContent = PREFS.deviceName;
    $('#set-device-name').textContent = PREFS.deviceName;
  }
  applyTheme(PREFS.theme || 'auto');
  syncSettingSwitches();
  renderTrusted();
}

/* ================================= init ================================== */
(async function init() {
  await loadPrefs();
  initNotifications();
  try { DEFAULT_DEST = await invoke('default_dest_dir'); } catch (_) {}
  applyPrefsToUi();
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

  // First run owns the window until setup is finished.
  if (!PREFS.onboarded) {
    $$('.onboard-ver').forEach((el) => { el.textContent = $('#app-version').textContent || ''; });
    try { $('#onboard-name').value = await invoke('device_name'); } catch (_) {}
    try { $('#onboard-fp').textContent = await invoke('my_fingerprint'); } catch (_) {}
    $('#onboard-dest').textContent = PREFS.destDir || DEFAULT_DEST || 'Downloads';
    $('#onboard-dest').title = PREFS.destDir || DEFAULT_DEST || '';
    showOnboarding(1);
  } else {
    nearby.on = PREFS.nearbyOn;
    if (nearby.on) nearbyStart();
    else { nearbySetSwitch(false); nearbyStatus('Sharing is off, this device is invisible.'); renderDevices(); }
  }
  renderDevices();
  renderActivityLive();
  loadHistory();

  // The tray panel hands work to the window that owns the pipelines.
  if (HAS_TAURI && TAURI.event && TAURI.event.listen) {
    TAURI.event.listen('tray-send', (ev) => { showPanel('send'); startSend(ev.payload); }).catch(() => {});
    TAURI.event.listen('tray-code', (ev) => {
      showPanel('receive');
      $('#recv-code-input').value = ev.payload;
      $('#recv-code-input').dispatchEvent(new Event('input'));
      submitCode();
    }).catch(() => {});
  }
})();
