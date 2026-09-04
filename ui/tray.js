/* Dropwire tray panel (G1).
 *
 * Deliberately tiny: it shows what is running, takes a drop or a pasted code,
 * and gets out of the way. Anything that needs a decision opens the main
 * window instead of growing this panel.
 */

const TAURI = window.__TAURI__;
const HAS_TAURI = !!(TAURI && TAURI.core);
const invoke = HAS_TAURI ? TAURI.core.invoke : async () => { throw new Error('Run inside the Dropwire app.'); };
const $ = (s) => document.querySelector(s);

function fmtBytes(n) {
  if (n == null) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${u[i]}`;
}

const GLYPH = {
  send: '<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true"><path d="M12 19V5M5 12l7-7 7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  recv: '<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true"><path d="M12 5v14M5 12l7 7 7-7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};

function row(dir, name, meta, state, stateClass) {
  const el = document.getElementById('tpl-tray-row').content.firstElementChild.cloneNode(true);
  const d = el.querySelector('.js-dir');
  d.className = 'tray-row-dir js-dir ' + dir;
  d.innerHTML = GLYPH[dir];
  const n = el.querySelector('.js-name'); n.textContent = name; n.title = name;
  el.querySelector('.js-meta').textContent = meta || '';
  const s = el.querySelector('.js-state');
  s.textContent = state || '';
  if (stateClass) s.className = 'tray-row-state js-state ' + stateClass;
  return el;
}

/* The main window owns transfer state; the panel mirrors it. Refreshed on every
   open, which is the only time it is visible. */
async function refresh() {
  // In flight comes from the main window via a shared event; history is durable.
  let recent = [];
  try { recent = await invoke('list_transfers'); } catch (_) {}
  const rs = $('#tray-recent'), section = $('#tray-recent-section');
  rs.innerHTML = '';
  const items = (recent || []).slice(0, 4);
  section.hidden = items.length === 0;
  items.forEach((t) => {
    const dir = (t.direction || '').toLowerCase() === 'send' ? 'send' : 'recv';
    const doneish = t.status === 'done';
    rs.appendChild(row(
      dir,
      t.name || 'transfer',
      `${fmtBytes(t.total_bytes)}${t.dest ? ' · saved' : ''}`,
      doneish ? 'Done' : (t.status || ''),
      doneish ? 'ok' : '',
    ));
  });
}

/* Live transfers are broadcast by the main window so both surfaces agree. */
if (HAS_TAURI && TAURI.event && TAURI.event.listen) {
  TAURI.event.listen('live-transfers', (ev) => {
    const items = ev.payload || [];
    const list = $('#tray-live'), section = $('#tray-live-section');
    list.innerHTML = '';
    section.hidden = items.length === 0;
    items.forEach((t) => {
      list.appendChild(row(
        t.dir === 'recv' ? 'recv' : 'send',
        t.name,
        t.meta,
        t.state === 'transferring' ? Math.round((t.pct || 0) * 100) + '%' : (t.state || ''),
        t.state === 'transferring' ? 'pct' : '',
      ));
    });
  }).catch(() => {});
}

$('#tray-open').addEventListener('click', () => invoke('show_main').catch(() => {}));

/* Paste a code straight into the panel: hand it to the main window, which owns
   the preview-and-accept gate. The panel never downloads anything itself. */
document.addEventListener('paste', async (e) => {
  const text = (e.clipboardData && e.clipboardData.getData('text') || '').trim();
  if (!/^blob/i.test(text)) return;
  e.preventDefault();
  try {
    await invoke('show_main');
    if (TAURI.event && TAURI.event.emit) await TAURI.event.emit('tray-code', text);
  } catch (_) {}
});

/* Dropping on the panel starts a send without opening the main window. */
if (HAS_TAURI && TAURI.webview && TAURI.webview.getCurrentWebview) {
  TAURI.webview.getCurrentWebview().onDragDropEvent((event) => {
    const dz = $('#tray-drop'); const t = event.payload.type;
    if (t === 'enter' || t === 'over') dz.classList.add('dragover');
    else if (t === 'leave') dz.classList.remove('dragover');
    else if (t === 'drop') {
      dz.classList.remove('dragover');
      const p = event.payload.paths || [];
      if (!p.length) return;
      // The main window holds the send pipeline and the code the user needs.
      invoke('show_main')
        .then(() => TAURI.event && TAURI.event.emit && TAURI.event.emit('tray-send', p[0]))
        .catch(() => {});
    }
  }).catch(() => {});
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') invoke('hide_tray_window').catch(() => {});
});

(async function init() {
  if (navigator.platform && /mac/i.test(navigator.platform)) $('#tray-paste-key').textContent = '⌘';
  try {
    await invoke('my_endpoint_id');
    $('#tray-net').textContent = 'up';
    $('#tray-net').className = 'pill direct';
  } catch (_) {
    $('#tray-net').textContent = 'starting';
    $('#tray-net').className = 'pill connecting';
  }
  refresh();
})();
