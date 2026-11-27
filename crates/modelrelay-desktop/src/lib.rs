use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use modelrelay_worker::{
    WorkerConnectionState, WorkerDaemon, WorkerDaemonConfig, WorkerStatusSnapshot,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;
use url::Url;

pub mod updater;

/// Validate that `candidate` parses as an absolute http/https URL with a non-empty host.
///
/// # Errors
/// Returns a human-readable error describing why the URL is invalid.
pub fn validate_http_url(candidate: &str, field: &str) -> Result<(), String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is required"));
    }
    let parsed = Url::parse(trimmed).map_err(|e| format!("{field} is not a valid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("{field} must use http or https (got '{other}')")),
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(format!("{field} is missing a hostname"));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub backend_url: String,
    pub relay_url: String,
    pub worker_secret: String,
    pub provider: String,
    pub worker_name: String,
    pub models: Vec<String>,
    pub max_concurrent: u32,
    pub auto_start: bool,
    pub poll_interval_secs: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            backend_url: "http://localhost:11434".to_string(),
            relay_url: "https://api.modelrelay.io".to_string(),
            worker_secret: String::new(),
            provider: "local".to_string(),
            worker_name: "my-worker".to_string(),
            models: vec!["*".to_string()],
            max_concurrent: 4,
            auto_start: false,
            poll_interval_secs: 30,
        }
    }
}

impl AppSettings {
    /// Normalize whitespace and clamp numeric bounds before persisting or running.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.backend_url = self.backend_url.trim().to_string();
        self.relay_url = self.relay_url.trim().to_string();
        self.worker_secret = self.worker_secret.trim().to_string();
        self.provider = {
            let trimmed = self.provider.trim();
            if trimmed.is_empty() {
                "local".to_string()
            } else {
                trimmed.to_string()
            }
        };
        self.worker_name = {
            let trimmed = self.worker_name.trim();
            if trimmed.is_empty() {
                "my-worker".to_string()
            } else {
                trimmed.to_string()
            }
        };
        self.models = self
            .models
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if self.models.is_empty() {
            self.models.push("*".to_string());
        }
        self.max_concurrent = self.max_concurrent.clamp(1, 64);
        self.poll_interval_secs = self.poll_interval_secs.clamp(1, 3600);
        self
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub connected: bool,
    pub running: bool,
    pub state: WorkerConnectionState,
    pub relay_url: String,
    pub active_requests: u32,
    pub models: Vec<String>,
    pub error: Option<String>,
}

type StatusCallback = Arc<dyn Fn(&AppStatus) + Send + Sync>;

struct ActiveWorkerHandles {
    daemon_handle: JoinHandle<()>,
    watcher_handle: JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
    refresh_notify: Arc<Notify>,
}

/// Manages the worker daemon lifecycle: start, stop, status, and settings persistence.
pub struct WorkerManager {
    lifecycle_lock: Arc<Mutex<()>>,
    active_worker: Arc<Mutex<Option<ActiveWorkerHandles>>>,
    settings: Arc<Mutex<AppSettings>>,
    status: Arc<Mutex<AppStatus>>,
    on_status_change: Arc<Mutex<Option<StatusCallback>>>,
    settings_path: PathBuf,
    initial_auto_start: bool,
}

impl WorkerManager {
    /// Create a new `WorkerManager`, loading persisted settings from `settings_path` if available.
    #[must_use]
    pub fn new(settings_path: PathBuf) -> Self {
        let settings = match std::fs::read_to_string(&settings_path) {
            Ok(json) => serde_json::from_str::<AppSettings>(&json)
                .unwrap_or_default()
                .sanitized(),
            Err(_) => AppSettings::default(),
        };
        let initial_auto_start = settings.auto_start && !settings.worker_secret.is_empty();

        Self {
            lifecycle_lock: Arc::new(Mutex::new(())),
            active_worker: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings)),
            status: Arc::new(Mutex::new(AppStatus::default())),
            on_status_change: Arc::new(Mutex::new(None)),
            settings_path,
            initial_auto_start,
        }
    }

    #[must_use]
    pub const fn initial_auto_start(&self) -> bool {
        self.initial_auto_start
    }

    /// Register a callback invoked whenever [`AppStatus`] changes (used to keep the
    /// tray icon tooltip synchronized with live daemon state).
    pub async fn set_on_status_change(&self, callback: StatusCallback) {
        *self.on_status_change.lock().await = Some(callback);
    }

    /// Atomically persist settings to disk with restrictive permissions on Unix
    /// (`0600`) so `worker_secret` is not world-readable.
    fn persist_settings(&self, settings: &AppSettings) -> Result<(), String> {
        if let Some(parent) = self.settings_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        let tmp_path = self.settings_path.with_extension("json.tmp");

        write_private_file(&tmp_path, json.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp_path, &self.settings_path).map_err(|e| e.to_string())
    }

    pub async fn get_settings(&self) -> AppSettings {
        self.settings.lock().await.clone()
    }

    /// # Errors
    /// Returns an error if URLs fail validation or settings cannot be persisted to disk.
    pub async fn save_settings(&self, new_settings: AppSettings) -> Result<(), String> {
        let sanitized = new_settings.sanitized();
        validate_http_url(&sanitized.backend_url, "Backend URL")?;
        validate_http_url(&sanitized.relay_url, "Relay Server URL")?;
        self.persist_settings(&sanitized)?;
        *self.settings.lock().await = sanitized;
        Ok(())
    }

    pub async fn get_status(&self) -> AppStatus {
        self.status.lock().await.clone()
    }

    pub async fn is_running(&self) -> bool {
        let guard = self.active_worker.lock().await;
        guard
            .as_ref()
            .is_some_and(|w| !w.daemon_handle.is_finished())
    }

    /// Trigger an immediate backend model refresh if the worker is running.
    ///
    /// # Errors
    /// Returns an error if the backend URL is invalid when refreshing while stopped.
    pub async fn refresh_models(&self) -> Result<(), String> {
        let refresh_notify = {
            let guard = self.active_worker.lock().await;
            guard.as_ref().map(|w| Arc::clone(&w.refresh_notify))
        };

        if let Some(notify) = refresh_notify {
            notify.notify_one();
            return Ok(());
        }

        // If stopped, resolve wildcard models directly so the UI can preview models.
        let settings = self.settings.lock().await.clone();
        validate_http_url(&settings.backend_url, "Backend URL")?;
        let mut config = WorkerDaemonConfig {
            proxy_base_url: settings.relay_url.clone(),
            provider: settings.provider.clone(),
            worker_secret: settings.worker_secret.clone(),
            worker_name: settings.worker_name.clone(),
            models: settings.models.clone(),
            max_concurrent: settings.max_concurrent,
            backend_base_url: settings.backend_url.clone(),
            endpoint_prefixes: vec![],
        };
        config.resolve_wildcard_models().await;
        let snapshot = {
            let mut s = self.status.lock().await;
            s.models = config.models;
            s.clone()
        };
        notify_status_callback(&self.on_status_change, &snapshot).await;
        Ok(())
    }

    /// Start the worker daemon in a background tokio task.
    /// Serialized with `stop_worker` so concurrent start/stop calls never leak
    /// duplicate daemons.
    ///
    /// # Errors
    /// Returns an error if settings are invalid or the worker secret is empty.
    #[allow(clippy::too_many_lines)]
    pub async fn start_worker(&self) -> Result<(), String> {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        self.stop_worker_locked().await;

        let settings = self.settings.lock().await.clone().sanitized();
        validate_http_url(&settings.backend_url, "Backend URL")?;
        validate_http_url(&settings.relay_url, "Relay Server URL")?;
        if settings.worker_secret.is_empty() {
            return Err("Worker secret is required".to_string());
        }

        let wildcard_discovery = settings.models.iter().any(|m| m.trim() == "*");
        let mut config = WorkerDaemonConfig {
            proxy_base_url: settings.relay_url.clone(),
            provider: settings.provider.clone(),
            worker_secret: settings.worker_secret.clone(),
            worker_name: settings.worker_name.clone(),
            models: settings.models.clone(),
            max_concurrent: settings.max_concurrent,
            backend_base_url: settings.backend_url.clone(),
            endpoint_prefixes: vec![],
        };
        config.resolve_wildcard_models().await;

        let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot {
            state: WorkerConnectionState::Connecting,
            active_requests: 0,
            models: config.models.clone(),
            last_error: None,
            worker_id: None,
        });
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let refresh_notify = Arc::new(Notify::new());

        let initial_snapshot = {
            let mut s = self.status.lock().await;
            s.connected = false;
            s.running = true;
            s.state = WorkerConnectionState::Connecting;
            s.relay_url.clone_from(&settings.relay_url);
            s.active_requests = 0;
            s.models.clone_from(&config.models);
            s.error = None;
            s.clone()
        };
        notify_status_callback(&self.on_status_change, &initial_snapshot).await;

        // Forward live worker status updates into AppStatus & tray tooltip callback.
        let status_for_watcher = Arc::clone(&self.status);
        let cb_for_watcher = Arc::clone(&self.on_status_change);
        let relay_url = settings.relay_url.clone();
        let watcher_handle = tokio::spawn(async move {
            while status_rx.changed().await.is_ok() {
                let snap = status_rx.borrow().clone();
                let updated = {
                    let mut s = status_for_watcher.lock().await;
                    s.connected = snap.state == WorkerConnectionState::Connected;
                    s.running = snap.state != WorkerConnectionState::Disconnected;
                    s.state = snap.state;
                    s.relay_url.clone_from(&relay_url);
                    s.active_requests = snap.active_requests;
                    if !snap.models.is_empty() {
                        s.models = snap.models;
                    }
                    s.error = snap.last_error;
                    s.clone()
                };
                notify_status_callback(&cb_for_watcher, &updated).await;
            }
        });

        let status_for_exit = Arc::clone(&self.status);
        let cb_for_exit = Arc::clone(&self.on_status_change);
        let poll_interval = (settings.poll_interval_secs > 0)
            .then(|| Duration::from_secs(settings.poll_interval_secs));
        let daemon = WorkerDaemon::new(config)
            .with_wildcard_discovery(wildcard_discovery)
            .with_models_poll_interval(poll_interval)
            .with_status_sender(status_tx)
            .with_shutdown_rx(shutdown_rx)
            .with_models_refresh_notify(Arc::clone(&refresh_notify));

        let daemon_handle = tokio::spawn(async move {
            let result = daemon.run_with_reconnect().await;
            let exited = {
                let mut s = status_for_exit.lock().await;
                s.connected = false;
                s.running = false;
                s.state = WorkerConnectionState::Disconnected;
                s.active_requests = 0;
                if let Err(e) = result {
                    s.error = Some(e.to_string());
                }
                s.clone()
            };
            notify_status_callback(&cb_for_exit, &exited).await;
        });

        *self.active_worker.lock().await = Some(ActiveWorkerHandles {
            daemon_handle,
            watcher_handle,
            shutdown_tx,
            refresh_notify,
        });
        Ok(())
    }

    /// Gracefully stop the running worker daemon (signalling shutdown so it sends
    /// a clean WebSocket Close frame before exiting).
    pub async fn stop_worker(&self) {
        let _lifecycle_guard = self.lifecycle_lock.lock().await;
        self.stop_worker_locked().await;
    }

    async fn stop_worker_locked(&self) {
        let existing = self.active_worker.lock().await.take();
        if let Some(handles) = existing {
            let _ = handles.shutdown_tx.send(true);
            let mut daemon_handle = handles.daemon_handle;
            if tokio::time::timeout(Duration::from_secs(3), &mut daemon_handle)
                .await
                .is_err()
            {
                daemon_handle.abort();
                let _ = daemon_handle.await;
            }
            handles.watcher_handle.abort();
            let _ = handles.watcher_handle.await;
        }

        let stopped = {
            let mut s = self.status.lock().await;
            s.connected = false;
            s.running = false;
            s.state = WorkerConnectionState::Disconnected;
            s.active_requests = 0;
            s.error = None;
            s.clone()
        };
        notify_status_callback(&self.on_status_change, &stopped).await;
    }
}

async fn notify_status_callback(cb_lock: &Arc<Mutex<Option<StatusCallback>>>, status: &AppStatus) {
    let cb = cb_lock.lock().await.clone();
    if let Some(callback) = cb {
        callback(status);
    }
}

#[cfg(unix)]
fn write_private_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ollama_default() {
        validate_http_url("http://localhost:11434", "Backend URL").unwrap();
    }

    #[test]
    fn accepts_lm_studio_default() {
        validate_http_url("http://localhost:1234", "Backend URL").unwrap();
    }

    #[test]
    fn accepts_kiln_default() {
        validate_http_url("http://localhost:8420", "Backend URL").unwrap();
    }

    #[test]
    fn accepts_remote_https_host() {
        validate_http_url("https://api.modelrelay.io", "Relay Server URL").unwrap();
    }

    #[test]
    fn accepts_ip_and_path() {
        validate_http_url("http://192.168.1.42:11434/v1", "Backend URL").unwrap();
    }

    #[test]
    fn rejects_empty() {
        let err = validate_http_url("", "Backend URL").unwrap_err();
        assert!(err.contains("required"), "got: {err}");
    }

    #[test]
    fn rejects_whitespace_only() {
        let err = validate_http_url("   ", "Backend URL").unwrap_err();
        assert!(err.contains("required"), "got: {err}");
    }

    #[test]
    fn rejects_missing_scheme() {
        assert!(validate_http_url("localhost:11434", "Backend URL").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate_http_url("not a url", "Backend URL").is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        let err = validate_http_url("ftp://localhost:11434", "Backend URL").unwrap_err();
        assert!(err.contains("http or https"), "got: {err}");
    }

    #[test]
    fn rejects_missing_host() {
        assert!(validate_http_url("http://:8080/x", "Backend URL").is_err());
    }

    #[test]
    fn default_provider_matches_server_and_worker() {
        assert_eq!(AppSettings::default().provider, "local");
    }

    #[test]
    fn partial_settings_json_preserves_existing_fields() {
        let partial =
            r#"{"worker_secret": "mr_live_abc123", "relay_url": "https://api.modelrelay.io"}"#;
        let parsed: AppSettings = serde_json::from_str(partial).unwrap();
        assert_eq!(parsed.worker_secret, "mr_live_abc123");
        assert_eq!(parsed.provider, "local");
        assert_eq!(parsed.max_concurrent, 4);
    }
}
