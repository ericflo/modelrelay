use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const DEFAULT_PROTOCOL_VERSION: &str = "katamari-worker-v1";
const MAX_WORKER_NAME_LEN: usize = 32;
const MAX_MODELS_PER_WORKER: usize = 4;
const DEFAULT_CLIENT_IDENTITY: &str = "127.0.0.1";
const FAILED_AUTH_RATE_LIMIT_THRESHOLD: usize = 3;
const FAILED_AUTH_RATE_LIMIT_WINDOW_TICKS: u64 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderConfig {
    pub worker_secret: String,
    pub enabled: bool,
}

impl ProviderConfig {
    #[must_use]
    pub fn enabled(worker_secret: impl Into<String>) -> Self {
        Self {
            worker_secret: worker_secret.into(),
            enabled: true,
        }
    }

    #[must_use]
    pub fn disabled(worker_secret: impl Into<String>) -> Self {
        Self {
            worker_secret: worker_secret.into(),
            enabled: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectRequest {
    pub provider: String,
    pub header_secret: Option<String>,
    pub query_secret: Option<String>,
    pub client_identity: String,
}

impl ConnectRequest {
    #[must_use]
    pub fn with_header_secret(provider: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            header_secret: Some(secret.into()),
            query_secret: None,
            client_identity: DEFAULT_CLIENT_IDENTITY.to_string(),
        }
    }

    #[must_use]
    pub fn with_query_secret(provider: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            header_secret: None,
            query_secret: Some(secret.into()),
            client_identity: DEFAULT_CLIENT_IDENTITY.to_string(),
        }
    }

    #[must_use]
    pub fn with_client_identity(mut self, client_identity: impl Into<String>) -> Self {
        self.client_identity = client_identity.into();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseCode {
    PolicyViolation,
    ProtocolError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeFailure {
    pub code: CloseCode,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct RegistrationHarness {
    providers: HashMap<String, ProviderConfig>,
    next_worker_id: usize,
    current_tick: u64,
    failed_auth_attempts: HashMap<String, FailedAuthRateLimitState>,
}

impl RegistrationHarness {
    #[must_use]
    pub fn new(providers: impl IntoIterator<Item = (impl Into<String>, ProviderConfig)>) -> Self {
        Self {
            providers: providers
                .into_iter()
                .map(|(name, config)| (name.into(), config))
                .collect(),
            next_worker_id: 1,
            current_tick: 0,
            failed_auth_attempts: HashMap::new(),
        }
    }

    pub fn advance_time(&mut self, ticks: u64) {
        self.current_tick += ticks;
        self.expire_failed_auth_rate_limits();
    }

    /// Authenticates an incoming worker connection for a configured provider.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeFailure`] when the provider is unknown, disabled, missing
    /// credentials, or presents the wrong secret.
    pub fn connect(
        &mut self,
        request: ConnectRequest,
    ) -> Result<RegistrationSession, HandshakeFailure> {
        self.expire_failed_auth_rate_limits();

        let client_identity = request.client_identity.clone();
        if self.client_is_rate_limited(&client_identity) {
            return Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: format!(
                    "worker authentication rate limited for client `{client_identity}`"
                ),
            });
        }

        let provider = self
            .providers
            .get(&request.provider)
            .ok_or_else(|| HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: format!("unknown provider `{}`", request.provider),
            })?;

        if !provider.enabled {
            return Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: format!("provider `{}` is disabled", request.provider),
            });
        }

        let presented_secret = request
            .header_secret
            .or(request.query_secret)
            .ok_or_else(|| HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "missing worker secret".to_string(),
            })?;

        if presented_secret != provider.worker_secret {
            self.record_failed_auth(client_identity);
            return Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "worker authentication failed".to_string(),
            });
        }

        self.failed_auth_attempts.remove(&request.client_identity);

        let worker_id = format!("worker-{}", self.next_worker_id);
        self.next_worker_id += 1;

        Ok(RegistrationSession { worker_id })
    }

    fn client_is_rate_limited(&self, client_identity: &str) -> bool {
        self.failed_auth_attempts
            .get(client_identity)
            .and_then(|state| state.limited_until_tick)
            .is_some_and(|limited_until_tick| limited_until_tick > self.current_tick)
    }

    fn record_failed_auth(&mut self, client_identity: String) {
        let state =
            self.failed_auth_attempts
                .entry(client_identity)
                .or_insert(FailedAuthRateLimitState {
                    failed_attempts: 0,
                    limited_until_tick: None,
                    expires_at_tick: self.current_tick + FAILED_AUTH_RATE_LIMIT_WINDOW_TICKS,
                });

        state.failed_attempts += 1;
        state.expires_at_tick = self.current_tick + FAILED_AUTH_RATE_LIMIT_WINDOW_TICKS;
        if state.failed_attempts >= FAILED_AUTH_RATE_LIMIT_THRESHOLD {
            state.limited_until_tick =
                Some(self.current_tick + FAILED_AUTH_RATE_LIMIT_WINDOW_TICKS);
        }
    }

    fn expire_failed_auth_rate_limits(&mut self) {
        self.failed_auth_attempts
            .retain(|_, state| state.expires_at_tick > self.current_tick);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FailedAuthRateLimitState {
    failed_attempts: usize,
    limited_until_tick: Option<u64>,
    expires_at_tick: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationSession {
    worker_id: String,
}

impl RegistrationSession {
    /// Exchanges one worker JSON message for the next server JSON message.
    ///
    /// # Errors
    ///
    /// Returns [`HandshakeFailure`] when the worker payload is invalid JSON or when
    /// registration violates the protocol contract.
    pub fn exchange_text(&self, worker_message: &str) -> Result<String, HandshakeFailure> {
        let message: WorkerToServer =
            serde_json::from_str(worker_message).map_err(|error| HandshakeFailure {
                code: CloseCode::ProtocolError,
                reason: format!("invalid worker message: {error}"),
            })?;

        match message {
            WorkerToServer::Register(register) => {
                let acknowledged = register.sanitized(&self.worker_id)?;
                serde_json::to_string(&ServerToWorker::RegisterAck(acknowledged)).map_err(|error| {
                    HandshakeFailure {
                        code: CloseCode::ProtocolError,
                        reason: format!("failed to encode register_ack: {error}"),
                    }
                })
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerToServer {
    Register(RegisterMessage),
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToWorker {
    RegisterAck(RegisterAck),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterMessage {
    pub worker_name: String,
    pub models: Vec<String>,
    pub max_concurrent: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
}

impl RegisterMessage {
    fn sanitized(self, worker_id: &str) -> Result<RegisterAck, HandshakeFailure> {
        let protocol_version = self
            .protocol_version
            .unwrap_or_else(|| DEFAULT_PROTOCOL_VERSION.to_string());

        if protocol_version != DEFAULT_PROTOCOL_VERSION {
            return Err(HandshakeFailure {
                code: CloseCode::ProtocolError,
                reason: format!(
                    "unsupported protocol version `{protocol_version}`; expected `{DEFAULT_PROTOCOL_VERSION}`"
                ),
            });
        }

        let mut warnings = Vec::new();

        let trimmed_name = self.worker_name.trim();
        let worker_name = if trimmed_name.len() > MAX_WORKER_NAME_LEN {
            warnings.push(format!(
                "worker_name truncated to {MAX_WORKER_NAME_LEN} characters"
            ));
            trimmed_name[..MAX_WORKER_NAME_LEN].to_string()
        } else {
            trimmed_name.to_string()
        };

        let mut models = Vec::new();
        for model in self.models {
            let sanitized = model.trim();
            if sanitized.is_empty() || models.iter().any(|existing| existing == sanitized) {
                continue;
            }
            if models.len() == MAX_MODELS_PER_WORKER {
                warnings.push(format!(
                    "model list truncated to {MAX_MODELS_PER_WORKER} entries"
                ));
                break;
            }
            models.push(sanitized.to_string());
        }

        if models.is_empty() {
            warnings.push("worker registered without any accepted models".to_string());
        }

        Ok(RegisterAck {
            worker_id: worker_id.to_string(),
            worker_name,
            models,
            max_concurrent: self.max_concurrent.max(1),
            protocol_version,
            warnings,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterAck {
    pub worker_id: String,
    pub worker_name: String,
    pub models: Vec<String>,
    pub max_concurrent: u16,
    pub protocol_version: String,
    pub warnings: Vec<String>,
}
