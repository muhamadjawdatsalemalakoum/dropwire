//! Dropwire desktop shell (Tauri v2).
//!
//! Deliberately thin: it owns the Tauri app, the window, and the command surface,
//! and forwards everything to the verified `irohcore` engine. No iroh-blobs types
//! appear here — only `irohcore`'s stable API.

mod settings;
mod snippets;

use std::path::PathBuf;
use std::sync::Arc;

use irohcore::{
    Core, CoreConfig, CoreError, CtrlMsg, NearbyDevice, Progress, TransferId, TransferPreview,
    TransferRecord,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio_stream::StreamExt;

/// Long-lived app state: the engine handle plus this machine's preferences.
struct AppState {
    core: Core,
    settings: settings::Store,
    /// Text sent with "Send text", kept in the data folder while in use.
    snippets: Arc<snippets::Snippets>,
}

fn fp_to_string(fp: tauri_plugin_dialog::FilePath) -> Option<String> {
    fp.into_path()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// This device's stable public identity.
#[tauri::command]
fn my_endpoint_id(state: State<'_, AppState>) -> String {
    state.core.endpoint_id()
}

/// This device's short human-checkable fingerprint (pairing dialogs).
#[tauri::command]
fn my_fingerprint(state: State<'_, AppState>) -> String {
    state.core.fingerprint()
}

/// This device's advertised display name (what nearby peers see).
#[tauri::command]
async fn device_name(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.core.device_name().await)
}

/* ------------------------------ settings ------------------------------- */

/// Everything the setup screens, Settings sheet and trusted list read from.
#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> settings::Settings {
    state.settings.get()
}

/// Rename this device. Applies to the live mDNS advertisement immediately.
/// The engine checks and tidies the name (an empty one, or one over 40
/// characters, is refused with a message to show as it is), and the tidied
/// name is the one saved.
#[tauri::command]
async fn set_device_name(
    name: String,
    state: State<'_, AppState>,
) -> Result<settings::Settings, String> {
    state
        .core
        .set_device_name(name)
        .await
        .map_err(|e| e.to_string())?;
    let name = state.core.device_name().await;
    Ok(state.settings.update(|s| s.device_name = Some(name)))
}

/// Passed by the "Start at login" entry, so a login launch can start in the
/// tray instead of opening the window.
const LOGIN_LAUNCH_ARG: &str = "--autostart";

/// Add or remove Dropwire from the system's startup list (the Run key on
/// Windows, a LaunchAgent on macOS, an XDG autostart entry on Linux).
#[cfg(desktop)]
fn set_start_at_login(app: &AppHandle, on: bool) -> Result<(), String> {
    let Some(al) = app.try_state::<tauri_plugin_autostart::AutoLaunchManager>() else {
        return Err("start at login is not available on this system".into());
    };
    let result = if on { al.enable() } else { al.disable() };
    match result {
        Ok(()) => Ok(()),
        // Turning it off when the entry is already gone (removed outside the
        // app) is already done.
        Err(_) if !on && al.is_enabled().ok() == Some(false) => Ok(()),
        Err(e) => Err(format!("could not change start at login: {e}")),
    }
}

#[cfg(not(desktop))]
fn set_start_at_login(_app: &AppHandle, _on: bool) -> Result<(), String> {
    Err("start at login is not available on this system".into())
}

/// Whether Dropwire is registered (and not disabled) in the system's startup
/// list, or `None` if that cannot be read.
#[cfg(desktop)]
fn start_at_login_registered(app: &AppHandle) -> Option<bool> {
    app.try_state::<tauri_plugin_autostart::AutoLaunchManager>()?
        .is_enabled()
        .ok()
}

#[cfg(not(desktop))]
fn start_at_login_registered(_app: &AppHandle) -> Option<bool> {
    None
}

/// Persist one of the simple preferences. Unknown keys are rejected rather than
/// silently ignored, so a typo in the UI shows up immediately.
#[tauri::command]
fn set_pref(
    key: String,
    value: serde_json::Value,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<settings::Settings, String> {
    let as_bool = || {
        value
            .as_bool()
            .ok_or_else(|| format!("{key} expects a bool"))
    };
    let as_str = || {
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{key} expects a string"))
    };
    let updated = match key.as_str() {
        "onboarded" => state.settings.update(|s| s.onboarded = true),
        "nearbyOn" => {
            let v = as_bool()?;
            state.settings.update(|s| s.nearby_on = v)
        }
        "skipCodeForTrusted" => {
            let v = as_bool()?;
            state.settings.update(|s| s.skip_code_for_trusted = v)
        }
        "trayOnClose" => {
            let v = as_bool()?;
            state.settings.update(|s| s.tray_on_close = v)
        }
        "startAtLogin" => {
            let v = as_bool()?;
            // Register first: the switch must never read On while nothing
            // would actually start at login.
            set_start_at_login(&app, v)?;
            state.settings.update(|s| s.start_at_login = v)
        }
        "theme" => {
            let v = as_str()?;
            state.settings.update(|s| s.theme = v)
        }
        "destDir" => {
            let v = as_str()?;
            state.settings.update(|s| s.dest_dir = Some(v))
        }
        other => return Err(format!("unknown preference: {other}")),
    };
    Ok(updated)
}

/// Remember a device we just completed a transfer with (or bump its counters).
#[tauri::command]
fn trust_remember(
    device: settings::Trusted,
    state: State<'_, AppState>,
) -> Result<settings::Settings, String> {
    if device.endpoint_id.is_empty() {
        return Err("a trusted device needs an endpoint id".into());
    }
    Ok(state.settings.remember(device))
}

#[tauri::command]
fn trust_forget(
    endpoint_id: String,
    state: State<'_, AppState>,
) -> Result<settings::Settings, String> {
    Ok(state.settings.forget(&endpoint_id))
}

/* ------------------------------ send text ------------------------------ */

/// Write a snippet to a small file so it can travel the ordinary transfer
/// path. Returns the path for `start_send`: text is not a second protocol, it
/// is just a small file (see the design's G2 sheet). The file lives in the
/// data folder only while something uses it (see `snippets`).
#[tauri::command]
fn write_text_file(text: String, state: State<'_, AppState>) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("nothing to send".into());
    }
    let path = state.snippets.write(&text)?;
    Ok(path.to_string_lossy().into_owned())
}

/* -------------------------------- window ------------------------------- */

/// Show and focus the main window (from the tray, or "Open Dropwire").
#[tauri::command]
fn show_main(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(t) = app.get_webview_window("tray") {
        let _ = t.hide();
    }
}

/// Keep the main window's title bar reachable. The window has no native frame,
/// so the drawn title bar is the only way to move it. `preventOverflow` shrinks
/// the first window to the work area, but a work area smaller than the minimum
/// size can still leave it hanging off an edge; pull it back so the top-left
/// corner (and as much of the bar as fits) starts on screen.
fn keep_title_bar_on_screen(w: &tauri::WebviewWindow) {
    let (Ok(Some(m)), Ok(pos), Ok(size)) =
        (w.current_monitor(), w.outer_position(), w.outer_size())
    else {
        return;
    };
    let wa = m.work_area();
    let (left, top) = (wa.position.x, wa.position.y);
    let right = left.saturating_add(i32::try_from(wa.size.width).unwrap_or(i32::MAX));
    let bottom = top.saturating_add(i32::try_from(wa.size.height).unwrap_or(i32::MAX));
    let (w_px, h_px) = (
        i32::try_from(size.width).unwrap_or(i32::MAX),
        i32::try_from(size.height).unwrap_or(i32::MAX),
    );
    // Prefer fitting entirely; when it cannot, the top-left edge wins.
    let x = pos.x.min(right.saturating_sub(w_px)).max(left);
    let y = pos.y.min(bottom.saturating_sub(h_px)).max(top);
    if (x, y) != (pos.x, pos.y) {
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Hide the tray panel (it closes shortly after losing focus, and after an action).
#[tauri::command]
fn hide_tray_window(app: AppHandle) {
    if let Some(t) = app.get_webview_window("tray") {
        let _ = t.hide();
    }
}

/// Drive the tray icon's state: "idle" | "active" | "done" | "attention".
/// The icon is the only always-visible surface, so it carries transfer state.
#[tauri::command]
fn set_tray_state(app: AppHandle, state_name: String) {
    set_tray_icon(&app, &state_name);
}

/// Start the nearby session: advertise on the LAN + browse for peers, and
/// spawn the two event pumps (offers in → window events; nothing out needs a
/// pump since offer updates stream through their own channels).
#[tauri::command]
async fn nearby_start(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.core.start_nearby().await.map_err(|e| e.to_string())?;
    spawn_offer_pump(&app, &state);
    Ok(())
}

/// Stop the nearby session (peers see us leave within their TTL).
#[tauri::command]
async fn nearby_stop(state: State<'_, AppState>) -> Result<(), String> {
    state.core.stop_nearby().await;
    Ok(())
}

/// Live snapshot of nearby Dropwire devices.
#[tauri::command]
async fn nearby_list(state: State<'_, AppState>) -> Result<Vec<NearbyDevice>, String> {
    Ok(state.core.nearby_devices().await)
}

/// Offer the send `transfer_id` (the card's id) to a nearby device. Streams
/// `OfferUpdate`s back over the channel; the final update is Accepted /
/// Declined / Failed{reason} / Withdrawn. Returns the offer's id.
#[tauri::command]
async fn nearby_offer(
    endpoint_id: String,
    transfer_id: String,
    on_update: Channel<irohcore::OfferUpdate>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let id = transfer_id
        .parse::<TransferId>()
        .map_err(|e| e.to_string())?;
    let (offer_id, mut stream) = state
        .core
        .offer_nearby(endpoint_id, id)
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        while let Some(u) = stream.next().await {
            let _ = on_update.send(u);
        }
    });
    // The offer's id, for nearby_cancel_offer.
    Ok(offer_id)
}

/// Take back an offer this device sent (the id nearby_offer returned) before
/// it is answered. Its channel then ends with `withdrawn`; the send goes on.
#[tauri::command]
async fn nearby_cancel_offer(offer_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.core.cancel_offer(&offer_id);
    Ok(())
}

/// Answer an incoming offer (both-sides consent: this is the receiver half).
#[tauri::command]
async fn nearby_respond(
    offer_id: String,
    accept: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .core
        .respond_offer(offer_id, accept)
        .await
        .map_err(|e| e.to_string())
}

/// Forward every incoming offer to the webview as `nearby-offer` events, and
/// every offer that ended before it was answered here (the sender took it
/// back, or it expired) as `nearby-offer-withdrawn`, so its dialog can close.
/// One long-lived pump per app run (guarded so repeated nearby_start is cheap).
fn spawn_offer_pump(app: &AppHandle, state: &State<'_, AppState>) {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    emit_all(app.clone(), state.core.subscribe_offers(), "nearby-offer");
    emit_all(
        app.clone(),
        state.core.subscribe_offer_withdrawals(),
        "nearby-offer-withdrawn",
    );
}

/// Emit everything `rx` carries to the webview as `event`, for the app run.
fn emit_all<T>(handle: AppHandle, mut rx: tokio::sync::broadcast::Receiver<T>, event: &'static str)
where
    T: serde::Serialize + Clone + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(item) => {
                    let _ = handle.emit(event, item);
                }
                // Lagged is RECOVERABLE: under a burst we fell behind and lost
                // `n` items, but the receiver keeps working. This pump is a
                // process-wide singleton, so treating Lagged as terminal (the
                // old `while let Ok` did) would silently kill delivery for the
                // whole app run. Keep looping.
                Err(RecvError::Lagged(_)) => continue,
                // The sender was dropped (engine gone) — nothing left to pump.
                Err(RecvError::Closed) => break,
            }
        }
    });
}

/// Local transfer history (newest first).
#[tauri::command]
async fn list_transfers(state: State<'_, AppState>) -> Result<Vec<TransferRecord>, String> {
    Ok(state.core.transfers().await)
}

/// Forget finished history. It only ever existed on this device, so this is the
/// whole delete: nothing has to be revoked anywhere else. Sent text goes with
/// it, except text a send is still serving, which goes when that send ends.
#[tauri::command]
async fn clear_transfers(state: State<'_, AppState>) -> Result<(), String> {
    state.core.clear_transfers().await;
    let left = state.core.transfers().await;
    state.snippets.sweep(left.iter().flat_map(record_sources));
    Ok(())
}

/// Native file/folder picker. Returns absolute paths (empty if cancelled).
#[tauri::command]
async fn pick_paths(
    app: AppHandle,
    directory: bool,
    multiple: bool,
) -> Result<Vec<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dlg = app.dialog().file();
    if directory {
        dlg.pick_folder(move |f| {
            let _ = tx.send(f.into_iter().collect::<Vec<_>>());
        });
    } else if multiple {
        dlg.pick_files(move |f| {
            let _ = tx.send(f.unwrap_or_default());
        });
    } else {
        dlg.pick_file(move |f| {
            let _ = tx.send(f.into_iter().collect::<Vec<_>>());
        });
    }
    let paths = rx.await.map_err(|e| e.to_string())?;
    Ok(paths.into_iter().filter_map(fp_to_string).collect())
}

/// Native "choose a destination folder" picker.
#[tauri::command]
async fn pick_dest_dir(app: AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |f| {
        let _ = tx.send(f);
    });
    let f = rx.await.map_err(|e| e.to_string())?;
    Ok(f.and_then(fp_to_string))
}

/// Render a QR code for the given text as an SVG string (brand colors).
#[tauri::command]
fn qr_svg(text: String) -> Result<String, String> {
    use qrcode::render::svg;
    use qrcode::QrCode;
    let code = QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(160, 160)
        .quiet_zone(true)
        .dark_color(svg::Color("#0e1116"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// Every path a history record was sent from: the single `source`, or each of
/// `sources` for a send of several.
fn record_sources(r: &TransferRecord) -> impl Iterator<Item = &str> {
    r.source
        .as_deref()
        .into_iter()
        .chain(r.sources.iter().map(String::as_str))
}

/// Start sending one or more files and folders under one code. Streams
/// `Progress` over the channel; returns the transfer id.
#[tauri::command]
async fn start_send(
    paths: Vec<String>,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    // Sent text must outlive the send: a large snippet is served straight
    // from its file. The stream ends when the send does.
    let held: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| state.snippets.hold_for_send(p))
        .collect();
    let started = state.core.send_many(paths).await;
    let (snippets, core) = (state.snippets.clone(), state.core.clone());
    let release = move |held: Vec<PathBuf>| async move {
        if held.is_empty() {
            return;
        }
        let records = core.transfers().await;
        for p in &held {
            snippets.release(p, records.iter().flat_map(record_sources));
        }
    };
    let (id, mut stream) = match started {
        Ok(started) => started,
        Err(e) => {
            release(held).await;
            return Err(e.to_string());
        }
    };
    tauri::async_runtime::spawn(async move {
        while let Some(p) = stream.next().await {
            let _ = on_event.send(p);
        }
        release(held).await;
    });
    Ok(id.to_string())
}

/// A receive command's error as the UI gets it: `kind` to choose the help to
/// show (`invalidTicket`, `unreachable`, `alreadyClaimed`, ...) and a plain
/// `message` that can be shown as it is.
#[derive(serde::Serialize)]
struct CommandError {
    kind: &'static str,
    message: String,
}

/// The shell's own refusals before the engine is called: they are all about
/// where to save (no folder to save into, or a path that is not a full one).
impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self {
            kind: "destination",
            message,
        }
    }
}

impl From<irohcore::CoreError> for CommandError {
    fn from(e: irohcore::CoreError) -> Self {
        Self {
            kind: e.kind(),
            message: e.to_string(),
        }
    }
}

/// Preview a ticket's contents (file list, sizes, total, route) WITHOUT
/// downloading any file content. Powers the receive "preview before you accept".
#[tauri::command]
async fn inspect_ticket(
    ticket: String,
    state: State<'_, AppState>,
) -> Result<TransferPreview, CommandError> {
    Ok(state.core.inspect(ticket).await?)
}

/// Where receives land when no folder was chosen: Downloads/Dropwire, or
/// Dropwire in the home folder when the system names no Downloads folder
/// (Linux without xdg-user-dirs). Never a relative path: saving into one fails,
/// and only after the whole download.
fn default_dest() -> Result<PathBuf, String> {
    dirs::download_dir()
        .filter(|d| d.is_absolute())
        .or_else(|| dirs::home_dir().filter(|h| h.is_absolute()))
        .map(|base| base.join("Dropwire"))
        .ok_or_else(|| "There is no folder to save into. Choose one in Settings.".to_string())
}

/// Resolve a receive's destination folder, defaulting to [`default_dest`].
fn dest_or_default(dest: Option<String>) -> Result<PathBuf, String> {
    let Some(d) = dest.filter(|d| !d.trim().is_empty()) else {
        return default_dest();
    };
    let p = PathBuf::from(d);
    if p.is_absolute() {
        return Ok(p);
    }
    // Only the old fallback ("./Dropwire") ever produced a relative folder, and
    // it may still be saved in settings. Resolve it against the home folder,
    // where that default now lives, never against wherever the app started.
    dirs::home_dir()
        .map(|home| home.join(&p).components().collect::<PathBuf>())
        .filter(|abs| abs.is_absolute())
        .ok_or_else(|| "Choose a full folder path to save into.".to_string())
}

/// The default save folder (see [`default_dest`]). The UI shows this and uses
/// it as the path to reveal when a receive used the default destination.
#[tauri::command]
fn default_dest_dir() -> Result<String, String> {
    default_dest().map(|p| p.to_string_lossy().into_owned())
}

/// Pump a transfer's progress stream to the UI channel.
fn pump(mut stream: irohcore::ProgressStream, on_event: Channel<Progress>) {
    tauri::async_runtime::spawn(async move {
        while let Some(p) = stream.next().await {
            let _ = on_event.send(p);
        }
    });
}

/// Start receiving a ticket into `dest` (or the default Downloads/Dropwire folder).
#[tauri::command]
async fn start_receive(
    ticket: String,
    dest: Option<String>,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, CommandError> {
    let (id, stream) = state
        .core
        .receive(ticket, dest_or_default(dest)?)
        .await
        .map_err(CommandError::from)?;
    pump(stream, on_event);
    Ok(id.to_string())
}

/// Start receiving only the chosen files (0-based indices into the preview list).
#[tauri::command]
async fn start_receive_selected(
    ticket: String,
    dest: Option<String>,
    selected: Vec<usize>,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, CommandError> {
    let (id, stream) = state
        .core
        .receive_selected(ticket, dest_or_default(dest)?, selected)
        .await
        .map_err(CommandError::from)?;
    pump(stream, on_event);
    Ok(id.to_string())
}

/// Resume an interrupted or failed receive from history, under the same id,
/// with the code, folder and files it was started with.
#[tauri::command]
async fn resume_transfer(
    id: String,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, CommandError> {
    let tid: TransferId = id.parse().map_err(|_| CommandError {
        kind: "other",
        message: "This transfer is no longer in the history.".into(),
    })?;
    let (id, stream) = state.core.resume(tid).await?;
    pump(stream, on_event);
    Ok(id.to_string())
}

/// Send a one-shot control message to the sender (e.g. an instant decline).
/// A decline names the code, so the sender can release it for someone else.
#[tauri::command]
async fn send_control(
    ticket: String,
    kind: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let msg = match kind.as_str() {
        "decline" => return state.core.decline(ticket).await.map_err(|e| e.to_string()),
        "ack" => CtrlMsg::Ack,
        "hello" => CtrlMsg::Hello,
        other => return Err(format!("unknown control kind: {other}")),
    };
    state
        .core
        .send_control(ticket, msg)
        .await
        .map_err(|e| e.to_string())
}

/// Cancel an in-flight transfer.
#[tauri::command]
async fn cancel_transfer(id: String, state: State<'_, AppState>) -> Result<(), String> {
    let tid: TransferId = id.parse().map_err(|_| "invalid transfer id".to_string())?;
    state.core.cancel(tid).await;
    Ok(())
}

/// What [`reveal_path`] shows: a folder opened, or a file selected in its folder.
#[derive(Debug, PartialEq)]
enum Reveal {
    Folder(PathBuf),
    File(PathBuf),
}

const REVEAL_GONE: &str = "That folder is no longer there. It may have been moved or deleted.";

/// Check a path to reveal. It must be absolute and exist; a missing folder is
/// an error rather than letting the file manager open somewhere else instead
/// (Explorer falls back to Documents).
fn reveal_target(path: &str) -> Result<Reveal, String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err(REVEAL_GONE.into());
    }
    // Normalized (on Windows: backslashes, no `..`) without the `\\?\` prefix
    // that canonicalize adds, which Explorer does not understand.
    let p = std::path::absolute(p).map_err(|_| REVEAL_GONE.to_string())?;
    match std::fs::metadata(&p) {
        Ok(m) if m.is_dir() => Ok(Reveal::Folder(p)),
        Ok(m) if m.is_file() => Ok(Reveal::File(p)),
        _ => Err(REVEAL_GONE.into()),
    }
}

/// Show a folder in the system file manager, or select a file in its folder.
/// It never opens a file: explorer, open and xdg-open all launch a file they
/// are handed, so a file is only ever selected, never passed on its own.
#[tauri::command]
fn reveal_path(path: String) -> Result<(), String> {
    let target = reveal_target(&path)?;
    spawn_reveal(&target).map_err(|e| format!("Could not open the folder: {e}"))
}

#[cfg(target_os = "windows")]
fn spawn_reveal(target: &Reveal) -> std::io::Result<()> {
    use std::ffi::OsString;
    use std::os::windows::process::CommandExt;
    // Quoted by hand: Explorer splits an unquoted argument at commas.
    // Windows paths cannot contain a quote, so this cannot be broken out of.
    let (lead, p) = match target {
        Reveal::Folder(p) => ("\"", p),
        Reveal::File(p) => ("/select,\"", p),
    };
    let mut arg = OsString::from(lead);
    arg.push(p.as_os_str());
    arg.push("\"");
    std::process::Command::new("explorer")
        .raw_arg(arg)
        .spawn()
        .map(drop)
}

#[cfg(target_os = "macos")]
fn spawn_reveal(target: &Reveal) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new("open");
    match target {
        // A folder with an extension may be a bundle (Some.app), which
        // `open` would launch, so those are selected in Finder instead.
        Reveal::Folder(p) if p.extension().is_none() => cmd.arg(p),
        Reveal::Folder(p) | Reveal::File(p) => cmd.arg("-R").arg(p),
    };
    cmd.spawn().map(drop)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_reveal(target: &Reveal) -> std::io::Result<()> {
    // There is no portable "select this file"; open its folder instead.
    let folder = match target {
        Reveal::Folder(p) => p.as_path(),
        Reveal::File(p) => p.parent().unwrap_or(p.as_path()),
    };
    std::process::Command::new("xdg-open")
        .arg(folder)
        .spawn()
        .map(drop)
}

#[cfg(not(any(unix, windows)))]
fn spawn_reveal(_target: &Reveal) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported on this system"))
}

/// A link [`open_external`] may hand to the system: a well-formed https URL
/// to a named host, with no credentials and nothing Explorer's command line
/// could misread (it splits at commas and quotes).
fn checked_https_url(raw: &str) -> Option<String> {
    // Spelled out in full: the parser would also accept "https:host" and
    // repair other malformed input.
    let spelled_out = raw
        .get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"));
    if !spelled_out || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let url = tauri::Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.domain().is_none_or(|d| !d.contains('.'))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let s = url.as_str();
    if s.contains([',', '"', '\'', '\\', '<', '>', '`']) {
        return None;
    }
    Some(s.to_string())
}

/// Open a web link in the user's default browser. Only well-formed https
/// links: the app's own UI passes its source and profile links, and nothing
/// else should reach the system's link handler.
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let url = checked_https_url(&url).ok_or_else(|| "That link cannot be opened.".to_string())?;
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();
    #[cfg(not(any(unix, windows)))]
    let spawned: std::io::Result<std::process::Child> =
        Err(std::io::Error::other("not supported on this system"));
    spawned
        .map(drop)
        .map_err(|e| format!("Could not open the link: {e}"))
}

/// The app version (from the crate/workspace version, kept in sync with tauri.conf).
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Where a panic breadcrumb is written. Honors `DROPWIRE_DATA_DIR`; otherwise
/// the OS data dir. Note this is resolved WITHOUT a Tauri handle (the hook is
/// installed before the app exists), so in the default case it lands beside —
/// not inside — Tauri's identifier-scoped `app_data_dir()`.
fn panic_log_path() -> Option<PathBuf> {
    let base = match std::env::var("DROPWIRE_DATA_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::data_dir()?.join("dropwire"),
    };
    Some(base.join("panic.log"))
}

/// Record panics to a file (and still run the default hook). Release builds are
/// stripped, so a native crash report won't symbolicate; this leaves a readable
/// `thread '…' panicked at …` breadcrumb for beta reports. Combined with
/// `panic = "unwind"`, a panic in a background mDNS thread degrades discovery
/// instead of aborting the whole process.
fn install_panic_logger() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        append_breadcrumb(&format!("[panic] thread '{name}': {info}"));
        default(info);
    }));
}

/// Append one line to the breadcrumb log (see `panic_log_path`). Best effort:
/// failing to write it must never stop anything else.
fn append_breadcrumb(line: &str) {
    let Some(path) = panic_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// True when the engine failed to start only because another copy of Dropwire
/// still holds this data folder's blob store (its database lock).
fn is_store_locked(err: &CoreError) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if e.to_string().contains("already open") {
            return true;
        }
        cur = e.source();
    }
    format!("{err:?}").contains("already open")
}

/// Start the engine. If the blob store is still locked, the likely cause is a
/// copy of Dropwire that is shutting down (it was just quit, and this is the
/// relaunch), so wait a few seconds for it to let go before giving up.
fn start_engine(config: impl Fn() -> CoreConfig) -> Result<Core, CoreError> {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match tauri::async_runtime::block_on(Core::start(config())) {
            Err(e) if is_store_locked(&e) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(750));
            }
            result => return result,
        }
    }
}

/// The engine could not start. Without this, the setup error becomes a panic
/// that nobody sees (release builds have no console) and the app just never
/// appears. Say what happened in a native dialog, then exit.
fn report_start_failure(app: &AppHandle, err: &CoreError) {
    use tauri_plugin_dialog::MessageDialogKind;
    append_breadcrumb(&format!("[start] engine failed to start: {err:#}"));
    let body = if is_store_locked(err) {
        "Dropwire is already running on this computer. Look for its icon in the \
         system tray or menu bar. If you just quit it, wait a moment and open it again."
            .to_string()
    } else {
        format!("Dropwire could not start.\n\n{err:#}")
    };
    let handle = app.clone();
    app.dialog()
        .message(body)
        .title("Dropwire")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}

/// Swap the tray icon for the given state name. The tray is the only surface
/// that is always visible, so it reports what the app is doing: dim at rest,
/// lime while transferring, green on completion, amber when a decision is due.
fn set_tray_icon(app: &AppHandle, state_name: &str) {
    let bytes: &[u8] = match state_name {
        "active" => include_bytes!("../icons/tray-active.png"),
        "done" => include_bytes!("../icons/tray-done.png"),
        "attention" => include_bytes!("../icons/tray-attention.png"),
        _ => include_bytes!("../icons/tray-idle.png"),
    };
    if let Some(tray) = app.tray_by_id("dropwire") {
        if let Ok(img) = tauri::image::Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
    }
}

/// A rectangle in one coordinate space: points on macOS, physical pixels on
/// Windows and Linux.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Area {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Top-left corner for a `panel`-sized tray panel opened by a click at `click`.
/// It is centered on the click, above it when the click is in the lower half of
/// the work area (a bottom taskbar) and below it otherwise (a top menu bar),
/// then pulled inside the work area, `margin` from its edges. That also keeps
/// it off side taskbars and out from under a notched menu bar. A work area
/// smaller than the panel pins it to the top-left corner.
fn place_panel(
    click: (f64, f64),
    work: Area,
    panel: (f64, f64),
    gap: f64,
    margin: f64,
) -> (f64, f64) {
    let (w, h) = panel;
    let x = click.0 - w / 2.0;
    let y = if click.1 > work.y + work.h / 2.0 {
        click.1 - h - gap
    } else {
        click.1 + gap
    };
    // Plain min/max rather than clamp(): clamp panics when the panel is
    // larger than the work area.
    let fit = |v: f64, lo: f64, span: f64, size: f64| {
        v.max(lo + margin).min(lo + span - size - margin).max(lo)
    };
    (fit(x, work.x, work.w, w), fit(y, work.y, work.h, h))
}

/// Put the tray panel next to the tray icon that was clicked, on the monitor
/// that holds the icon, inside that monitor's work area.
fn position_tray_panel(
    app: &AppHandle,
    win: &tauri::WebviewWindow,
    click: tauri::PhysicalPosition<f64>,
) {
    // macOS reports monitor geometry as points times each monitor's own scale,
    // so only points line up across mixed-scale displays. The cursor position
    // there is points times the primary monitor's scale. Windows reports
    // everything, click included, in one physical virtual-screen space.
    #[cfg(target_os = "macos")]
    let (per_monitor_unit, (cx, cy)) = {
        let primary = app
            .primary_monitor()
            .ok()
            .flatten()
            .map_or(1.0, |m| m.scale_factor());
        let c = app.cursor_position().unwrap_or(click);
        (true, (c.x / primary, c.y / primary))
    };
    #[cfg(not(target_os = "macos"))]
    let (per_monitor_unit, (cx, cy)) = (false, (click.x, click.y));

    let unit = |m: &tauri::Monitor| {
        if per_monitor_unit {
            m.scale_factor()
        } else {
            1.0
        }
    };
    let bounds = |m: &tauri::Monitor| {
        let k = unit(m);
        Area {
            x: f64::from(m.position().x) / k,
            y: f64::from(m.position().y) / k,
            w: f64::from(m.size().width) / k,
            h: f64::from(m.size().height) / k,
        }
    };
    let monitor = app
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .find(|m| bounds(m).contains(cx, cy))
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else {
        return;
    };
    let k = unit(&m);
    let wa = m.work_area();
    let work = Area {
        x: f64::from(wa.position.x) / k,
        y: f64::from(wa.position.y) / k,
        w: f64::from(wa.size.width) / k,
        h: f64::from(wa.size.height) / k,
    };
    // Panel size and spacing in logical px, scaled into the same space. The
    // size comes from the window itself (tauri.conf.json), measured in its
    // current scale.
    let logical =
        win.outer_size()
            .ok()
            .zip(win.scale_factor().ok())
            .map_or((360.0, 470.0), |(size, s)| {
                let l = size.to_logical::<f64>(s);
                (l.width, l.height)
            });
    let s = m.scale_factor() / k;
    let (x, y) = place_panel(
        (cx, cy),
        work,
        (logical.0 * s, logical.1 * s),
        12.0 * s,
        8.0 * s,
    );
    if per_monitor_unit {
        let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    } else {
        let _ = win.set_position(tauri::PhysicalPosition::new(
            x.round() as i32,
            y.round() as i32,
        ));
    }
}

/// The tray panel's transient state. The panel closes a moment after it loses
/// focus rather than at once: taking a file to "Drop to send" means clicking
/// into a file manager first, and a panel that vanished on that click could
/// never receive the drop. Focus coming back or a drag over the panel cancels
/// the pending close.
struct TrayPanel {
    /// Bumped whenever the panel is shown, refocused or dragged over, so a
    /// pending delayed close can tell it has been overtaken.
    generation: std::sync::atomic::AtomicU64,
    /// A drag is over the panel right now.
    dragging: std::sync::atomic::AtomicBool,
    /// When the delayed close last hid the panel.
    auto_hidden_at: std::sync::Mutex<Option<std::time::Instant>>,
}

static TRAY_PANEL: TrayPanel = TrayPanel {
    generation: std::sync::atomic::AtomicU64::new(0),
    dragging: std::sync::atomic::AtomicBool::new(false),
    auto_hidden_at: std::sync::Mutex::new(None),
};

/// How long the panel stays up after losing focus.
const TRAY_BLUR_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Cancel any pending delayed close. Returns the new generation.
fn tray_cancel_close() -> u64 {
    TRAY_PANEL
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        + 1
}

/// Close the panel after the grace period, unless something overtakes it.
fn tray_close_later(win: &tauri::Window) {
    use std::sync::atomic::Ordering;
    let generation = tray_cancel_close();
    let win = win.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(TRAY_BLUR_GRACE).await;
        if TRAY_PANEL.generation.load(Ordering::SeqCst) == generation
            && !TRAY_PANEL.dragging.load(Ordering::SeqCst)
        {
            *TRAY_PANEL
                .auto_hidden_at
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
            let _ = win.hide();
        }
    });
}

/// Keep the tray panel's delayed close in step with focus and drags.
fn on_tray_panel_event(win: &tauri::Window, event: &tauri::WindowEvent) {
    use std::sync::atomic::Ordering;
    use tauri::{DragDropEvent, WindowEvent};
    match event {
        WindowEvent::Focused(false) => tray_close_later(win),
        WindowEvent::Focused(true) => {
            tray_cancel_close();
        }
        WindowEvent::DragDrop(DragDropEvent::Enter { .. } | DragDropEvent::Over { .. }) => {
            TRAY_PANEL.dragging.store(true, Ordering::SeqCst);
            tray_cancel_close();
        }
        // The drag left, or was dropped (a drop with files opens the main
        // window, which closes the panel): back to closing on blur.
        WindowEvent::DragDrop(DragDropEvent::Leave | DragDropEvent::Drop { .. }) => {
            TRAY_PANEL.dragging.store(false, Ordering::SeqCst);
            if !win.is_focused().unwrap_or(false) {
                tray_close_later(win);
            }
        }
        _ => {}
    }
}

/// Toggle the tray panel: hide it if it is open, otherwise open it next to the
/// tray icon.
fn toggle_tray_window(app: &AppHandle, at: tauri::PhysicalPosition<f64>) {
    let Some(win) = app.get_webview_window("tray") else {
        return;
    };
    if win.is_visible().unwrap_or(false) {
        tray_cancel_close();
        let _ = win.hide();
        return;
    }
    // Pressing the icon takes focus from the panel. If the delayed close
    // happened to fire just before this click, the click was meant to close
    // the panel, not to open it again.
    let just_closed = TRAY_PANEL
        .auto_hidden_at
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
    if just_closed {
        return;
    }
    tray_cancel_close();
    TRAY_PANEL
        .dragging
        .store(false, std::sync::atomic::Ordering::SeqCst);
    position_tray_panel(app, &win, at);
    let _ = win.show();
    let _ = win.set_focus();
}

/// Set once the tray icon exists. Hiding the window on close is only safe when
/// there is a tray icon to bring it back from.
static TRAY_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Build the tray icon and its menu.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{TrayIconBuilder, TrayIconEvent};

    let open = MenuItem::with_id(app, "open", "Open Dropwire", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::with_id("dropwire")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../icons/tray-idle.png"
        ))?)
        .tooltip("Dropwire")
        .menu(&menu)
        // Left-click opens the panel; the menu stays on right-click only.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_tray_window(tray.app_handle(), position);
            }
        })
        .build(app)?;
    Ok(())
}

/// Stop the engine on the way out. Every exit path (tray Quit, closing the
/// window with the tray off, Cmd+Q) ends in `RunEvent::Exit`, which is the one
/// place this runs. The nearby goodbye goes first so peers drop this device
/// from their lists at once instead of showing it until their records expire;
/// then connections close cleanly and the blob store commits its last batch.
/// Each step is bounded so quitting never hangs on a slow peer.
fn shutdown_engine(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // The event loop is blocked below, so get out of sight first: a window or
    // tray icon that stops responding for a few seconds reads as a hang.
    for w in app.webview_windows().values() {
        let _ = w.hide();
    }
    let _ = app.remove_tray_by_id("dropwire");
    let core = state.core.clone();
    tauri::async_runtime::block_on(async move {
        use std::time::Duration;
        let _ = tokio::time::timeout(Duration::from_secs(2), core.stop_nearby()).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), core.shutdown()).await;
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_logger();
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // One running copy per data dir. Registered first, so a second launch (the
    // Start menu, a desktop shortcut, clicking a notification) hands off to the
    // running app, which brings its window forward, and exits before it ever
    // touches the identity, the endpoint or the blob store. A copy started with
    // its own --data-dir is left alone, so two can still run side by side for
    // testing the nearby flow.
    #[cfg(desktop)]
    if std::env::var_os("DROPWIRE_DATA_DIR").is_none_or(|d| d.is_empty()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A login launch that finds Dropwire already running has nothing
            // to add; anything else is someone opening the app.
            if !argv.iter().any(|a| a == LOGIN_LAUNCH_ARG) {
                show_main(app.clone());
            }
        }));
    }
    // "Start at login": the login entry launches with LOGIN_LAUNCH_ARG so the
    // app can tell a login launch from someone opening it.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![LOGIN_LAUNCH_ARG]),
        ));
    }
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // App data dir: identity (node.key), blob store, transfer catalog.
            // DROPWIRE_DATA_DIR overrides it — lets a second instance run side-
            // by-side on one machine for testing the nearby flow end-to-end.
            let data_dir = match std::env::var("DROPWIRE_DATA_DIR") {
                Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
                _ => app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("dropwire"),
            };
            // Build the engine inside Tauri's tokio runtime. Serverless by default:
            // DHT discovery + n0 free relay fallback.
            let store = settings::Store::load(&data_dir);
            let prefs = store.get();
            let core = match start_engine(|| CoreConfig::serverless(data_dir.clone())) {
                Ok(core) => core,
                Err(e) => {
                    // No engine, no app: nothing is managed and no tray is
                    // built. The dialog's callback exits.
                    report_start_failure(app.handle(), &e);
                    return Ok(());
                }
            };
            // Sent text lives beside the history that can resend it. Delete
            // what no record uses (left by a crash, or by a send that was
            // still serving when history was cleared), and the folder older
            // versions wrote to the cache folder and never cleaned up.
            let snippets = Arc::new(snippets::Snippets::new(data_dir.join("sent-text")));
            let records = tauri::async_runtime::block_on(core.transfers());
            snippets.sweep(records.iter().flat_map(record_sources));
            // The old folder belongs to the regular install; a copy started
            // with its own --data-dir leaves it alone.
            if std::env::var_os("DROPWIRE_DATA_DIR").is_none_or(|d| d.is_empty()) {
                if let Ok(cache) = app.path().app_cache_dir() {
                    let _ = std::fs::remove_dir_all(cache.join("dropwire-text"));
                }
                let _ = std::fs::remove_dir_all(std::env::temp_dir().join("dropwire-text"));
            }
            // A name chosen during setup outlives the hostname it was derived from.
            if let Some(name) = prefs.device_name.clone() {
                let c = core.clone();
                let used = tauri::async_runtime::block_on(async move {
                    c.set_device_name(name).await?;
                    Ok::<_, irohcore::CoreError>(c.device_name().await)
                });
                match used {
                    // Saved as the engine tidied it, so both show the same.
                    Ok(used) if prefs.device_name.as_deref() != Some(used.as_str()) => {
                        store.update(|s| s.device_name = Some(used));
                    }
                    Ok(_) => {}
                    // A name an older version saved that this one refuses
                    // (too long, say): fall back to the hostname name rather
                    // than keep showing one that nobody nearby sees.
                    Err(e) => {
                        eprintln!(
                            "[dropwire] saved device name not usable ({e}); using the default"
                        );
                        store.update(|s| s.device_name = None);
                    }
                }
            }
            // The system's startup list is the truth for "Start at login": the
            // entry may have been removed or disabled outside the app (Task
            // Manager, Login Items), so the switch follows what is registered.
            if let Some(on) = start_at_login_registered(app.handle()) {
                if on != prefs.start_at_login {
                    store.update(|s| s.start_at_login = on);
                }
            }
            app.manage(AppState {
                core,
                settings: store,
                snippets,
            });

            // A missing tray is not worth refusing to start over, but without
            // one, closing the window must quit rather than hide it for good.
            match build_tray(app.handle()) {
                Ok(()) => TRAY_READY.store(true, std::sync::atomic::Ordering::Relaxed),
                Err(e) => append_breadcrumb(&format!("[start] tray icon unavailable: {e}")),
            }

            // A login launch starts quietly in the tray. The window still opens
            // when setup has not been finished yet (the setup screens need it)
            // and when there is no tray icon to open it from later.
            let login_launch = std::env::args().any(|a| a == LOGIN_LAUNCH_ARG);
            let start_in_tray = login_launch
                && prefs.onboarded
                && TRAY_READY.load(std::sync::atomic::Ordering::Relaxed);
            if let Some(w) = app.get_webview_window("main") {
                keep_title_bar_on_screen(&w);
                if !start_in_tray {
                    let _ = w.show();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Close means "get out of the way", not "quit", when the tray is on:
            // nearby devices can only reach you while the app is running.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let keep = TRAY_READY.load(std::sync::atomic::Ordering::Relaxed)
                        && window
                            .app_handle()
                            .try_state::<AppState>()
                            .map(|s| s.settings.get().tray_on_close)
                            .unwrap_or(false);
                    api.prevent_close();
                    if keep {
                        let _ = window.hide();
                    } else {
                        // With the tray off, closing the window is quitting.
                        // Letting just this window go is not enough: the hidden
                        // tray panel keeps the process alive, still serving and
                        // advertising, with no way back to the window.
                        window.app_handle().exit(0);
                    }
                } else if window.label() == "tray" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // The tray panel is transient: it closes shortly after it loses
            // focus, unless a drag is on its way to it.
            if window.label() == "tray" {
                on_tray_panel_event(window, event);
            }
        })
        .invoke_handler(tauri::generate_handler![
            my_endpoint_id,
            my_fingerprint,
            device_name,
            get_settings,
            set_device_name,
            set_pref,
            trust_remember,
            trust_forget,
            write_text_file,
            show_main,
            hide_tray_window,
            set_tray_state,
            nearby_start,
            nearby_stop,
            nearby_list,
            nearby_offer,
            nearby_cancel_offer,
            nearby_respond,
            list_transfers,
            clear_transfers,
            pick_paths,
            pick_dest_dir,
            default_dest_dir,
            qr_svg,
            start_send,
            inspect_ticket,
            start_receive,
            start_receive_selected,
            resume_transfer,
            send_control,
            cancel_transfer,
            reveal_path,
            open_external,
            app_version
        ])
        .build(tauri::generate_context!())
        .expect("error while building Dropwire")
        .run(|app, event| match event {
            tauri::RunEvent::Exit => shutdown_engine(app),
            // Nothing can bring the main window back once it is destroyed, so
            // however that happens, the app goes with it rather than running on
            // unreachable behind the hidden tray panel.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Destroyed,
                ..
            } if label == "main" => app.exit(0),
            // macOS: with close-to-tray on, closing the window leaves Dropwire
            // running with no window. Clicking the dock icon raises Reopen, and
            // with nothing answering it the app is alive but unreachable: it
            // reads as frozen and the only way out is Cmd+Q. Bring it back.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main(app.clone()),
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dropwire-shell-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A second engine on the same data folder fails on the store lock, and
    /// that failure is recognized, so the relaunch waits instead of dying.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_held_store_lock_is_recognized() {
        let dir = scratch_dir("lock");
        let first = Core::start(CoreConfig::local_only(&dir))
            .await
            .expect("first start");
        let err = Core::start(CoreConfig::local_only(&dir))
            .await
            .err()
            .expect("a second engine on the same folder must not start");
        assert!(is_store_locked(&err), "not recognized as a lock: {err:?}");
        first.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default save folder is always absolute, whatever the system reports.
    #[test]
    fn the_default_destination_is_absolute() {
        if let Ok(p) = default_dest() {
            assert!(p.is_absolute(), "{p:?}");
            assert!(p.ends_with("Dropwire"), "{p:?}");
        }
        if let Ok(p) = dest_or_default(None) {
            assert!(p.is_absolute(), "{p:?}");
        }
        if let Ok(p) = dest_or_default(Some("   ".into())) {
            assert!(p.is_absolute(), "{p:?}");
        }
    }

    /// A chosen absolute folder is used as it is; the old relative fallback
    /// saved in settings resolves into the home folder, not the working dir.
    #[test]
    fn a_relative_destination_never_reaches_the_engine() {
        let chosen = std::env::temp_dir().join("Dropwire test");
        let got = dest_or_default(Some(chosen.to_string_lossy().into_owned())).expect("absolute");
        assert_eq!(got, chosen);

        if let Some(home) = dirs::home_dir() {
            let got = dest_or_default(Some("./Dropwire".into())).expect("resolved");
            assert!(got.is_absolute(), "{got:?}");
            assert_eq!(got, home.join("Dropwire"));
        }
    }

    /// Names listed between `open` and the next `close` after it, unquoted.
    fn listed(text: &str, open: &str, close: &str) -> Vec<String> {
        let start = text.find(open).expect("list start") + open.len();
        let len = text[start..].find(close).expect("list end");
        let mut names: Vec<String> = text[start..start + len]
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        names.sort();
        names
    }

    /// App commands granted by a capability file, as command names.
    fn granted(capability: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(capability).expect("capability json");
        let mut names: Vec<String> = v["permissions"]
            .as_array()
            .expect("permissions")
            .iter()
            .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
            .map(|c| c.replace('-', "_"))
            .collect();
        names.sort();
        names
    }

    /// With an app manifest, a command runs only where a capability grants
    /// it. One added to the handler but not to build.rs and a capability
    /// would fail as "not allowed" at runtime, so check that they agree, and
    /// that the tray panel gets no more than the few commands it uses.
    #[test]
    fn every_command_is_declared_and_granted() {
        let handler = listed(
            include_str!("lib.rs"),
            concat!(".invoke_handler(tauri::", "generate_handler!["),
            "]",
        );
        assert!(handler.iter().any(|c| c == "start_send"), "{handler:?}");
        let declared = listed(include_str!("../build.rs"), "&[&str] = &[", "];");
        assert_eq!(
            handler, declared,
            "build.rs COMMANDS must match the handler"
        );

        let main = granted(include_str!("../capabilities/default.json"));
        let tray = granted(include_str!("../capabilities/tray.json"));
        for cmd in &handler {
            assert!(
                main.contains(cmd) || tray.contains(cmd),
                "{cmd} is granted to no window"
            );
        }
        assert_eq!(
            tray,
            [
                "hide_tray_window",
                "list_transfers",
                "my_endpoint_id",
                "show_main"
            ],
            "the tray panel's commands"
        );
    }

    /// A folder is opened, a file is only ever selected, and anything missing
    /// or relative is an error instead of the file manager's fallback folder.
    #[test]
    fn reveal_opens_folders_selects_files_and_refuses_the_rest() {
        let dir = scratch_dir("reveal");
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("setup.exe");
        std::fs::write(&file, b"not really").expect("file");
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();

        assert_eq!(reveal_target(&s(&dir)), Ok(Reveal::Folder(dir.clone())));
        assert_eq!(reveal_target(&s(&file)), Ok(Reveal::File(file.clone())));
        assert_eq!(
            reveal_target(&s(&dir.join("moved away"))),
            Err(REVEAL_GONE.to_string())
        );
        assert_eq!(reveal_target("Dropwire"), Err(REVEAL_GONE.to_string()));
        assert_eq!(reveal_target(""), Err(REVEAL_GONE.to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_plain_https_links_are_opened() {
        for ok in [
            "https://github.com/muhamadjawdatsalemalakoum/dropwire",
            "https://www.linkedin.com/in/akoum/",
            "HTTPS://github.com/x?y=1#z",
        ] {
            assert!(checked_https_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "http://github.com/",
            "mailto:someone@example.com",
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "https://github.com/a,b",
            "https://github.com/a b",
            " https://github.com/",
            "https://user:pass@github.com/",
            "https://127.0.0.1/",
            "https://localhost/",
            "https:github.com",
            "",
        ] {
            assert!(checked_https_url(bad).is_none(), "{bad}");
        }
    }

    const PANEL: (f64, f64) = (360.0, 470.0);

    fn inside(p: (f64, f64), work: Area, margin: f64) -> bool {
        p.0 >= work.x + margin
            && p.1 >= work.y + margin
            && p.0 + PANEL.0 <= work.x + work.w - margin
            && p.1 + PANEL.1 <= work.y + work.h - margin
    }

    /// Windows, taskbar at the bottom: the click is below the work area, so the
    /// panel opens above it and stays clear of the taskbar and the right edge.
    #[test]
    fn panel_clears_a_bottom_taskbar() {
        let work = Area {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1032.0,
        };
        let p = place_panel((1800.0, 1060.0), work, PANEL, 12.0, 8.0);
        assert!(inside(p, work, 8.0), "{p:?}");
        assert!(
            p.1 + PANEL.1 < 1060.0,
            "panel must open above the icon: {p:?}"
        );
    }

    /// Taskbar on the left or right: the panel is pushed off it horizontally.
    #[test]
    fn panel_clears_a_side_taskbar() {
        let left = Area {
            x: 62.0,
            y: 0.0,
            w: 1858.0,
            h: 1080.0,
        };
        let p = place_panel((30.0, 1000.0), left, PANEL, 12.0, 8.0);
        assert!(inside(p, left, 8.0), "{p:?}");

        let right = Area {
            x: 0.0,
            y: 0.0,
            w: 1858.0,
            h: 1080.0,
        };
        let p = place_panel((1890.0, 1000.0), right, PANEL, 12.0, 8.0);
        assert!(inside(p, right, 8.0), "{p:?}");
    }

    /// macOS menu bar (a notched one is taller): the click is above the work
    /// area, so the panel opens below the menu bar, not under it.
    #[test]
    fn panel_opens_below_a_top_menu_bar() {
        let work = Area {
            x: 0.0,
            y: 37.0,
            w: 1512.0,
            h: 945.0,
        };
        let p = place_panel((1300.0, 12.0), work, PANEL, 12.0, 8.0);
        assert!(inside(p, work, 8.0), "{p:?}");
        assert!(p.1 >= 37.0 + 8.0, "{p:?}");
    }

    /// A secondary display left of the primary has negative coordinates.
    #[test]
    fn panel_stays_on_a_display_with_negative_coordinates() {
        let work = Area {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1040.0,
        };
        let p = place_panel((-100.0, 1060.0), work, PANEL, 12.0, 8.0);
        assert!(inside(p, work, 8.0), "{p:?}");
    }

    /// A work area smaller than the panel (a 1080p screen at 250 percent is
    /// 432 points tall) must not panic; the panel starts at the top-left.
    #[test]
    fn panel_on_a_tiny_screen_does_not_panic() {
        let work = Area {
            x: 0.0,
            y: 0.0,
            w: 768.0,
            h: 408.0,
        };
        let p = place_panel((700.0, 420.0), work, PANEL, 12.0, 8.0);
        assert!(p.0 >= 0.0 && p.0 + PANEL.0 <= 768.0, "{p:?}");
        assert_eq!(p.1, 0.0, "{p:?}");

        let work = Area {
            x: 100.0,
            y: 50.0,
            w: 300.0,
            h: 200.0,
        };
        let p = place_panel((250.0, 240.0), work, PANEL, 12.0, 8.0);
        assert_eq!(p, (100.0, 50.0));
    }

    #[test]
    fn area_contains_its_top_left_but_not_its_far_edges() {
        let a = Area {
            x: -10.0,
            y: 5.0,
            w: 20.0,
            h: 10.0,
        };
        assert!(a.contains(-10.0, 5.0));
        assert!(a.contains(9.9, 14.9));
        assert!(!a.contains(10.0, 5.0));
        assert!(!a.contains(0.0, 15.0));
    }

    /// Any other start failure is reported as it is, not as "already running".
    #[tokio::test(flavor = "multi_thread")]
    async fn other_start_failures_are_not_mistaken_for_a_lock() {
        let dir = scratch_dir("notadir");
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("temp dir");
        std::fs::write(&dir, b"a file where the data folder should be").expect("write");
        let err = Core::start(CoreConfig::local_only(&dir))
            .await
            .err()
            .expect("a data folder that is a file must fail");
        assert!(!is_store_locked(&err), "mistaken for a lock: {err:?}");
        let _ = std::fs::remove_file(&dir);
    }
}
