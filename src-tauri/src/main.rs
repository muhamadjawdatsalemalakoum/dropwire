// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Test/dev convenience: `--data-dir=<path>` isolates app state so two
    // instances can run side-by-side (the shell reads DROPWIRE_DATA_DIR).
    for arg in std::env::args().skip(1) {
        if let Some(dir) = arg.strip_prefix("--data-dir=") {
            std::env::set_var("DROPWIRE_DATA_DIR", dir);
            break;
        }
    }
    dropwire_lib::run()
}
