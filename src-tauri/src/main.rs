// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;
use tracing::{info, warn};

/// Match `--background` in dark theme (hsl(228 22% 6%) ≈ #0c0e14). Used
/// on the WKWebView/WebView2 surface so the first paint frame after the
/// window appears is already dark instead of opaque white.
const DARK_WEBVIEW_BG: tauri::utils::config::Color =
    tauri::utils::config::Color(0x0c, 0x0e, 0x14, 0xff);

fn main() {
    // Initialize tracing for structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("datrina=info".parse().unwrap())
                .add_directive("tauri=info".parse().unwrap()),
        )
        .init();

    info!("Datrina The Lenswright starting...");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_sql::Builder::default().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(datrina_lib::generate_handler!())
        .setup(|app| {
            info!("Setting up Datrina...");

            // Paint the WebView surface and force the native title-bar
            // appearance dark BEFORE the window becomes visible. The
            // window itself is configured with `"visible": false` so we
            // can apply these and then `show()` — without the gating,
            // WKWebView paints one opaque-white frame before the CSS
            // bundle arrives, which is the user-visible "white flash".
            if let Some(window) = app.get_webview_window("main") {
                if let Err(err) = window.set_background_color(Some(DARK_WEBVIEW_BG)) {
                    warn!(
                        "failed to set initial WebView background colour: {err:?} \
                         (white flash before first paint may be visible)"
                    );
                }
                if let Err(err) = window.set_theme(Some(tauri::Theme::Dark)) {
                    warn!("failed to force dark native theme on main window: {err:?}");
                }
                if let Err(err) = window.show() {
                    warn!("failed to show main window after dark-mode prep: {err:?}");
                }
            }

            // Initialize storage (SQLite)
            tauri::async_runtime::block_on(datrina_lib::init_storage(app))?;

            // Initialize MCP manager
            datrina_lib::init_mcp_manager(app)?;

            // Initialize scheduler
            datrina_lib::init_scheduler(app)?;

            info!("Datrina ready");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
