#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use state::AppState;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openscript_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let app_state = AppState::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .manage(app_state)
        .setup(|_app| {
            tracing::info!("OpenScript Tauri app initialized");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::system_capabilities,
            // Timeline
            commands::timeline::split_segment,
            commands::timeline::get_timeline,
            commands::timeline::timeline_preview,
            commands::timeline::undo,
            commands::timeline::redo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
