//! Auto-update flow for the desktop app.
//!
//! Uses `tauri-plugin-updater` to check a remote endpoint, download signed
//! bundles, verify their minisign signature against the baked-in public key,
//! and install the new version. The user is prompted via a Tauri dialog;
//! background checks are silent on "no update" and surface errors only in logs.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Summary of an available update, returned to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateSummary {
    pub available: bool,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub release_notes: Option<String>,
    pub pub_date: Option<String>,
}

impl UpdateSummary {
    fn none(current: String) -> Self {
        Self {
            available: false,
            current_version: current,
            latest_version: None,
            release_notes: None,
            pub_date: None,
        }
    }

    fn from_update(current: String, update: &Update) -> Self {
        Self {
            available: true,
            current_version: current,
            latest_version: Some(update.version.clone()),
            release_notes: update.body.clone(),
            pub_date: update.date.map(|d| d.to_string()),
        }
    }
}

/// Look up whether a newer version is available. Does not download or install.
///
/// # Errors
///
/// Returns an error string if the updater plugin cannot be initialized or the
/// remote update check fails (network, signature, or parse errors).
pub async fn fetch_update_summary<R: Runtime>(app: &AppHandle<R>) -> Result<UpdateSummary, String> {
    let current = app.package_info().version.to_string();
    let updater = app.updater().map_err(|e| e.to_string())?;

    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateSummary::from_update(current, &update)),
        Ok(None) => Ok(UpdateSummary::none(current)),
        Err(e) => Err(format!("Update check failed: {e}")),
    }
}

/// Download and install the available update, then restart the app.
///
/// Emits `updater-progress` events to the frontend with `{downloaded, total}`
/// fields so a progress bar can be shown.
///
/// # Errors
///
/// Returns an error string if the updater cannot be initialized, no update is
/// available, or the download/install step fails.
pub async fn download_and_install<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Err("No update available".to_string());
    };

    let app_for_progress = app.clone();
    let mut downloaded: u64 = 0;
    update
        .download_and_install(
            move |chunk_len, content_length| {
                downloaded += chunk_len as u64;
                let payload = serde_json::json!({
                    "downloaded": downloaded,
                    "total": content_length,
                });
                let _ = app_for_progress.emit("updater-progress", payload);
            },
            || {
                // Fires once the download is complete and install begins.
            },
        )
        .await
        .map_err(|e| format!("Update install failed: {e}"))?;

    // Ask the user to restart. If they decline, the new version will load on
    // next launch anyway.
    let app_for_restart = app.clone();
    app.dialog()
        .message("The update has been installed. Restart ModelRelay now?")
        .title("Update ready")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart now".to_string(),
            "Later".to_string(),
        ))
        .show(move |restart| {
            if restart {
                app_for_restart.restart();
            } else {
                let _ = app_for_restart.emit("updater-restart-deferred", ());
            }
        });

    Ok(())
}

/// Run a silent update check shortly after launch. If an update is available,
/// show a non-blocking dialog asking the user whether to install. Errors are
/// logged at `warn` level — a failed update check should never block startup.
pub fn spawn_launch_check<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        // Wait ~5s so startup isn't competing with the update check on slow
        // connections.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        match fetch_update_summary(&app).await {
            Ok(summary) if summary.available => {
                let latest = summary.latest_version.clone().unwrap_or_default();
                let current = summary.current_version.clone();
                let notes = summary
                    .release_notes
                    .clone()
                    .unwrap_or_else(|| "No release notes.".to_string());
                let app_for_prompt = app.clone();

                // Also let the UI know so it can show a badge/banner.
                let _ = app.emit("updater-available", &summary);

                app.dialog()
                    .message(format!(
                        "A new version of ModelRelay is available.\n\nCurrent: {current}\nLatest: {latest}\n\n{notes}"
                    ))
                    .title("Update available")
                    .kind(MessageDialogKind::Info)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "Install now".to_string(),
                        "Later".to_string(),
                    ))
                    .show(move |install| {
                        if install {
                            let app = app_for_prompt.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(e) = download_and_install(&app).await {
                                    tracing::error!(error = %e, "update install failed");
                                    let _ = app.emit("updater-error", e);
                                }
                            });
                        }
                    });
            }
            Ok(_) => {
                tracing::debug!("launch update check: no update available");
            }
            Err(e) => {
                tracing::warn!(error = %e, "launch update check failed");
            }
        }
    });
}
