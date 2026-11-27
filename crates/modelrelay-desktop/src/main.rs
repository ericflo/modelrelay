#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use modelrelay_desktop::{AppSettings, AppStatus, WorkerManager, updater};
use modelrelay_worker::WorkerConnectionState;
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, WindowEvent,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

const TRAY_ID: &str = "main-tray";

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn format_tray_tooltip(status: &AppStatus) -> String {
    match status.state {
        WorkerConnectionState::Connected => {
            if status.active_requests > 0 {
                format!("ModelRelay - Connected ({} active)", status.active_requests)
            } else {
                "ModelRelay - Connected".to_string()
            }
        }
        WorkerConnectionState::Connecting => "ModelRelay - Connecting\u{2026}".to_string(),
        WorkerConnectionState::Reconnecting => "ModelRelay - Reconnecting\u{2026}".to_string(),
        WorkerConnectionState::AuthFailed => "ModelRelay - Auth Failed".to_string(),
        WorkerConnectionState::Disconnected => "ModelRelay - Disconnected".to_string(),
    }
}

#[tauri::command]
async fn get_has_saved_settings(manager: tauri::State<'_, WorkerManager>) -> Result<bool, String> {
    let settings = manager.get_settings().await;
    Ok(!settings.worker_secret.is_empty())
}

#[tauri::command]
async fn get_status(manager: tauri::State<'_, WorkerManager>) -> Result<AppStatus, String> {
    Ok(manager.get_status().await)
}

#[tauri::command]
async fn get_settings(manager: tauri::State<'_, WorkerManager>) -> Result<AppSettings, String> {
    Ok(manager.get_settings().await)
}

#[tauri::command]
async fn save_settings(
    manager: tauri::State<'_, WorkerManager>,
    settings: AppSettings,
) -> Result<(), String> {
    let was_running = manager.is_running().await;
    manager.save_settings(settings).await?;
    if was_running {
        manager.start_worker().await?;
    }
    Ok(())
}

#[tauri::command]
async fn start_worker(manager: tauri::State<'_, WorkerManager>) -> Result<(), String> {
    manager.start_worker().await
}

#[tauri::command]
async fn stop_worker(manager: tauri::State<'_, WorkerManager>) -> Result<(), String> {
    manager.stop_worker().await;
    Ok(())
}

#[tauri::command]
async fn refresh_models(manager: tauri::State<'_, WorkerManager>) -> Result<(), String> {
    manager.refresh_models().await
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id("show", "Open Dashboard").build(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
    let check_updates =
        MenuItemBuilder::with_id("check_updates", "Check for Updates\u{2026}").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show)
        .item(&settings)
        .separator()
        .item(&check_updates)
        .separator()
        .item(&quit)
        .build()?;

    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("ModelRelay - Disconnected")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                show_main_window(app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("navigate-tab", "dashboard");
                }
            }
            "settings" => {
                show_main_window(app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("navigate-tab", "settings");
                }
            }
            "check_updates" => {
                show_main_window(app);
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("navigate-tab", "settings");
                }
                let _ = app.emit("updater-manual-check", ());
            }
            "quit" => {
                // Gracefully stop the worker (sending WS Close) before exiting.
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let manager = app_handle.state::<WorkerManager>();
                    manager.stop_worker().await;
                    app_handle.exit(0);
                });
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<updater::UpdateSummary, String> {
    updater::fetch_update_summary(&app).await
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    updater::download_and_install(&app).await
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_has_saved_settings,
            get_status,
            get_settings,
            save_settings,
            start_worker,
            stop_worker,
            refresh_models,
            check_for_update,
            install_update,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let settings_path = app_data_dir.join("settings.json");

            let manager = WorkerManager::new(settings_path);
            let auto_start_worker = manager.initial_auto_start();

            app.manage(manager);
            build_tray(app)?;

            let tray_app_handle = app.handle().clone();
            let setup_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let manager = setup_handle.state::<WorkerManager>();
                manager
                    .set_on_status_change(Arc::new(move |status| {
                        if let Some(tray) = tray_app_handle.tray_by_id(TRAY_ID) {
                            let _ = tray.set_tooltip(Some(format_tray_tooltip(status)));
                        }
                    }))
                    .await;

                if auto_start_worker && let Err(e) = manager.start_worker().await {
                    tracing::error!(error = %e, "auto-start failed");
                }
            });

            show_main_window(app.handle());
            updater::spawn_launch_check(app.handle().clone());

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| handle_run_event(app, &event));
}

#[cfg(target_os = "macos")]
fn handle_run_event(app: &AppHandle, event: &RunEvent) {
    if let RunEvent::Reopen { .. } = event {
        show_main_window(app);
    }
}

#[cfg(not(target_os = "macos"))]
fn handle_run_event(_app: &AppHandle, _event: &RunEvent) {}
