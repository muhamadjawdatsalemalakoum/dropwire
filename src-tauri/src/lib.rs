//! Dropwire desktop shell (Tauri v2).
//!
//! Deliberately thin: it owns the Tauri app, the window, and the command surface,
//! and forwards everything to the verified `irohcore` engine. No iroh-blobs types
//! appear here — only `irohcore`'s stable API.

use std::path::PathBuf;

use irohcore::{
    Core, CoreConfig, CtrlMsg, NearbyDevice, Progress, TransferId, TransferPreview, TransferRecord,
};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio_stream::StreamExt;

/// Long-lived app state: the single engine handle.
struct AppState {
    core: Core,
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

/// Offer the active send to a nearby device. Streams `OfferUpdate`s back over
/// the channel; the final update is Accepted / Declined / Failed{reason}.
#[tauri::command]
async fn nearby_offer(
    endpoint_id: String,
    on_update: Channel<irohcore::OfferUpdate>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (_id, mut stream) = state
        .core
        .offer_nearby(endpoint_id)
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        while let Some(u) = stream.next().await {
            let _ = on_update.send(u);
        }
    });
    // The engine's offer id is internal; the UI keys off its own card id.
    Ok(String::new())
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

/// Forward every incoming offer to the webview as `nearby-offer` events.
/// One long-lived pump per app run (guarded so repeated nearby_start is cheap).
fn spawn_offer_pump(app: &AppHandle, state: &State<'_, AppState>) {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let mut rx = state.core.subscribe_offers();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(offer) => {
                    let _ = handle.emit("nearby-offer", offer);
                }
                // Lagged is RECOVERABLE: under a burst of offers we fell behind
                // and lost `n` of them, but the receiver keeps working. This
                // pump is a process-wide singleton, so treating Lagged as
                // terminal (the old `while let Ok` did) would silently kill
                // incoming-offer delivery for the whole app run. Keep looping.
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

/// Start sending a file or folder. Streams `Progress` over the channel; returns the transfer id.
#[tauri::command]
async fn start_send(
    path: String,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (id, mut stream) = state
        .core
        .send(PathBuf::from(path))
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        while let Some(p) = stream.next().await {
            let _ = on_event.send(p);
        }
    });
    Ok(id.to_string())
}

/// Preview a ticket's contents (file list, sizes, total, route) WITHOUT
/// downloading any file content. Powers the receive "preview before you accept".
#[tauri::command]
async fn inspect_ticket(
    ticket: String,
    state: State<'_, AppState>,
) -> Result<TransferPreview, String> {
    state.core.inspect(ticket).await.map_err(|e| e.to_string())
}

/// Resolve a destination directory, defaulting to Downloads/Dropwire.
fn dest_or_default(dest: Option<String>) -> PathBuf {
    match dest {
        Some(d) => PathBuf::from(d),
        None => dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Dropwire"),
    }
}

/// The default save folder (Downloads/Dropwire). The UI shows this and uses it as
/// the path to reveal when a receive used the default destination.
#[tauri::command]
fn default_dest_dir() -> String {
    dest_or_default(None).to_string_lossy().into_owned()
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
) -> Result<String, String> {
    let (id, stream) = state
        .core
        .receive(ticket, dest_or_default(dest))
        .await
        .map_err(|e| e.to_string())?;
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
) -> Result<String, String> {
    let (id, stream) = state
        .core
        .receive_selected(ticket, dest_or_default(dest), selected)
        .await
        .map_err(|e| e.to_string())?;
    pump(stream, on_event);
    Ok(id.to_string())
}

/// Send a one-shot control message to the sender (e.g. an instant decline).
#[tauri::command]
async fn send_control(
    ticket: String,
    kind: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let msg = match kind.as_str() {
        "decline" => CtrlMsg::Decline,
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

/// Open a path in the OS file manager.
#[tauri::command]
fn reveal_path(path: String) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
}

/// Open a web link in the user's default browser. Only http(s)/mailto are allowed;
/// the URLs come from the app's own UI (credits, source, profile).
#[tauri::command]
fn open_external(url: String) {
    if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) {
        return;
    }
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
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
        if let Some(path) = panic_log_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                use std::io::Write;
                let thread = std::thread::current();
                let name = thread.name().unwrap_or("<unnamed>");
                let _ = writeln!(f, "[panic] thread '{name}': {info}");
            }
        }
        default(info);
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_logger();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            let core =
                tauri::async_runtime::block_on(Core::start(CoreConfig::serverless(data_dir)))?;
            app.manage(AppState { core });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            my_endpoint_id,
            my_fingerprint,
            nearby_start,
            nearby_stop,
            nearby_list,
            nearby_offer,
            nearby_respond,
            list_transfers,
            pick_paths,
            pick_dest_dir,
            default_dest_dir,
            qr_svg,
            start_send,
            inspect_ticket,
            start_receive,
            start_receive_selected,
            send_control,
            cancel_transfer,
            reveal_path,
            open_external,
            app_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running Dropwire");
}
