/// Every command in `generate_handler!` (src/lib.rs). Declaring them gives each
/// an `allow-<command>` permission, and with that a command runs only in a
/// window whose capability grants it: capabilities/default.json for the main
/// window, capabilities/tray.json for the tray panel. A new command must be
/// added here and granted there (a unit test in lib.rs checks both).
const COMMANDS: &[&str] = &[
    "my_endpoint_id",
    "my_fingerprint",
    "device_name",
    "get_settings",
    "set_device_name",
    "set_pref",
    "trust_remember",
    "trust_forget",
    "write_text_file",
    "show_main",
    "hide_tray_window",
    "set_tray_state",
    "nearby_start",
    "nearby_stop",
    "nearby_list",
    "nearby_offer",
    "nearby_respond",
    "list_transfers",
    "clear_transfers",
    "pick_paths",
    "pick_dest_dir",
    "default_dest_dir",
    "qr_svg",
    "start_send",
    "inspect_ticket",
    "start_receive",
    "start_receive_selected",
    "send_control",
    "cancel_transfer",
    "reveal_path",
    "open_external",
    "app_version",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run the Tauri build script");
}
