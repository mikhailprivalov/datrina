// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing::info;

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
        .invoke_handler(datrina_lib::generate_handler!())
        .setup(|app| {
            info!("Setting up Datrina...");

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
