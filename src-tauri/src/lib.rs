//! Dropwire desktop shell (Tauri v2).
//!
//! Deliberately thin: it owns the Tauri app, the window, and the command surface,
//! and forwards everything to the verified `irohcore` engine. No iroh-blobs types
//! appear here — only `irohcore`'s stable API.

use std::path::PathBuf;

use irohcore::{Core, CoreConfig, Progress, TransferId, TransferRecord};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
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

/// Start receiving a ticket into `dest` (or the default Downloads/Dropwire folder).
#[tauri::command]
async fn start_receive(
    ticket: String,
    dest: Option<String>,
    on_event: Channel<Progress>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let dest_dir = match dest {
        Some(d) => PathBuf::from(d),
        None => dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Dropwire"),
    };
    let (id, mut stream) = state
        .core
        .receive(ticket, dest_dir)
        .await
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn(async move {
        while let Some(p) = stream.next().await {
            let _ = on_event.send(p);
        }
    });
    Ok(id.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // App data dir: identity (node.key), blob store, transfer catalog.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("dropwire");
            // Build the engine inside Tauri's tokio runtime. Serverless by default:
            // DHT discovery + n0 free relay fallback.
            let core =
                tauri::async_runtime::block_on(Core::start(CoreConfig::serverless(data_dir)))?;
            app.manage(AppState { core });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            my_endpoint_id,
            list_transfers,
            pick_paths,
            pick_dest_dir,
            qr_svg,
            start_send,
            start_receive,
            cancel_transfer,
            reveal_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running Dropwire");
}
