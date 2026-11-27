pub mod api_keys;
pub mod http;
pub mod worker_socket;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use modelrelay_protocol::{
    HeaderMap, MAX_MODEL_METADATA_BYTES, MAX_MODEL_METADATA_ENTRIES, ModelMetadataMap,
    ModelsUpdateMessage, PongMessage, ProviderRouting, RegisterAck, RegisterMessage,
    RequestMessage, ResponseCompleteMessage,
};
use tokio::sync::{Notify, mpsc, oneshot};

#[cfg(feature = "postgres")]
pub use api_keys::PostgresApiKeyStore;
pub use api_keys::{ApiKeyMetadata, ApiKeyStore, InMemoryApiKeyStore, StoreError};
pub use http::ProxyHttpApp;
pub use worker_socket::{WorkerSocketApp, WorkerSocketProviderConfig};

const MAX_REQUEUE_COUNT: usize = 3;
const STREAM_CHANNEL_CAPACITY: usize = 64;
const MAX_SELECTION_CURSORS: usize = 4_096;

#[derive(Debug, Default)]
pub struct ProxyServerCore {
    next_request_id: usize,
    next_worker_id: usize,
    worker_order: Vec<String>,
    workers: HashMap<String, WorkerState>,
    worker_notifiers: HashMap<String, Arc<Notify>>,
    provider_queues: HashMap<String, VecDeque<RequestRecord>>,
    provider_queue_policies: HashMap<String, ProviderQueuePolicy>,
    default_queue_policy: ProviderQueuePolicy,
    selection_cursors: HashMap<SelectionKey, String>,
    active_requests: HashMap<String, ActiveRequestState>,
    pending_worker_requests: HashMap<String, VecDeque<RequestMessage>>,
    pending_http_response_senders: HashMap<String, PendingHttpResponseSender>,
    worker_cancel_signals: Vec<WorkerCancelSignal>,
    worker_graceful_shutdown_signals: Vec<GracefulShutdownSignal>,
    worker_models_refresh_signals: Vec<WorkerModelsRefreshSignal>,
    graceful_shutdown_disconnect_reasons: HashMap<String, GracefulShutdownDisconnectReason>,
}

impl ProxyServerCore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            next_worker_id: 1,
            ..Self::default()
        }
    }

    pub fn configure_provider_queue(
        &mut self,
        provider: impl Into<String>,
        policy: ProviderQueuePolicy,
    ) {
        self.provider_queue_policies.insert(provider.into(), policy);
    }

    /// Set the fallback queue policy used for any provider queue that does not
    /// have an explicit entry in `provider_queue_policies`.
    pub fn set_default_queue_policy(&mut self, policy: ProviderQueuePolicy) {
        self.default_queue_policy = policy;
    }

    /// Attach a [`Notify`] handle for a connected worker socket so the core can
    /// wake the socket task immediately when requests, cancellations, refreshes,
    /// or shutdown signals are queued for it.
    pub fn register_worker_notifier(&mut self, worker_id: &str, notify: Arc<Notify>) {
        if self.workers.contains_key(worker_id) {
            self.worker_notifiers.insert(worker_id.to_string(), notify);
            if self.worker_has_pending_outbound(worker_id) {
                self.notify_worker(worker_id);
            }
        }
    }

    fn worker_has_pending_outbound(&self, worker_id: &str) -> bool {
        self.pending_worker_requests
            .get(worker_id)
            .is_some_and(|q| !q.is_empty())
            || self
                .worker_cancel_signals
                .iter()
                .any(|s| s.worker_id == worker_id)
            || self
                .worker_graceful_shutdown_signals
                .iter()
                .any(|s| s.worker_id == worker_id)
            || self
                .worker_models_refresh_signals
                .iter()
                .any(|s| s.worker_id == worker_id)
            || self
                .graceful_shutdown_disconnect_reasons
                .contains_key(worker_id)
    }

    fn notify_worker(&self, worker_id: &str) {
        if let Some(notify) = self.worker_notifiers.get(worker_id) {
            notify.notify_one();
        }
    }

    fn queue_policy(&self, provider: &str) -> ProviderQueuePolicy {
        self.provider_queue_policies
            .get(provider)
            .copied()
            .unwrap_or(self.default_queue_policy)
    }

    pub fn register_worker(
        &mut self,
        provider: impl Into<String>,
        register: RegisterMessage,
    ) -> RegisteredWorker {
        self.register_worker_for_owner(provider, register, None)
    }

    pub fn register_worker_for_owner(
        &mut self,
        provider: impl Into<String>,
        register: RegisterMessage,
        owner_key_id: Option<String>,
    ) -> RegisteredWorker {
        let provider = provider.into();
        let worker_id = format!("worker-{}", self.next_worker_id);
        self.next_worker_id += 1;

        let sanitized_models = sanitize_models(register.models);
        let sanitized_metadata =
            sanitize_model_metadata(register.model_metadata, &sanitized_models);
        let native_metadata_reported = !sanitized_metadata.is_empty();
        let warnings = registration_warnings(&register.worker_name, &sanitized_models);
        let current_load = register.current_load.unwrap_or(0) as usize;
        let max_concurrent = register.max_concurrent.max(1) as usize;

        let endpoint_prefixes = register.endpoint_prefixes;

        tracing::info!(
            worker_id = %worker_id,
            worker_name = %register.worker_name,
            provider = %provider,
            models = ?sanitized_models,
            metadata_count = sanitized_metadata.len(),
            max_concurrent,
            scoped = owner_key_id.is_some(),
            "worker registered"
        );

        self.worker_order.push(worker_id.clone());
        self.workers.insert(
            worker_id.clone(),
            WorkerState {
                provider,
                worker_name: register.worker_name,
                owner_key_id,
                models: sanitized_models.clone(),
                model_metadata: sanitized_metadata,
                native_metadata_reported,
                legacy_metadata_probed_at: None,
                endpoint_prefixes: endpoint_prefixes.clone(),
                max_concurrent,
                reported_load: current_load,
                in_flight_requests: Vec::new(),
                is_draining: false,
                graceful_shutdown_deadline: None,
            },
        );

        // Immediately drain any compatible queued requests so waiting clients do
        // not sit stranded until a heartbeat or subsequent completion.
        while self.worker_has_capacity(&worker_id) {
            if self.dispatch_next_compatible(&worker_id).is_none() {
                break;
            }
        }

        RegisteredWorker {
            worker_id: worker_id.clone(),
            ack: RegisterAck {
                worker_id,
                models: sanitized_models,
                warnings,
                protocol_version: register.protocol_version,
                endpoint_prefixes,
            },
        }
    }

    pub fn update_worker_models(
        &mut self,
        worker_id: &str,
        update: ModelsUpdateMessage,
    ) -> Vec<DispatchAssignment> {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            return Vec::new();
        };

        worker.models = sanitize_models(update.models);
        if !update.model_metadata.is_empty() {
            worker.model_metadata = sanitize_model_metadata(update.model_metadata, &worker.models);
            worker.native_metadata_reported = true;
        } else if worker.native_metadata_reported {
            worker.model_metadata.clear();
        } else {
            // Keep cached legacy probe entries only for models still advertised,
            // and allow a fresh legacy probe on the next `/v1/models` call.
            let allowed: HashSet<&str> = worker.models.iter().map(String::as_str).collect();
            if !allowed.contains("*") {
                worker
                    .model_metadata
                    .retain(|id, _| allowed.contains(id.as_str()));
            }
            worker.legacy_metadata_probed_at = None;
        }
        worker.reported_load = update.current_load as usize;

        tracing::debug!(
            worker_id = %worker_id,
            models = ?worker.models,
            reported_load = worker.reported_load,
            "worker models updated"
        );

        let mut assignments = Vec::new();
        while self.worker_has_capacity(worker_id) {
            let Some(assignment) = self.dispatch_next_compatible(worker_id) else {
                break;
            };
            assignments.push(assignment);
        }

        assignments
    }

    pub fn record_worker_pong(
        &mut self,
        worker_id: &str,
        pong: &PongMessage,
    ) -> Vec<DispatchAssignment> {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            return Vec::new();
        };

        worker.reported_load = pong.current_load as usize;

        let mut assignments = Vec::new();
        while self.worker_has_capacity(worker_id) {
            let Some(assignment) = self.dispatch_next_compatible(worker_id) else {
                break;
            };
            assignments.push(assignment);
        }

        assignments
    }

    pub fn submit_request(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> SubmissionOutcome {
        self.submit_transport_request(
            provider,
            model,
            "/".to_string(),
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_transport_request(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        is_streaming: bool,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
    ) -> SubmissionOutcome {
        self.submit_transport_request_for_owner(
            provider,
            model,
            endpoint_path,
            method,
            is_streaming,
            body,
            headers,
            provider_routing,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_transport_request_for_owner(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        is_streaming: bool,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
        owner_key_id: Option<String>,
    ) -> SubmissionOutcome {
        let request = RequestRecord {
            request_id: format!("request-{}", self.next_request_id),
            provider: provider.into(),
            model: model.into(),
            owner_key_id,
            queued_at: Instant::now(),
            transport: RequestTransport {
                endpoint_path: endpoint_path.into(),
                method: method.into(),
                is_streaming,
                body: body.into(),
                headers,
            },
            provider_routing,
            requeue_count: 0,
        };
        self.next_request_id += 1;
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::Queued {
                request: request.clone(),
            },
        );

        if let Some(worker_id) = self.find_eligible_worker_id(&request) {
            tracing::debug!(
                request_id = %request.request_id,
                worker_id = %worker_id,
                model = %request.model,
                endpoint = %request.transport.endpoint_path,
                streaming = request.transport.is_streaming,
                "request dispatched"
            );
            self.assign_to_worker(&worker_id, request.clone());
            return SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: request.request_id,
                worker_id,
            });
        }

        let policy = self.queue_policy(&request.provider);
        let current_queue_len = self
            .provider_queues
            .get(&request.provider)
            .map_or(0, VecDeque::len);

        if current_queue_len >= policy.max_queue_len {
            self.active_requests.remove(&request.request_id);
            let reason = if self.has_compatible_worker(&request) {
                RequestFailureReason::QueueFull
            } else {
                RequestFailureReason::NoWorkersAvailable
            };
            tracing::warn!(
                request_id = %request.request_id,
                provider = %request.provider,
                model = %request.model,
                ?reason,
                "request rejected"
            );
            return SubmissionOutcome::Rejected(RequestFailure {
                request_id: request.request_id,
                reason,
            });
        }

        let queue = self
            .provider_queues
            .entry(request.provider.clone())
            .or_default();
        queue.push_back(request.clone());
        let queue_len = queue.len();
        tracing::debug!(
            request_id = %request.request_id,
            provider = %request.provider,
            model = %request.model,
            queue_len,
            "request queued"
        );
        SubmissionOutcome::Queued(QueuedAssignment {
            request_id: request.request_id,
            queue_len,
        })
    }

    pub fn finish_request(
        &mut self,
        worker_id: &str,
        request_id: &str,
    ) -> Option<DispatchAssignment> {
        let should_disconnect_after_finish = {
            let worker = self.workers.get_mut(worker_id)?;
            let position = worker
                .in_flight_requests
                .iter()
                .position(|active_request_id| active_request_id == request_id)?;
            worker.in_flight_requests.remove(position);
            worker.reported_load = worker.reported_load.saturating_sub(1);
            worker.is_draining && worker.in_flight_requests.is_empty()
        };
        self.active_requests.remove(request_id);
        self.pending_http_response_senders.remove(request_id);

        if should_disconnect_after_finish {
            tracing::info!(worker_id = %worker_id, "draining worker finished last request");
            self.graceful_shutdown_disconnect_reasons.insert(
                worker_id.to_string(),
                GracefulShutdownDisconnectReason::Completed,
            );
            self.notify_worker(worker_id);
            self.remove_worker(worker_id);
            return None;
        }

        self.dispatch_next_compatible(worker_id)
    }

    /// Submits an HTTP-backed request and returns a future-like handle for the worker response.
    ///
    /// # Errors
    ///
    /// Returns [`RequestFailure`] when the underlying transport submission is rejected
    /// immediately, which currently happens if the provider queue is already full.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_http_response_request(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
    ) -> Result<PendingHttpResponse, RequestFailure> {
        self.submit_http_response_request_for_owner(
            provider,
            model,
            endpoint_path,
            method,
            body,
            headers,
            provider_routing,
            None,
        )
    }

    /// Submits an HTTP-backed request scoped to an optional API key owner.
    ///
    /// # Errors
    ///
    /// Returns [`RequestFailure`] when the underlying transport submission is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_http_response_request_for_owner(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
        owner_key_id: Option<String>,
    ) -> Result<PendingHttpResponse, RequestFailure> {
        let (response_tx, response_rx) = oneshot::channel();
        let outcome = self.submit_transport_request_for_owner(
            provider,
            model,
            endpoint_path,
            method,
            false,
            body,
            headers,
            provider_routing,
            owner_key_id,
        );

        let request_id = match outcome {
            SubmissionOutcome::Dispatched(DispatchAssignment { request_id, .. })
            | SubmissionOutcome::Queued(QueuedAssignment { request_id, .. }) => request_id,
            SubmissionOutcome::Rejected(failure) => return Err(failure),
        };

        self.pending_http_response_senders.insert(
            request_id.clone(),
            PendingHttpResponseSender::Unary(response_tx),
        );

        Ok(PendingHttpResponse {
            request_id,
            response_rx,
        })
    }

    /// Submits an HTTP-backed streaming request and returns a receiver for live body events.
    ///
    /// # Errors
    ///
    /// Returns [`RequestFailure`] when the underlying transport submission is rejected
    /// immediately, which currently happens if the provider queue is already full.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_http_streaming_request(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
    ) -> Result<PendingStreamingHttpResponse, RequestFailure> {
        self.submit_http_streaming_request_for_owner(
            provider,
            model,
            endpoint_path,
            method,
            body,
            headers,
            provider_routing,
            None,
        )
    }

    /// Submits an HTTP-backed streaming request scoped to an optional API key owner.
    ///
    /// # Errors
    ///
    /// Returns [`RequestFailure`] when the underlying transport submission is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_http_streaming_request_for_owner(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
        endpoint_path: impl Into<String>,
        method: impl Into<String>,
        body: impl Into<String>,
        headers: HeaderMap,
        provider_routing: Option<ProviderRouting>,
        owner_key_id: Option<String>,
    ) -> Result<PendingStreamingHttpResponse, RequestFailure> {
        let (event_tx, event_rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
        let outcome = self.submit_transport_request_for_owner(
            provider,
            model,
            endpoint_path,
            method,
            true,
            body,
            headers,
            provider_routing,
            owner_key_id,
        );

        let request_id = match outcome {
            SubmissionOutcome::Dispatched(DispatchAssignment { request_id, .. })
            | SubmissionOutcome::Queued(QueuedAssignment { request_id, .. }) => request_id,
            SubmissionOutcome::Rejected(failure) => return Err(failure),
        };

        self.pending_http_response_senders.insert(
            request_id.clone(),
            PendingHttpResponseSender::Streaming(event_tx),
        );

        Ok(PendingStreamingHttpResponse {
            request_id,
            event_rx,
        })
    }

    /// Prepares a bounded streaming sender for an incoming chunk from `worker_id`.
    ///
    /// Returns `Some(Some(sender))` when the chunk should be forwarded to the HTTP
    /// client, `Some(None)` when the chunk is a benign late/post-cancellation frame
    /// that should be dropped without disconnecting the worker, and `None` only
    /// when `worker_id` is no longer registered.
    pub fn prepare_stream_chunk_sender(
        &mut self,
        worker_id: &str,
        request_id: &str,
    ) -> Option<Option<mpsc::Sender<HttpResponseEvent>>> {
        if !self.workers.contains_key(worker_id) {
            return None;
        }

        let Some(ActiveRequestState::InFlight {
            worker_id: active_worker_id,
            cancellation,
            has_emitted_chunk,
            ..
        }) = self.active_requests.get_mut(request_id)
        else {
            tracing::debug!(
                worker_id = %worker_id,
                request_id = %request_id,
                "dropping late response chunk for inactive request"
            );
            return Some(None);
        };

        if active_worker_id != worker_id {
            tracing::debug!(
                worker_id = %worker_id,
                active_worker_id = %active_worker_id,
                request_id = %request_id,
                "dropping chunk from non-owner worker"
            );
            return Some(None);
        }

        if cancellation.is_some() {
            tracing::debug!(
                worker_id = %worker_id,
                request_id = %request_id,
                "dropping chunk for cancelled request"
            );
            return Some(None);
        }

        let Some(PendingHttpResponseSender::Streaming(sender)) =
            self.pending_http_response_senders.get(request_id)
        else {
            return Some(None);
        };

        *has_emitted_chunk = true;
        Some(Some(sender.clone()))
    }

    pub fn stream_http_response_chunk(
        &mut self,
        worker_id: &str,
        request_id: &str,
        chunk: String,
    ) -> bool {
        match self.prepare_stream_chunk_sender(worker_id, request_id) {
            Some(Some(sender)) => {
                let _ = sender.try_send(HttpResponseEvent::Chunk(chunk));
                true
            }
            Some(None) => true,
            None => false,
        }
    }

    pub fn complete_http_response(
        &mut self,
        worker_id: &str,
        response: ResponseCompleteMessage,
    ) -> bool {
        if !self.workers.contains_key(worker_id) {
            return false;
        }

        let request_id = response.request_id.clone();

        // Verify ownership BEFORE removing pending_http_response_senders so a
        // stray completion from a non-owner worker cannot steal an active client.
        let owned_by_worker = matches!(
            self.active_requests.get(&request_id),
            Some(ActiveRequestState::InFlight {
                worker_id: active_worker_id,
                ..
            }) if active_worker_id == worker_id
        );

        if !owned_by_worker {
            tracing::debug!(
                worker_id = %worker_id,
                request_id = %request_id,
                "ignoring response_complete for unknown or non-owned request"
            );
            return true;
        }

        let sender = self.pending_http_response_senders.remove(&request_id);
        let _ = self.finish_request(worker_id, &request_id);

        if let Some(sender) = sender {
            match sender {
                PendingHttpResponseSender::Unary(sender) => {
                    let _ = sender.send(Ok(response));
                }
                PendingHttpResponseSender::Streaming(sender) => {
                    let _ = sender.try_send(HttpResponseEvent::Complete(response));
                }
            }
        }
        true
    }

    /// Handle a worker-reported [`modelrelay_protocol::ErrorMessage`] without
    /// tearing down the worker's WebSocket connection.
    pub fn handle_worker_error(
        &mut self,
        worker_id: &str,
        request_id: Option<&str>,
        code: &str,
        message: &str,
    ) -> bool {
        if !self.workers.contains_key(worker_id) {
            return false;
        }

        let Some(request_id) = request_id else {
            tracing::warn!(
                worker_id = %worker_id,
                code = %code,
                message = %message,
                "worker reported general error"
            );
            return true;
        };

        let owned_by_worker = matches!(
            self.active_requests.get(request_id),
            Some(ActiveRequestState::InFlight {
                worker_id: active_worker_id,
                ..
            }) if active_worker_id == worker_id
        );

        if !owned_by_worker {
            tracing::debug!(
                worker_id = %worker_id,
                request_id = %request_id,
                code = %code,
                "ignoring worker error for inactive request"
            );
            return true;
        }

        tracing::warn!(
            worker_id = %worker_id,
            request_id = %request_id,
            code = %code,
            message = %message,
            "worker reported request error"
        );

        let sender = self.pending_http_response_senders.remove(request_id);
        let _ = self.finish_request(worker_id, request_id);

        if let Some(sender) = sender {
            let error_body = serde_json::json!({
                "error": {
                    "type": code,
                    "message": message,
                }
            })
            .to_string();
            let mut headers = HeaderMap::new();
            headers.insert("content-type".to_string(), "application/json".to_string());
            let complete = ResponseCompleteMessage {
                request_id: request_id.to_string(),
                status_code: 502,
                headers,
                body: Some(error_body),
                token_counts: None,
            };
            match sender {
                PendingHttpResponseSender::Unary(tx) => {
                    let _ = tx.send(Ok(complete));
                }
                PendingHttpResponseSender::Streaming(tx) => {
                    let _ = tx.try_send(HttpResponseEvent::WorkerError(message.to_string()));
                }
            }
        }

        true
    }

    #[must_use]
    pub fn take_pending_worker_requests(&mut self, worker_id: &str) -> Vec<RequestMessage> {
        self.pending_worker_requests
            .remove(worker_id)
            .map(VecDeque::into_iter)
            .into_iter()
            .flatten()
            .collect()
    }

    #[must_use]
    pub fn take_pending_worker_cancel_signals(
        &mut self,
        worker_id: &str,
    ) -> Vec<WorkerCancelSignal> {
        let mut pending = Vec::new();
        let mut remaining = Vec::with_capacity(self.worker_cancel_signals.len());

        for signal in self.worker_cancel_signals.drain(..) {
            let is_still_pending = matches!(
                self.active_requests.get(&signal.request_id),
                Some(ActiveRequestState::InFlight {
                    worker_id: active_worker_id,
                    cancellation: Some(_),
                    ..
                }) if active_worker_id == &signal.worker_id
            );

            if signal.worker_id == worker_id && is_still_pending {
                pending.push(signal);
            } else if is_still_pending {
                remaining.push(signal);
            }
        }

        self.worker_cancel_signals = remaining;
        pending
    }

    pub fn begin_graceful_shutdown(
        &mut self,
        reason: Option<&str>,
        drain_timeout: Duration,
    ) -> Vec<GracefulShutdownSignal> {
        let disconnect_deadline = Instant::now() + drain_timeout;
        let mut signals = Vec::new();

        for worker_id in self.worker_order.clone() {
            let Some(worker) = self.workers.get_mut(&worker_id) else {
                continue;
            };
            if worker.is_draining {
                continue;
            }

            worker.is_draining = true;
            worker.graceful_shutdown_deadline = Some(disconnect_deadline);

            let signal = GracefulShutdownSignal {
                worker_id: worker_id.clone(),
                reason: reason.map(ToOwned::to_owned),
                drain_timeout,
            };
            self.worker_graceful_shutdown_signals.push(signal.clone());
            self.notify_worker(&worker_id);
            signals.push(signal);
        }

        tracing::info!(
            draining_workers = signals.len(),
            drain_timeout_secs = drain_timeout.as_secs(),
            "initiated graceful worker shutdown"
        );

        signals
    }

    #[must_use]
    pub fn take_pending_worker_graceful_shutdown_signals(
        &mut self,
        worker_id: &str,
    ) -> Vec<GracefulShutdownSignal> {
        let mut pending = Vec::new();
        let mut remaining = Vec::with_capacity(self.worker_graceful_shutdown_signals.len());

        for signal in self.worker_graceful_shutdown_signals.drain(..) {
            if signal.worker_id == worker_id && self.workers.contains_key(&signal.worker_id) {
                pending.push(signal);
            } else if self.workers.contains_key(&signal.worker_id) {
                remaining.push(signal);
            }
        }

        self.worker_graceful_shutdown_signals = remaining;
        pending
    }

    pub fn request_worker_models_refresh(
        &mut self,
        worker_id: &str,
        reason: Option<&str>,
    ) -> Option<WorkerModelsRefreshSignal> {
        if !self.workers.contains_key(worker_id) {
            return None;
        }

        let signal = WorkerModelsRefreshSignal {
            worker_id: worker_id.to_string(),
            reason: reason.map(ToOwned::to_owned),
        };
        self.worker_models_refresh_signals.push(signal.clone());
        self.notify_worker(worker_id);
        Some(signal)
    }

    #[must_use]
    pub fn take_pending_worker_models_refresh_signals(
        &mut self,
        worker_id: &str,
    ) -> Vec<WorkerModelsRefreshSignal> {
        let mut pending = Vec::new();
        let mut remaining = Vec::with_capacity(self.worker_models_refresh_signals.len());

        for signal in self.worker_models_refresh_signals.drain(..) {
            if signal.worker_id == worker_id && self.workers.contains_key(&signal.worker_id) {
                pending.push(signal);
            } else if self.workers.contains_key(&signal.worker_id) {
                remaining.push(signal);
            }
        }

        self.worker_models_refresh_signals = remaining;
        pending
    }

    #[must_use]
    pub fn expire_graceful_shutdown(&mut self, now: Instant) -> GracefulShutdownOutcome {
        let expiring_workers = self
            .worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                let deadline = worker.graceful_shutdown_deadline?;
                (deadline <= now).then(|| worker_id.clone())
            })
            .collect::<Vec<_>>();

        let mut disconnected_worker_ids = Vec::new();
        let mut failed_requests = Vec::new();

        for worker_id in expiring_workers {
            let Some(worker) = self.workers.get(&worker_id).cloned() else {
                continue;
            };

            for request_id in &worker.in_flight_requests {
                self.active_requests.remove(request_id);
                self.fail_pending_http_response(
                    request_id,
                    RequestFailureReason::GracefulShutdownTimedOut,
                );
                failed_requests.push(RequestFailure {
                    request_id: request_id.clone(),
                    reason: RequestFailureReason::GracefulShutdownTimedOut,
                });
            }

            self.graceful_shutdown_disconnect_reasons.insert(
                worker_id.clone(),
                GracefulShutdownDisconnectReason::TimedOut,
            );
            self.notify_worker(&worker_id);
            self.remove_worker(&worker_id);
            disconnected_worker_ids.push(worker_id);
        }

        GracefulShutdownOutcome {
            disconnected_worker_ids,
            failed_requests,
        }
    }

    /// Reap in-flight cancelled requests whose worker failed to acknowledge the
    /// cancellation within `grace`. Prevents legacy or misbehaving workers from
    /// leaking concurrency slots permanently.
    #[must_use]
    pub fn expire_cancelled_requests(&mut self, now: Instant, grace: Duration) -> Vec<String> {
        let expired: Vec<(String, String)> = self
            .active_requests
            .iter()
            .filter_map(|(request_id, state)| match state {
                ActiveRequestState::InFlight {
                    worker_id,
                    cancelled_at: Some(cancelled_at),
                    ..
                } if now.saturating_duration_since(*cancelled_at) >= grace => {
                    Some((worker_id.clone(), request_id.clone()))
                }
                _ => None,
            })
            .collect();

        let mut reaped = Vec::with_capacity(expired.len());
        for (worker_id, request_id) in expired {
            tracing::warn!(
                worker_id = %worker_id,
                request_id = %request_id,
                "reaping unacknowledged cancelled request to free worker slot"
            );
            self.fail_pending_http_response(
                &request_id,
                RequestFailureReason::RequestAlreadyCanceled,
            );
            let _ = self.finish_request(&worker_id, &request_id);
            reaped.push(request_id);
        }
        reaped
    }

    #[must_use]
    pub fn expire_queue_timeouts(&mut self, now: Instant) -> Vec<RequestFailure> {
        let mut failures = Vec::new();
        let mut expired_request_ids = Vec::new();
        let default_policy = self.default_queue_policy;

        for (provider, queue) in &mut self.provider_queues {
            let Some(timeout) = self
                .provider_queue_policies
                .get(provider)
                .copied()
                .unwrap_or(default_policy)
                .queue_timeout_ticks
                .map(Duration::from_secs)
            else {
                continue;
            };

            let mut retained_queue = VecDeque::with_capacity(queue.len());

            while let Some(request) = queue.pop_front() {
                if now.saturating_duration_since(request.queued_at) >= timeout {
                    self.active_requests.remove(&request.request_id);
                    expired_request_ids.push(request.request_id.clone());
                    failures.push(RequestFailure {
                        request_id: request.request_id,
                        reason: RequestFailureReason::QueueTimedOut,
                    });
                } else {
                    retained_queue.push_back(request);
                }
            }

            *queue = retained_queue;
        }

        // Prune empty queues to avoid unbounded HashMap growth over time.
        self.provider_queues.retain(|_, queue| !queue.is_empty());

        for request_id in expired_request_ids {
            self.fail_pending_http_response(&request_id, RequestFailureReason::QueueTimedOut);
        }

        failures
    }

    pub fn delete_provider(&mut self, provider: &str) {
        let queued_request_ids = self
            .provider_queues
            .remove(provider)
            .into_iter()
            .flat_map(std::iter::IntoIterator::into_iter)
            .map(|request| request.request_id)
            .collect::<Vec<_>>();

        for request_id in queued_request_ids {
            self.active_requests.remove(&request_id);
            self.fail_pending_http_response(&request_id, RequestFailureReason::ProviderDeleted);
        }

        let worker_ids = self
            .worker_order
            .iter()
            .filter_map(|worker_id| {
                self.workers
                    .get(worker_id)
                    .filter(|worker| worker.provider == provider)
                    .map(|_| worker_id.clone())
            })
            .collect::<Vec<_>>();

        for worker_id in worker_ids {
            let Some(worker) = self.workers.get(&worker_id).cloned() else {
                continue;
            };

            for request_id in worker.in_flight_requests {
                self.active_requests.remove(&request_id);
                self.fail_pending_http_response(&request_id, RequestFailureReason::ProviderDeleted);
            }

            self.graceful_shutdown_disconnect_reasons.insert(
                worker_id.clone(),
                GracefulShutdownDisconnectReason::Completed,
            );
            self.notify_worker(&worker_id);
            let _ = self.remove_worker(&worker_id);
        }
    }

    pub fn disconnect_worker(&mut self, worker_id: &str) -> Option<WorkerDisconnectOutcome> {
        let worker = self.remove_worker(worker_id)?;

        tracing::info!(
            worker_id = %worker_id,
            worker_name = %worker.worker_name,
            provider = %worker.provider,
            in_flight = worker.in_flight_requests.len(),
            "worker disconnected"
        );

        let mut requeued_request_ids = Vec::new();
        let mut failed_requests = Vec::new();
        let mut requeued_requests = Vec::new();

        for request_id in worker.in_flight_requests {
            let Some(active_request) = self.active_requests.remove(&request_id) else {
                continue;
            };

            let ActiveRequestState::InFlight {
                mut request,
                cancellation,
                has_emitted_chunk,
                ..
            } = active_request
            else {
                continue;
            };

            if cancellation.is_some() {
                self.fail_pending_http_response(
                    &request_id,
                    RequestFailureReason::RequestAlreadyCanceled,
                );
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::RequestAlreadyCanceled,
                });
                continue;
            }

            // Never requeue a streaming request after chunks have already been
            // flushed to the HTTP client — re-running it from scratch would glue
            // two unrelated completions onto the same HTTP 200 SSE stream.
            if has_emitted_chunk {
                tracing::warn!(
                    request_id = %request_id,
                    worker_id = %worker_id,
                    "failing streaming request after mid-stream worker disconnect"
                );
                self.fail_pending_http_response(
                    &request_id,
                    RequestFailureReason::WorkerDisconnectedMidStream,
                );
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::WorkerDisconnectedMidStream,
                });
                continue;
            }

            if request.requeue_count >= MAX_REQUEUE_COUNT {
                tracing::warn!(
                    request_id = %request_id,
                    worker_id = %worker_id,
                    "request exhausted max requeue attempts"
                );
                self.fail_pending_http_response(
                    &request_id,
                    RequestFailureReason::MaxRequeuesExceeded,
                );
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::MaxRequeuesExceeded,
                });
                continue;
            }

            request.requeue_count += 1;
            tracing::info!(
                request_id = %request.request_id,
                requeue_count = request.requeue_count,
                provider = %request.provider,
                "requeueing in-flight request after worker disconnect"
            );
            requeued_request_ids.push(request.request_id.clone());
            requeued_requests.push(request.clone());
            self.active_requests.insert(
                request.request_id.clone(),
                ActiveRequestState::Queued { request },
            );
        }

        // Re-insert each request under its ORIGINAL `request.provider` (not
        // `worker.provider`) so queue timeouts and cancellations stay coherent
        // even when cross-provider fallback dispatched the request.
        for request in requeued_requests.into_iter().rev() {
            self.provider_queues
                .entry(request.provider.clone())
                .or_default()
                .push_front(request);
        }

        // Immediately dispatch requeued work to any surviving idle workers.
        if !requeued_request_ids.is_empty() {
            self.dispatch_all_available();
        }

        Some(WorkerDisconnectOutcome {
            requeued_request_ids,
            failed_requests,
        })
    }

    fn dispatch_all_available(&mut self) {
        let candidate_ids = self.worker_order.clone();
        for worker_id in candidate_ids {
            while self.worker_has_capacity(&worker_id) {
                if self.dispatch_next_compatible(&worker_id).is_none() {
                    break;
                }
            }
        }
    }

    pub fn cancel_request(
        &mut self,
        request_id: &str,
        reason: CancelReason,
    ) -> Option<CancellationOutcome> {
        match self.active_requests.get(request_id)?.clone() {
            ActiveRequestState::Queued { request } => {
                if let Some(queue) = self.provider_queues.get_mut(&request.provider)
                    && let Some(index) = queue
                        .iter()
                        .position(|queued_request| queued_request.request_id == request_id)
                {
                    queue.remove(index);
                } else {
                    // Defensive fallback: scan all queues in case of external mutation.
                    for queue in self.provider_queues.values_mut() {
                        if let Some(index) = queue
                            .iter()
                            .position(|queued_request| queued_request.request_id == request_id)
                        {
                            queue.remove(index);
                            break;
                        }
                    }
                }
                self.active_requests.remove(request_id);
                self.pending_http_response_senders.remove(request_id);
                tracing::debug!(request_id = %request_id, ?reason, "removed queued request on cancel");

                Some(CancellationOutcome::RemovedFromQueue {
                    request_id: request_id.to_string(),
                })
            }
            ActiveRequestState::InFlight {
                worker_id,
                cancellation,
                ..
            } => {
                let effective_reason = cancellation.unwrap_or(reason);
                if cancellation.is_none() {
                    if let Some(ActiveRequestState::InFlight {
                        cancellation,
                        cancelled_at,
                        ..
                    }) = self.active_requests.get_mut(request_id)
                    {
                        *cancellation = Some(reason);
                        *cancelled_at = Some(Instant::now());
                    }
                    if reason == CancelReason::ClientDisconnected {
                        self.pending_http_response_senders.remove(request_id);
                    }
                    self.worker_cancel_signals.push(WorkerCancelSignal {
                        worker_id: worker_id.clone(),
                        request_id: request_id.to_string(),
                        reason,
                    });
                    self.notify_worker(&worker_id);
                    tracing::debug!(
                        request_id = %request_id,
                        worker_id = %worker_id,
                        ?reason,
                        "queued worker cancel signal"
                    );
                }

                Some(CancellationOutcome::WorkerCancelSent(WorkerCancelSignal {
                    worker_id,
                    request_id: request_id.to_string(),
                    reason: effective_reason,
                }))
            }
        }
    }

    #[must_use]
    pub fn connected_worker_count(&self) -> usize {
        self.workers.len()
    }

    #[must_use]
    pub fn total_queue_depth(&self) -> usize {
        self.provider_queues.values().map(VecDeque::len).sum()
    }

    #[must_use]
    pub fn admin_workers_snapshot(&self) -> Vec<AdminWorkerInfo> {
        self.worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                Some(AdminWorkerInfo {
                    worker_id: worker_id.clone(),
                    provider: worker.provider.clone(),
                    worker_name: worker.worker_name.clone(),
                    models: worker.models.clone(),
                    endpoint_prefixes: worker.endpoint_prefixes.clone(),
                    max_concurrent: worker.max_concurrent,
                    reported_load: worker.reported_load,
                    in_flight_count: worker.in_flight_requests.len(),
                    is_draining: worker.is_draining,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn admin_queue_depth(&self) -> HashMap<String, usize> {
        self.provider_queues
            .iter()
            .map(|(provider, queue)| (provider.clone(), queue.len()))
            .collect()
    }

    #[must_use]
    pub fn worker_debug_info(&self) -> Vec<(String, String, Vec<String>, usize, usize, bool)> {
        self.worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                Some((
                    worker_id.clone(),
                    worker.provider.clone(),
                    worker.models.clone(),
                    worker.in_flight_requests.len(),
                    worker.max_concurrent,
                    worker.is_draining,
                ))
            })
            .collect()
    }

    #[must_use]
    pub fn provider_models(&self, provider: &str) -> Vec<String> {
        self.provider_models_for_owner(provider, None)
    }

    #[must_use]
    pub fn provider_models_for_owner(
        &self,
        provider: &str,
        owner_key_id: Option<&str>,
    ) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut models = Vec::new();

        // Exact provider workers first (for stable ordering), then union all
        // other routable providers visible to this tenant so mixed provider
        // names (e.g. "local" + "default") never hide models from /v1/models.
        for exact_only in [true, false] {
            for worker_id in &self.worker_order {
                let Some(worker) = self.workers.get(worker_id) else {
                    continue;
                };
                if !worker.visible_to_owner(owner_key_id) {
                    continue;
                }
                if exact_only && worker.provider != provider {
                    continue;
                }
                if !exact_only && worker.provider == provider {
                    continue;
                }
                for model in &worker.models {
                    if model != "*" && seen.insert(model.clone()) {
                        models.push(model.clone());
                    }
                }
            }
        }

        models
    }

    /// Returns full OpenAI-compatible model objects (preserving backend `/v1/models`
    /// metadata such as context length, max output tokens, reasoning efforts, and
    /// modalities) across all workers visible to `owner_key_id`.
    #[must_use]
    pub fn provider_model_entries_for_owner(
        &self,
        provider: &str,
        owner_key_id: Option<&str>,
    ) -> Vec<serde_json::Map<String, serde_json::Value>> {
        let mut model_order = Vec::new();
        let mut model_metadata: HashMap<
            String,
            Option<serde_json::Map<String, serde_json::Value>>,
        > = HashMap::new();

        for exact_only in [true, false] {
            for worker_id in &self.worker_order {
                let Some(worker) = self.workers.get(worker_id) else {
                    continue;
                };
                if !worker.visible_to_owner(owner_key_id) {
                    continue;
                }
                if exact_only && worker.provider != provider {
                    continue;
                }
                if !exact_only && worker.provider == provider {
                    continue;
                }
                for model in &worker.models {
                    if model == "*" {
                        continue;
                    }
                    let meta = worker.model_metadata.get(model).cloned();
                    match model_metadata.entry(model.clone()) {
                        std::collections::hash_map::Entry::Vacant(slot) => {
                            model_order.push(model.clone());
                            slot.insert(meta);
                        }
                        std::collections::hash_map::Entry::Occupied(mut slot) => {
                            if slot.get().is_none() && meta.is_some() {
                                slot.insert(meta);
                            }
                        }
                    }
                }
            }
        }

        model_order
            .into_iter()
            .map(|id| {
                let mut obj = model_metadata.remove(&id).flatten().unwrap_or_default();
                obj.insert("id".to_string(), serde_json::Value::String(id));
                if obj
                    .get("object")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    obj.insert(
                        "object".to_string(),
                        serde_json::Value::String("model".to_string()),
                    );
                }
                if obj
                    .get("owned_by")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    obj.insert(
                        "owned_by".to_string(),
                        serde_json::Value::String("worker-proxy".to_string()),
                    );
                }
                obj
            })
            .collect()
    }

    /// Identifies connected legacy workers (those that registered without native
    /// `model_metadata`) that are eligible for a lightweight cached `GET /v1/models`
    /// metadata probe.
    #[must_use]
    pub fn legacy_metadata_probe_targets(
        &self,
        provider: &str,
        owner_key_id: Option<&str>,
        now: Instant,
        ttl: Duration,
    ) -> Vec<String> {
        let mut targets = Vec::new();
        for exact_only in [true, false] {
            for worker_id in &self.worker_order {
                let Some(worker) = self.workers.get(worker_id) else {
                    continue;
                };
                if !worker.visible_to_owner(owner_key_id) {
                    continue;
                }
                if exact_only && worker.provider != provider {
                    continue;
                }
                if !exact_only && worker.provider == provider {
                    continue;
                }
                if worker.native_metadata_reported
                    || worker.is_draining
                    || !worker.supports_endpoint("/v1/models")
                    || !self.worker_notifiers.contains_key(worker_id)
                    || worker.selection_load() >= worker.max_concurrent
                {
                    continue;
                }
                if worker
                    .legacy_metadata_probed_at
                    .is_some_and(|probed_at| now.saturating_duration_since(probed_at) < ttl)
                {
                    continue;
                }
                targets.push(worker_id.clone());
            }
        }
        targets
    }

    /// Dispatches a non-queued `GET /v1/models` probe directly to a specific connected
    /// worker to populate cached metadata for legacy workers that did not send
    /// `model_metadata` in `Register`. Returns `None` if the worker is unavailable or busy.
    pub fn submit_worker_models_probe(&mut self, worker_id: &str) -> Option<PendingHttpResponse> {
        let (provider, owner_key_id) = {
            let worker = self.workers.get_mut(worker_id)?;
            if worker.is_draining
                || worker.native_metadata_reported
                || worker.selection_load() >= worker.max_concurrent
                || !worker.supports_endpoint("/v1/models")
                || !self.worker_notifiers.contains_key(worker_id)
            {
                return None;
            }
            worker.legacy_metadata_probed_at = Some(Instant::now());
            (worker.provider.clone(), worker.owner_key_id.clone())
        };

        let request_id = format!("request-{}", self.next_request_id);
        self.next_request_id += 1;
        let request = RequestRecord {
            request_id: request_id.clone(),
            provider,
            model: "*".to_string(),
            owner_key_id,
            queued_at: Instant::now(),
            transport: RequestTransport {
                endpoint_path: "/v1/models".to_string(),
                method: "GET".to_string(),
                is_streaming: false,
                body: String::new(),
                headers: HeaderMap::new(),
            },
            provider_routing: None,
            requeue_count: 0,
        };
        let (tx, rx) = oneshot::channel();
        self.pending_http_response_senders
            .insert(request_id.clone(), PendingHttpResponseSender::Unary(tx));
        self.assign_to_worker(worker_id, request);

        Some(PendingHttpResponse {
            request_id,
            response_rx: rx,
        })
    }

    /// Stores metadata obtained from a legacy worker `GET /v1/models` probe unless
    /// the worker has since reported native metadata.
    pub fn apply_probed_worker_metadata(
        &mut self,
        worker_id: &str,
        probed_metadata: ModelMetadataMap,
    ) {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            return;
        };
        if worker.native_metadata_reported {
            return;
        }
        worker.model_metadata = sanitize_model_metadata(probed_metadata, &worker.models);
    }

    #[must_use]
    pub fn worker_ids_for_provider(&self, provider: &str) -> Vec<String> {
        self.worker_order
            .iter()
            .filter(|worker_id| {
                self.workers
                    .get(*worker_id)
                    .is_some_and(|worker| worker.provider == provider)
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn queued_request_ids(&self, provider: &str) -> Vec<String> {
        self.provider_queues
            .get(provider)
            .into_iter()
            .flat_map(|queue| queue.iter().map(|request| request.request_id.clone()))
            .collect()
    }

    #[must_use]
    pub fn worker_in_flight_request_ids(&self, worker_id: &str) -> Vec<String> {
        self.workers
            .get(worker_id)
            .map(|worker| worker.in_flight_requests.clone())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn has_worker(&self, worker_id: &str) -> bool {
        self.workers.contains_key(worker_id)
    }

    #[must_use]
    pub fn worker_is_draining(&self, worker_id: &str) -> bool {
        self.workers
            .get(worker_id)
            .is_some_and(|worker| worker.is_draining)
    }

    #[must_use]
    pub(crate) fn worker_graceful_shutdown_deadline(&self, worker_id: &str) -> Option<Instant> {
        self.workers
            .get(worker_id)
            .and_then(|worker| worker.graceful_shutdown_deadline)
    }

    pub fn disconnect_drained_worker_if_idle(&mut self, worker_id: &str) -> bool {
        let should_disconnect = self
            .workers
            .get(worker_id)
            .is_some_and(|worker| worker.is_draining && worker.in_flight_requests.is_empty());

        if should_disconnect {
            self.graceful_shutdown_disconnect_reasons.insert(
                worker_id.to_string(),
                GracefulShutdownDisconnectReason::Completed,
            );
            self.remove_worker(worker_id);
        }

        should_disconnect
    }

    pub(crate) fn take_graceful_shutdown_disconnect_reason(
        &mut self,
        worker_id: &str,
    ) -> Option<GracefulShutdownDisconnectReason> {
        self.graceful_shutdown_disconnect_reasons.remove(worker_id)
    }

    #[must_use]
    pub fn worker_reported_load(&self, worker_id: &str) -> Option<usize> {
        self.workers
            .get(worker_id)
            .map(|worker| worker.reported_load)
    }

    #[must_use]
    pub fn request_state(&self, request_id: &str) -> Option<RequestState> {
        match self.active_requests.get(request_id)? {
            ActiveRequestState::Queued { .. } => Some(RequestState::Queued),
            ActiveRequestState::InFlight {
                worker_id,
                cancellation,
                ..
            } => Some(RequestState::InFlight {
                worker_id: worker_id.clone(),
                cancellation: *cancellation,
            }),
        }
    }

    #[must_use]
    pub fn worker_cancel_signals(&self) -> Vec<WorkerCancelSignal> {
        self.worker_cancel_signals.clone()
    }

    fn dispatch_next_compatible(&mut self, worker_id: &str) -> Option<DispatchAssignment> {
        if !self.worker_has_capacity(worker_id) {
            return None;
        }
        let worker = self.workers.get(worker_id)?.clone();

        // Search all provider queues, enforcing model, endpoint, tenant owner,
        // and ProviderRouting constraints (only/ignore/allow_fallbacks).
        let mut best_provider = None;
        let mut best_index = None;
        let mut best_queued_at = None;
        for (queue_provider, queue) in &self.provider_queues {
            if let Some(idx) = queue
                .iter()
                .position(|request| worker_matches_request(&worker, request, false))
            {
                let queued_at = queue[idx].queued_at;
                if best_queued_at.is_none_or(|best| queued_at < best) {
                    best_provider = Some(queue_provider.clone());
                    best_index = Some(idx);
                    best_queued_at = Some(queued_at);
                }
            }
        }

        // Second pass for queued requests that allow fallback from `only`.
        if best_provider.is_none() {
            for (queue_provider, queue) in &self.provider_queues {
                if let Some(idx) = queue
                    .iter()
                    .position(|request| worker_matches_request(&worker, request, true))
                {
                    let queued_at = queue[idx].queued_at;
                    if best_queued_at.is_none_or(|best| queued_at < best) {
                        best_provider = Some(queue_provider.clone());
                        best_index = Some(idx);
                        best_queued_at = Some(queued_at);
                    }
                }
            }
        }

        let provider_key = best_provider?;
        let queue_index = best_index?;
        let queue = self.provider_queues.get_mut(&provider_key)?;
        let request = queue.remove(queue_index)?;

        self.assign_to_worker(worker_id, request.clone());
        Some(DispatchAssignment {
            request_id: request.request_id,
            worker_id: worker_id.to_string(),
        })
    }

    fn find_eligible_worker_id(&mut self, request: &RequestRecord) -> Option<String> {
        if let Some(routing) = &request.provider_routing {
            return self.find_eligible_worker_id_with_routing(request, routing);
        }
        if let Some(id) =
            self.find_eligible_worker_id_for_provider(Some(&request.provider), request, false)
        {
            return Some(id);
        }
        self.find_eligible_worker_id_for_provider(None, request, false)
    }

    fn find_eligible_worker_id_with_routing(
        &mut self,
        request: &RequestRecord,
        routing: &ProviderRouting,
    ) -> Option<String> {
        let eligible_workers: Vec<(String, usize, String)> = self
            .worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                if worker.selection_load() >= worker.max_concurrent
                    || !worker_matches_request(worker, request, false)
                {
                    return None;
                }
                Some((
                    worker_id.clone(),
                    worker.selection_load(),
                    worker.provider.clone(),
                ))
            })
            .collect();

        if eligible_workers.is_empty() {
            if routing.allow_fallbacks.unwrap_or(true) {
                if let Some(id) = self.find_eligible_worker_id_for_provider(
                    Some(&request.provider),
                    request,
                    true,
                ) {
                    return Some(id);
                }
                return self.find_eligible_worker_id_for_provider(None, request, true);
            }
            return None;
        }

        let sort_round_robin = routing
            .sort
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("round_robin"));

        if let Some(order) = &routing.order {
            for preferred in order {
                let preferred_workers: Vec<(String, usize)> = eligible_workers
                    .iter()
                    .filter(|(_, _, prov)| prov == preferred)
                    .map(|(id, load, _)| (id.clone(), *load))
                    .collect();
                if !preferred_workers.is_empty() {
                    return self.pick_from_candidates(
                        preferred,
                        &request.model,
                        &preferred_workers,
                        sort_round_robin,
                    );
                }
            }
        }

        let candidates: Vec<(String, usize)> = eligible_workers
            .into_iter()
            .map(|(id, load, _)| (id, load))
            .collect();
        self.pick_from_candidates(
            &request.provider,
            &request.model,
            &candidates,
            sort_round_robin,
        )
    }

    fn find_eligible_worker_id_for_provider(
        &mut self,
        provider_filter: Option<&str>,
        request: &RequestRecord,
        allow_fallback_pass: bool,
    ) -> Option<String> {
        let candidates: Vec<(String, usize)> = self
            .worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                let selection_load = worker.selection_load();
                let has_capacity = selection_load < worker.max_concurrent;
                let provider_matches =
                    provider_filter.is_none_or(|provider| worker.provider == provider);

                if provider_matches
                    && has_capacity
                    && worker_matches_request(worker, request, allow_fallback_pass)
                {
                    Some((worker_id.clone(), selection_load))
                } else {
                    None
                }
            })
            .collect();

        if candidates.is_empty() {
            return None;
        }

        let effective_provider = provider_filter.unwrap_or("_any");
        self.pick_from_candidates(effective_provider, &request.model, &candidates, false)
    }

    fn pick_from_candidates(
        &mut self,
        effective_provider: &str,
        model: &str,
        candidates: &[(String, usize)],
        pure_round_robin: bool,
    ) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }

        let pool: Vec<&str> = if pure_round_robin {
            candidates.iter().map(|(id, _)| id.as_str()).collect()
        } else {
            let lowest_load = candidates.iter().map(|(_, load)| *load).min()?;
            candidates
                .iter()
                .filter(|(_, load)| *load == lowest_load)
                .map(|(id, _)| id.as_str())
                .collect()
        };

        let key = SelectionKey::new(effective_provider, model);
        let last_worker_id = self.selection_cursors.get(&key).map(String::as_str);

        // Find the next candidate strictly after `last_worker_id` in `self.worker_order`.
        let chosen = if let Some(last_id) = last_worker_id {
            let last_pos = self.worker_order.iter().position(|id| id == last_id);
            match last_pos {
                Some(pos) => pool
                    .iter()
                    .find(|cand| {
                        self.worker_order
                            .iter()
                            .position(|id| id == *cand)
                            .is_some_and(|cand_pos| cand_pos > pos)
                    })
                    .or_else(|| pool.first())
                    .copied()?,
                None => *pool.first()?,
            }
        } else {
            *pool.first()?
        };

        let chosen_owned = chosen.to_string();
        if self.selection_cursors.len() >= MAX_SELECTION_CURSORS
            && !self.selection_cursors.contains_key(&key)
        {
            self.selection_cursors.clear();
        }
        self.selection_cursors.insert(key, chosen_owned.clone());
        Some(chosen_owned)
    }

    fn assign_to_worker(&mut self, worker_id: &str, request: RequestRecord) {
        if let Some(worker) = self.workers.get_mut(worker_id) {
            worker.in_flight_requests.push(request.request_id.clone());
            worker.reported_load = worker.reported_load.saturating_add(1);
        }
        self.pending_worker_requests
            .entry(worker_id.to_string())
            .or_default()
            .push_back(request.to_worker_request());
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::InFlight {
                request,
                worker_id: worker_id.to_string(),
                cancellation: None,
                cancelled_at: None,
                has_emitted_chunk: false,
            },
        );
        self.notify_worker(worker_id);
    }

    fn worker_has_capacity(&self, worker_id: &str) -> bool {
        self.workers.get(worker_id).is_some_and(|worker| {
            !worker.is_draining && worker.selection_load() < worker.max_concurrent
        })
    }

    fn has_compatible_worker(&self, request: &RequestRecord) -> bool {
        let allow_fallback = request
            .provider_routing
            .as_ref()
            .is_some_and(|r| r.allow_fallbacks.unwrap_or(true));
        self.workers.values().any(|worker| {
            worker_matches_request(worker, request, false)
                || (allow_fallback && worker_matches_request(worker, request, true))
        })
    }

    fn fail_pending_http_response(
        &mut self,
        request_id: &str,
        reason: RequestFailureReason,
    ) -> bool {
        let Some(sender) = self.pending_http_response_senders.remove(request_id) else {
            return false;
        };

        match sender {
            PendingHttpResponseSender::Unary(sender) => sender.send(Err(reason)).is_ok(),
            PendingHttpResponseSender::Streaming(sender) => {
                sender.try_send(HttpResponseEvent::Failure(reason)).is_ok()
            }
        }
    }

    fn remove_worker(&mut self, worker_id: &str) -> Option<WorkerState> {
        let worker = self.workers.remove(worker_id)?;
        self.worker_order
            .retain(|registered_worker_id| registered_worker_id != worker_id);
        self.worker_notifiers.remove(worker_id);
        self.pending_worker_requests.remove(worker_id);
        self.worker_cancel_signals
            .retain(|signal| signal.worker_id != worker_id);
        self.worker_graceful_shutdown_signals
            .retain(|signal| signal.worker_id != worker_id);
        self.worker_models_refresh_signals
            .retain(|signal| signal.worker_id != worker_id);
        Some(worker)
    }
}

fn worker_matches_request(
    worker: &WorkerState,
    request: &RequestRecord,
    allow_fallback_pass: bool,
) -> bool {
    if worker.is_draining {
        return false;
    }
    if !worker.visible_to_owner(request.owner_key_id.as_deref()) {
        return false;
    }
    if !worker
        .models
        .iter()
        .any(|supported| supported == "*" || supported == &request.model)
    {
        return false;
    }
    if !worker.supports_endpoint(&request.transport.endpoint_path) {
        return false;
    }

    if let Some(routing) = &request.provider_routing {
        // `ignore` is always honored, even during fallback passes.
        if let Some(ignore) = &routing.ignore
            && ignore.iter().any(|p| p == &worker.provider)
        {
            return false;
        }
        if let Some(only) = &routing.only
            && !only.iter().any(|p| p == &worker.provider)
        {
            if allow_fallback_pass && routing.allow_fallbacks.unwrap_or(true) {
                return true;
            }
            return false;
        }
    }

    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorker {
    pub worker_id: String,
    pub ack: RegisterAck,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AdminWorkerInfo {
    pub worker_id: String,
    pub provider: String,
    pub worker_name: String,
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_prefixes: Vec<String>,
    pub max_concurrent: usize,
    pub reported_load: usize,
    pub in_flight_count: usize,
    pub is_draining: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderQueuePolicy {
    pub max_queue_len: usize,
    pub queue_timeout_ticks: Option<u64>,
}

impl Default for ProviderQueuePolicy {
    fn default() -> Self {
        Self {
            max_queue_len: usize::MAX,
            queue_timeout_ticks: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Dispatched(DispatchAssignment),
    Queued(QueuedAssignment),
    Rejected(RequestFailure),
}

#[derive(Debug)]
pub struct PendingHttpResponse {
    pub request_id: String,
    pub response_rx: oneshot::Receiver<Result<ResponseCompleteMessage, RequestFailureReason>>,
}

#[derive(Debug)]
pub struct PendingStreamingHttpResponse {
    pub request_id: String,
    pub event_rx: mpsc::Receiver<HttpResponseEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpResponseEvent {
    Chunk(String),
    Complete(ResponseCompleteMessage),
    Failure(RequestFailureReason),
    WorkerError(String),
}

#[derive(Debug)]
enum PendingHttpResponseSender {
    Unary(oneshot::Sender<Result<ResponseCompleteMessage, RequestFailureReason>>),
    Streaming(mpsc::Sender<HttpResponseEvent>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchAssignment {
    pub request_id: String,
    pub worker_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedAssignment {
    pub request_id: String,
    pub queue_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    ClientDisconnected,
    RequestTimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancellationOutcome {
    RemovedFromQueue { request_id: String },
    WorkerCancelSent(WorkerCancelSignal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCancelSignal {
    pub worker_id: String,
    pub request_id: String,
    pub reason: CancelReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDisconnectOutcome {
    pub requeued_request_ids: Vec<String>,
    pub failed_requests: Vec<RequestFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GracefulShutdownSignal {
    pub worker_id: String,
    pub reason: Option<String>,
    pub drain_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerModelsRefreshSignal {
    pub worker_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GracefulShutdownOutcome {
    pub disconnected_worker_ids: Vec<String>,
    pub failed_requests: Vec<RequestFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GracefulShutdownDisconnectReason {
    Completed,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestState {
    Queued,
    InFlight {
        worker_id: String,
        cancellation: Option<CancelReason>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestFailure {
    pub request_id: String,
    pub reason: RequestFailureReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestFailureReason {
    RequestAlreadyCanceled,
    MaxRequeuesExceeded,
    QueueTimedOut,
    QueueFull,
    NoWorkersAvailable,
    ProviderDisabled,
    ProviderDeleted,
    GracefulShutdownTimedOut,
    WorkerDisconnectedMidStream,
}

#[derive(Debug, Clone, PartialEq)]
struct WorkerState {
    provider: String,
    worker_name: String,
    owner_key_id: Option<String>,
    models: Vec<String>,
    model_metadata: ModelMetadataMap,
    native_metadata_reported: bool,
    legacy_metadata_probed_at: Option<Instant>,
    endpoint_prefixes: Vec<String>,
    max_concurrent: usize,
    reported_load: usize,
    in_flight_requests: Vec<String>,
    is_draining: bool,
    graceful_shutdown_deadline: Option<Instant>,
}

impl WorkerState {
    fn selection_load(&self) -> usize {
        self.reported_load.max(self.in_flight_requests.len())
    }

    /// Returns true if this worker can serve requests from `owner_key_id`.
    ///
    /// Shared fleet workers (`self.owner_key_id == None`) can serve any client.
    /// Tenant-scoped workers (`self.owner_key_id == Some(w)`) only serve clients
    /// with the matching API key (`Some(w)`), or unauthenticated requests when
    /// API key enforcement is disabled (`None`).
    fn visible_to_owner(&self, request_owner: Option<&str>) -> bool {
        match (self.owner_key_id.as_deref(), request_owner) {
            (Some(worker_owner), Some(req_owner)) => worker_owner == req_owner,
            _ => true,
        }
    }

    /// Returns true if this worker can handle requests to the given endpoint path.
    /// An empty prefixes list means the worker accepts any endpoint (legacy behavior).
    fn supports_endpoint(&self, path: &str) -> bool {
        if self.endpoint_prefixes.is_empty() {
            return true;
        }
        self.endpoint_prefixes
            .iter()
            .any(|prefix| path.starts_with(prefix.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestRecord {
    request_id: String,
    provider: String,
    model: String,
    owner_key_id: Option<String>,
    queued_at: Instant,
    transport: RequestTransport,
    provider_routing: Option<ProviderRouting>,
    requeue_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestTransport {
    endpoint_path: String,
    method: String,
    is_streaming: bool,
    body: String,
    headers: HeaderMap,
}

impl RequestRecord {
    fn to_worker_request(&self) -> RequestMessage {
        RequestMessage {
            request_id: self.request_id.clone(),
            model: self.model.clone(),
            endpoint_path: self.transport.endpoint_path.clone(),
            method: self.transport.method.clone(),
            is_streaming: self.transport.is_streaming,
            body: self.transport.body.clone(),
            headers: self.transport.headers.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveRequestState {
    Queued {
        request: RequestRecord,
    },
    InFlight {
        request: RequestRecord,
        worker_id: String,
        cancellation: Option<CancelReason>,
        cancelled_at: Option<Instant>,
        has_emitted_chunk: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SelectionKey {
    provider: String,
    model: String,
}

impl SelectionKey {
    fn new(provider: &str, model: &str) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
        }
    }
}

fn sanitize_models(models: Vec<String>) -> Vec<String> {
    let mut sanitized = Vec::new();
    let mut seen = HashSet::new();

    for model in models {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            continue;
        }

        if seen.insert(trimmed.to_string()) {
            sanitized.push(trimmed.to_string());
        }
    }

    sanitized
}

fn sanitize_model_metadata(
    metadata: ModelMetadataMap,
    sanitized_models: &[String],
) -> ModelMetadataMap {
    if metadata.is_empty() {
        return ModelMetadataMap::new();
    }
    let allowed: HashSet<&str> = sanitized_models.iter().map(String::as_str).collect();
    let allow_all = allowed.contains("*");
    let mut cleaned = ModelMetadataMap::new();

    for (raw_id, obj) in metadata {
        if cleaned.len() >= MAX_MODEL_METADATA_ENTRIES {
            break;
        }
        let id = raw_id.trim();
        if id.is_empty() || (!allow_all && !allowed.contains(id)) {
            continue;
        }
        if serde_json::to_string(&obj)
            .is_ok_and(|serialized| serialized.len() <= MAX_MODEL_METADATA_BYTES)
        {
            cleaned.insert(id.to_string(), obj);
        }
    }

    cleaned
}

fn registration_warnings(worker_name: &str, sanitized_models: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();

    if worker_name.trim().is_empty() {
        warnings.push("worker_name was empty after trimming".to_string());
    }
    if sanitized_models.is_empty() {
        warnings.push("worker registered without any routable models".to_string());
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::{
        CancelReason, CancellationOutcome, DispatchAssignment, ProviderQueuePolicy,
        ProxyServerCore, QueuedAssignment, RequestFailure, RequestFailureReason, RequestState,
        SubmissionOutcome, WorkerCancelSignal, WorkerDisconnectOutcome,
    };
    use modelrelay_protocol::{
        HeaderMap, ModelMetadataMap, ModelsUpdateMessage, ProviderRouting, RegisterMessage,
    };
    use std::time::Duration;

    fn register_message(
        worker_name: &str,
        models: &[&str],
        max_concurrent: u32,
        current_load: Option<u32>,
    ) -> RegisterMessage {
        RegisterMessage {
            worker_name: worker_name.to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            max_concurrent,
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load,
            endpoint_prefixes: vec![],
            model_metadata: ModelMetadataMap::new(),
        }
    }

    #[test]
    fn registers_workers_and_tracks_provider_model_catalog() {
        let mut core = ProxyServerCore::new();

        let registered = core.register_worker(
            "openai",
            register_message(
                "gpu-box-a",
                &["llama-3.1-70b", "llama-3.1-70b", " mistral-large ", ""],
                2,
                Some(0),
            ),
        );

        assert_eq!(registered.worker_id, "worker-1");
        assert_eq!(registered.ack.worker_id, "worker-1");
        assert_eq!(
            registered.ack.models,
            vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()]
        );
        assert_eq!(
            registered.ack.protocol_version.as_deref(),
            Some("2026-04-bridge-v1")
        );
        assert!(registered.ack.warnings.is_empty());
        assert_eq!(
            core.provider_models("openai"),
            vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()]
        );
    }

    #[test]
    fn selects_lowest_load_and_rotates_among_equal_exact_matches() {
        let mut core = ProxyServerCore::new();
        let first = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 3, Some(0)),
            )
            .worker_id;
        let second = core
            .register_worker(
                "openai",
                register_message("gpu-b", &["llama-3.1-70b"], 3, Some(0)),
            )
            .worker_id;
        let third = core
            .register_worker(
                "openai",
                register_message("gpu-c", &["llama-3.1-70b"], 3, Some(1)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: first.clone(),
            })
        );
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-2".to_string(),
                worker_id: second.clone(),
            })
        );
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-3".to_string(),
                worker_id: third.clone(),
            })
        );
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-4".to_string(),
                worker_id: first.clone(),
            })
        );
    }

    #[test]
    fn bounds_queue_per_provider_and_preserves_existing_entries() {
        let mut core = ProxyServerCore::new();
        core.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 1,
                queue_timeout_ticks: None,
            },
        );

        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Rejected(RequestFailure {
                request_id: "request-3".to_string(),
                reason: RequestFailureReason::QueueFull,
            })
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
        assert_eq!(
            core.worker_in_flight_request_ids(&worker_id),
            vec!["request-1".to_string()]
        );
    }

    #[test]
    fn models_update_changes_routing_without_worker_reconnect() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-1".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            core.update_worker_models(
                &worker_id,
                ModelsUpdateMessage {
                    models: vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()],
                    current_load: 0,
                    model_metadata: ModelMetadataMap::new(),
                }
            ),
            vec![DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            }]
        );
        assert!(core.queued_request_ids("openai").is_empty());

        assert_eq!(core.finish_request(&worker_id, "request-1"), None);

        assert!(
            core.update_worker_models(
                &worker_id,
                ModelsUpdateMessage {
                    models: vec!["llama-3.1-70b".to_string()],
                    current_load: 0,
                    model_metadata: ModelMetadataMap::new(),
                }
            )
            .is_empty()
        );
        assert_eq!(
            core.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
    }

    #[test]
    fn queued_dispatch_remains_fifo_among_compatible_requests() {
        let mut core = ProxyServerCore::new();
        let llama_worker = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        let mistral_worker = core
            .register_worker(
                "openai",
                register_message("gpu-b", &["mistral-large"], 1, Some(0)),
            )
            .worker_id;

        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(_)
        ));

        assert_eq!(
            core.finish_request(&llama_worker, "request-1"),
            Some(DispatchAssignment {
                request_id: "request-4".to_string(),
                worker_id: llama_worker,
            })
        );
        assert_eq!(
            core.finish_request(&mistral_worker, "request-2"),
            Some(DispatchAssignment {
                request_id: "request-3".to_string(),
                worker_id: mistral_worker,
            })
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-5".to_string()]
        );
    }

    #[test]
    fn canceling_queued_request_removes_it_before_dispatch() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );

        assert_eq!(
            core.cancel_request("request-2", CancelReason::ClientDisconnected),
            Some(CancellationOutcome::RemovedFromQueue {
                request_id: "request-2".to_string(),
            })
        );
        assert!(core.queued_request_ids("openai").is_empty());
        assert_eq!(core.request_state("request-2"), None);
        assert_eq!(core.finish_request(&worker_id, "request-1"), None);
        assert!(core.worker_in_flight_request_ids(&worker_id).is_empty());
    }

    #[test]
    fn queued_request_times_out_after_waiting_for_worker_capacity() {
        let mut core = ProxyServerCore::new();
        core.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 2,
                queue_timeout_ticks: Some(5),
            },
        );
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );

        let timeout_at = match core.active_requests.get("request-2") {
            Some(super::ActiveRequestState::Queued { request }) => {
                request.queued_at + Duration::from_secs(5)
            }
            _ => panic!("request-2 should still be queued"),
        };

        assert_eq!(
            core.expire_queue_timeouts(timeout_at),
            vec![RequestFailure {
                request_id: "request-2".to_string(),
                reason: RequestFailureReason::QueueTimedOut,
            }]
        );
        assert!(core.queued_request_ids("openai").is_empty());
        assert_eq!(core.request_state("request-2"), None);
        assert_eq!(core.finish_request(&worker_id, "request-1"), None);
    }

    #[test]
    fn queue_timeout_removes_only_expired_requests_from_the_provider_queue() {
        let mut core = ProxyServerCore::new();
        core.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 3,
                queue_timeout_ticks: Some(5),
            },
        );
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(_)
        ));
        assert!(matches!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(_)
        ));

        let cutoff = {
            let queue = core
                .provider_queues
                .get_mut("openai")
                .expect("openai queue should exist");
            let first = queue
                .iter_mut()
                .find(|request| request.request_id == "request-2")
                .expect("request-2 should still be queued");
            let cutoff = first.queued_at + Duration::from_secs(5);
            first.queued_at -= Duration::from_secs(5);

            let second = queue
                .iter_mut()
                .find(|request| request.request_id == "request-3")
                .expect("request-3 should still be queued");
            second.queued_at = cutoff.checked_sub(Duration::from_secs(1)).unwrap();
            cutoff
        };

        if let Some(super::ActiveRequestState::Queued { request }) =
            core.active_requests.get_mut("request-2")
        {
            request.queued_at -= Duration::from_secs(5);
        }
        if let Some(super::ActiveRequestState::Queued { request }) =
            core.active_requests.get_mut("request-3")
        {
            request.queued_at = cutoff.checked_sub(Duration::from_secs(1)).unwrap();
        }

        assert_eq!(
            core.expire_queue_timeouts(cutoff),
            vec![RequestFailure {
                request_id: "request-2".to_string(),
                reason: RequestFailureReason::QueueTimedOut,
            }]
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-3".to_string()]
        );
        assert_eq!(core.request_state("request-2"), None);
        assert_eq!(core.request_state("request-3"), Some(RequestState::Queued));
        assert_eq!(
            core.finish_request(&worker_id, "request-1"),
            Some(DispatchAssignment {
                request_id: "request-3".to_string(),
                worker_id,
            })
        );
    }

    #[test]
    fn canceling_in_flight_request_records_worker_signal_until_finish() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            })
        );
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::InFlight {
                worker_id: worker_id.clone(),
                cancellation: None,
            })
        );
        assert_eq!(
            core.cancel_request("request-1", CancelReason::ClientDisconnected),
            Some(CancellationOutcome::WorkerCancelSent(WorkerCancelSignal {
                worker_id: worker_id.clone(),
                request_id: "request-1".to_string(),
                reason: CancelReason::ClientDisconnected,
            }))
        );
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::InFlight {
                worker_id: worker_id.clone(),
                cancellation: Some(CancelReason::ClientDisconnected),
            })
        );
        assert_eq!(
            core.worker_cancel_signals(),
            vec![WorkerCancelSignal {
                worker_id: worker_id.clone(),
                request_id: "request-1".to_string(),
                reason: CancelReason::ClientDisconnected,
            }]
        );

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            core.finish_request(&worker_id, "request-1"),
            Some(DispatchAssignment {
                request_id: "request-2".to_string(),
                worker_id: worker_id.clone(),
            })
        );
        assert_eq!(core.request_state("request-1"), None);
        assert_eq!(
            core.worker_in_flight_request_ids(&worker_id),
            vec!["request-2".to_string()]
        );
    }

    #[test]
    fn disconnecting_a_worker_requeues_a_live_in_flight_request() {
        let mut core = ProxyServerCore::new();
        let first = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        let second = core
            .register_worker(
                "openai",
                register_message("gpu-b", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: first.clone(),
            })
        );

        assert_eq!(
            core.disconnect_worker(&first),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: vec!["request-1".to_string()],
                failed_requests: Vec::new(),
            })
        );
        // Idle compatible replacement worker receives the requeued request immediately
        // without waiting for an unrelated models update or completion event.
        assert_eq!(
            core.worker_in_flight_request_ids(&second),
            vec!["request-1".to_string()]
        );
        assert!(core.queued_request_ids("openai").is_empty());
    }

    #[test]
    fn disconnecting_only_worker_queues_request_until_replacement_registers() {
        let mut core = ProxyServerCore::new();
        let first = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: first.clone(),
            })
        );

        assert_eq!(
            core.disconnect_worker(&first),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: vec!["request-1".to_string()],
                failed_requests: Vec::new(),
            })
        );
        assert_eq!(core.request_state("request-1"), Some(RequestState::Queued));
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-1".to_string()]
        );

        // Registering a new compatible worker immediately drains the queued request.
        let second = core
            .register_worker(
                "openai",
                register_message("gpu-b", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        assert_eq!(
            core.worker_in_flight_request_ids(&second),
            vec!["request-1".to_string()]
        );
        assert!(core.queued_request_ids("openai").is_empty());
    }

    #[test]
    fn disconnecting_a_worker_does_not_requeue_a_request_after_it_was_canceled() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            })
        );
        assert_eq!(
            core.cancel_request("request-1", CancelReason::ClientDisconnected),
            Some(CancellationOutcome::WorkerCancelSent(WorkerCancelSignal {
                worker_id: worker_id.clone(),
                request_id: "request-1".to_string(),
                reason: CancelReason::ClientDisconnected,
            }))
        );

        assert_eq!(
            core.disconnect_worker(&worker_id),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: Vec::new(),
                failed_requests: vec![RequestFailure {
                    request_id: "request-1".to_string(),
                    reason: RequestFailureReason::RequestAlreadyCanceled,
                }],
            })
        );
        assert_eq!(core.request_state("request-1"), None);
        assert!(core.queued_request_ids("openai").is_empty());
    }

    #[test]
    fn repeated_worker_disconnects_stop_requeueing_after_the_max_attempts() {
        let mut core = ProxyServerCore::new();
        let worker_one = core
            .register_worker(
                "openai",
                register_message("gpu-a", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        let worker_two = core
            .register_worker(
                "openai",
                register_message("gpu-b", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        let worker_three = core
            .register_worker(
                "openai",
                register_message("gpu-c", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;
        let worker_four = core
            .register_worker(
                "openai",
                register_message("gpu-d", &["llama-3.1-70b"], 1, Some(0)),
            )
            .worker_id;

        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_one.clone(),
            })
        );

        let requeues = [
            (worker_one, worker_two.clone()),
            (worker_two.clone(), worker_three.clone()),
            (worker_three.clone(), worker_four.clone()),
        ];

        for (disconnected_worker, replacement_worker) in requeues {
            assert_eq!(
                core.disconnect_worker(&disconnected_worker),
                Some(WorkerDisconnectOutcome {
                    requeued_request_ids: vec!["request-1".to_string()],
                    failed_requests: Vec::new(),
                })
            );
            assert_eq!(
                core.worker_in_flight_request_ids(&replacement_worker),
                vec!["request-1".to_string()]
            );
        }

        assert_eq!(
            core.disconnect_worker(&worker_four),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: Vec::new(),
                failed_requests: vec![RequestFailure {
                    request_id: "request-1".to_string(),
                    reason: RequestFailureReason::MaxRequeuesExceeded,
                }],
            })
        );
        assert_eq!(core.request_state("request-1"), None);
        assert!(core.queued_request_ids("openai").is_empty());
    }

    #[test]
    fn wildcard_model_matches_any_requested_model() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker("openai", register_message("gpu-a", &["*"], 2, Some(0)))
            .worker_id;

        // Direct dispatch should work for any model name
        assert_eq!(
            core.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            })
        );

        assert_eq!(
            core.submit_request("openai", "gpt-4o"),
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-2".to_string(),
                worker_id: worker_id.clone(),
            })
        );

        // Queue a request, then free capacity — wildcard worker should drain it
        assert_eq!(
            core.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-3".to_string(),
                queue_len: 1,
            })
        );

        // Finish a request to free capacity — dispatch_next_compatible runs inside
        // finish_request, so the queued request-3 should be dispatched immediately.
        let dispatched = core.finish_request(&worker_id, "request-1");
        assert_eq!(
            dispatched,
            Some(DispatchAssignment {
                request_id: "request-3".to_string(),
                worker_id: worker_id.clone(),
            })
        );
    }

    #[test]
    fn wildcard_worker_is_recognized_as_compatible() {
        let mut core = ProxyServerCore::new();
        let _worker_id = core
            .register_worker("openai", register_message("gpu-a", &["*"], 1, Some(0)))
            .worker_id;

        // With a wildcard worker, submitting an unknown model should dispatch, not queue
        // (provider_has_compatible_worker should return true)
        assert!(matches!(
            core.submit_request("openai", "any-random-model"),
            SubmissionOutcome::Dispatched(_)
        ));
    }

    #[test]
    fn provider_models_filters_out_wildcard() {
        let mut core = ProxyServerCore::new();
        core.register_worker(
            "openai",
            register_message("gpu-a", &["gpt-4", "*", "gpt-3.5"], 1, Some(0)),
        );
        core.register_worker("openai", register_message("gpu-b", &["gpt-4"], 1, Some(0)));

        let models = core.provider_models("openai");
        assert!(
            !models.contains(&"*".to_string()),
            "wildcard must be filtered out"
        );
        assert!(models.contains(&"gpt-4".to_string()));
        assert!(models.contains(&"gpt-3.5".to_string()));
    }

    // ---- helper for workers with endpoint_prefixes ----

    fn register_message_with_endpoints(
        worker_name: &str,
        models: &[&str],
        max_concurrent: u32,
        current_load: Option<u32>,
        endpoint_prefixes: &[&str],
    ) -> RegisterMessage {
        RegisterMessage {
            worker_name: worker_name.to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            max_concurrent,
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load,
            endpoint_prefixes: endpoint_prefixes
                .iter()
                .map(|ep| (*ep).to_string())
                .collect(),
            model_metadata: ModelMetadataMap::new(),
        }
    }

    /// Helper: submit a request with provider routing through the transport API.
    fn submit_with_routing(
        core: &mut ProxyServerCore,
        provider: &str,
        model: &str,
        routing: ProviderRouting,
    ) -> SubmissionOutcome {
        core.submit_transport_request(
            provider,
            model,
            "/v1/chat/completions",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            Some(routing),
        )
    }

    // ---- Provider Routing: only ----

    #[test]
    fn routing_only_dispatches_to_listed_providers() {
        let mut core = ProxyServerCore::new();
        let _anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, openai),
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    #[test]
    fn routing_only_rejects_when_no_listed_provider_available_and_no_fallbacks() {
        let mut core = ProxyServerCore::new();
        core.register_worker(
            "anthropic",
            register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
        );

        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        // No openai worker exists, fallbacks disabled → should queue or reject (no workers match)
        assert!(
            !matches!(result, SubmissionOutcome::Dispatched(_)),
            "should not dispatch to anthropic when only=openai and allow_fallbacks=false"
        );
    }

    // ---- Provider Routing: ignore ----

    #[test]
    fn routing_ignore_skips_specified_providers() {
        let mut core = ProxyServerCore::new();
        let _anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                ignore: Some(vec!["anthropic".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, openai),
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    #[test]
    fn routing_ignore_all_providers_rejects_without_fallbacks() {
        let mut core = ProxyServerCore::new();
        core.register_worker(
            "openai",
            register_message("openai-1", &["gpt-4"], 2, Some(0)),
        );
        core.register_worker(
            "anthropic",
            register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
        );

        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                ignore: Some(vec!["openai".to_string(), "anthropic".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        assert!(
            !matches!(result, SubmissionOutcome::Dispatched(_)),
            "ignoring all providers with no fallbacks should not dispatch"
        );
    }

    // ---- Provider Routing: order ----

    #[test]
    fn routing_order_prefers_first_listed_provider() {
        let mut core = ProxyServerCore::new();
        let _openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // Prefer anthropic first
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                order: Some(vec!["anthropic".to_string(), "openai".to_string()]),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, anthropic),
            other => panic!("expected Dispatched to anthropic, got {other:?}"),
        }
    }

    #[test]
    fn routing_order_falls_through_when_preferred_unavailable() {
        let mut core = ProxyServerCore::new();
        let openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // Prefer azure (no workers), then openai
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                order: Some(vec!["azure".to_string(), "openai".to_string()]),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, openai),
            other => panic!("expected Dispatched to openai fallback, got {other:?}"),
        }
    }

    // ---- Provider Routing: allow_fallbacks ----

    #[test]
    fn routing_allow_fallbacks_true_falls_back_to_any_provider() {
        let mut core = ProxyServerCore::new();
        let anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // Only want "azure" but allow fallbacks (default true)
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["azure".to_string()]),
                allow_fallbacks: Some(true),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, anthropic),
            other => panic!("expected fallback Dispatched to anthropic, got {other:?}"),
        }
    }

    #[test]
    fn routing_allow_fallbacks_default_true() {
        let mut core = ProxyServerCore::new();
        let anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // Only want "azure", allow_fallbacks not set (defaults to true)
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["azure".to_string()]),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, anthropic),
            other => panic!("expected fallback dispatch, got {other:?}"),
        }
    }

    #[test]
    fn routing_allow_fallbacks_false_rejects() {
        let mut core = ProxyServerCore::new();
        core.register_worker(
            "anthropic",
            register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
        );

        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["azure".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        assert!(
            !matches!(result, SubmissionOutcome::Dispatched(_)),
            "should not dispatch when only=azure, no azure workers, fallbacks=false"
        );
    }

    // ---- Provider Routing: combined only + order ----

    #[test]
    fn routing_only_and_order_combined() {
        let mut core = ProxyServerCore::new();
        let openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let _azure = core
            .register_worker("azure", register_message("azure-1", &["gpt-4"], 2, Some(0)))
            .worker_id;

        // Only openai and anthropic, prefer anthropic first
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["openai".to_string(), "anthropic".to_string()]),
                order: Some(vec!["anthropic".to_string(), "openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(
                d.worker_id, anthropic,
                "should dispatch to anthropic (preferred in order)"
            ),
            other => panic!("expected Dispatched, got {other:?}"),
        }

        // Fill anthropic, next request should go to openai
        let result2 = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                only: Some(vec!["openai".to_string(), "anthropic".to_string()]),
                order: Some(vec!["anthropic".to_string(), "openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        match result2 {
            SubmissionOutcome::Dispatched(d) => {
                // anthropic has load=1 now, still has capacity (max=2),
                // so order prefers anthropic again
                assert!(
                    d.worker_id == anthropic || d.worker_id == openai,
                    "should dispatch to anthropic or openai"
                );
            }
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    // ---- Provider Routing: ignore + order combined ----

    #[test]
    fn routing_ignore_and_order_combined() {
        let mut core = ProxyServerCore::new();
        let _openai = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let _anthropic = core
            .register_worker(
                "anthropic",
                register_message("anthropic-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;
        let azure = core
            .register_worker("azure", register_message("azure-1", &["gpt-4"], 2, Some(0)))
            .worker_id;

        // Ignore anthropic, prefer azure first then openai
        let result = submit_with_routing(
            &mut core,
            "openai",
            "gpt-4",
            ProviderRouting {
                ignore: Some(vec!["anthropic".to_string()]),
                order: Some(vec!["azure".to_string(), "openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            },
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, azure),
            other => panic!("expected Dispatched to azure, got {other:?}"),
        }
    }

    // ---- Backward compat: no routing ----

    #[test]
    fn no_routing_dispatches_normally() {
        let mut core = ProxyServerCore::new();
        let worker_id = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // submit_transport_request with None routing
        let result = core.submit_transport_request(
            "openai",
            "gpt-4",
            "/v1/chat/completions",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            None,
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, worker_id),
            other => panic!("expected normal dispatch, got {other:?}"),
        }
    }

    // ---- Wildcard proxy: endpoint prefix routing ----

    #[test]
    fn endpoint_prefix_worker_handles_matching_paths() {
        let mut core = ProxyServerCore::new();
        let embeddings_worker = core
            .register_worker(
                "openai",
                register_message_with_endpoints(
                    "embed-worker",
                    &["text-embedding-3-small"],
                    2,
                    Some(0),
                    &["/v1/embeddings"],
                ),
            )
            .worker_id;

        let result = core.submit_transport_request(
            "openai",
            "text-embedding-3-small",
            "/v1/embeddings",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            None,
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, embeddings_worker),
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    #[test]
    fn endpoint_prefix_worker_rejects_non_matching_paths() {
        let mut core = ProxyServerCore::new();
        // Worker only handles /v1/embeddings
        core.register_worker(
            "openai",
            register_message_with_endpoints(
                "embed-worker",
                &["gpt-4"],
                2,
                Some(0),
                &["/v1/embeddings"],
            ),
        );

        // Submit to /v1/chat/completions — should not match the embeddings worker
        let result = core.submit_transport_request(
            "openai",
            "gpt-4",
            "/v1/chat/completions",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            None,
        );

        assert!(
            !matches!(result, SubmissionOutcome::Dispatched(_)),
            "embeddings-only worker should not handle /v1/chat/completions"
        );
    }

    #[test]
    fn empty_endpoint_prefixes_accepts_any_path() {
        let mut core = ProxyServerCore::new();
        let worker = core
            .register_worker(
                "openai",
                register_message_with_endpoints("gpu-a", &["gpt-4"], 2, Some(0), &[]),
            )
            .worker_id;

        // No endpoint_prefixes means any path is accepted (legacy behavior)
        for path in &[
            "/v1/chat/completions",
            "/v1/embeddings",
            "/v1/models",
            "/custom/path",
        ] {
            let result = core.submit_transport_request(
                "openai",
                "gpt-4",
                *path,
                "POST",
                false,
                String::new(),
                HeaderMap::new(),
                None,
            );
            match result {
                SubmissionOutcome::Dispatched(d) => {
                    assert_eq!(d.worker_id, worker);
                    // Finish the request so the next one can dispatch
                    core.finish_request(&worker, &d.request_id);
                }
                other => panic!("expected Dispatched for path {path}, got {other:?}"),
            }
        }
    }

    #[test]
    fn routing_with_endpoint_prefix_combined() {
        let mut core = ProxyServerCore::new();

        // Embeddings worker for openai
        let openai_embed = core
            .register_worker(
                "openai",
                register_message_with_endpoints(
                    "openai-embed",
                    &["text-embedding-3-small"],
                    2,
                    Some(0),
                    &["/v1/embeddings"],
                ),
            )
            .worker_id;

        // Embeddings worker for anthropic
        let _anthropic_embed = core
            .register_worker(
                "anthropic",
                register_message_with_endpoints(
                    "anthropic-embed",
                    &["text-embedding-3-small"],
                    2,
                    Some(0),
                    &["/v1/embeddings"],
                ),
            )
            .worker_id;

        // Route to openai only, via /v1/embeddings endpoint
        let result = core.submit_transport_request(
            "openai",
            "text-embedding-3-small",
            "/v1/embeddings",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            Some(ProviderRouting {
                only: Some(vec!["openai".to_string()]),
                allow_fallbacks: Some(false),
                ..Default::default()
            }),
        );

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, openai_embed),
            other => panic!("expected Dispatched to openai embed worker, got {other:?}"),
        }
    }

    #[test]
    fn multiple_endpoint_prefixes_worker() {
        let mut core = ProxyServerCore::new();
        let worker = core
            .register_worker(
                "openai",
                register_message_with_endpoints(
                    "multi-worker",
                    &["gpt-4"],
                    4,
                    Some(0),
                    &["/v1/chat/completions", "/v1/embeddings", "/v1/models"],
                ),
            )
            .worker_id;

        for path in &["/v1/chat/completions", "/v1/embeddings", "/v1/models"] {
            let result = core.submit_transport_request(
                "openai",
                "gpt-4",
                *path,
                "POST",
                false,
                String::new(),
                HeaderMap::new(),
                None,
            );
            match result {
                SubmissionOutcome::Dispatched(d) => {
                    assert_eq!(d.worker_id, worker);
                    core.finish_request(&worker, &d.request_id);
                }
                other => panic!("expected Dispatched for path {path}, got {other:?}"),
            }
        }

        // Path not in prefixes should not match
        let result = core.submit_transport_request(
            "openai",
            "gpt-4",
            "/v1/audio/transcriptions",
            "POST",
            false,
            String::new(),
            HeaderMap::new(),
            None,
        );
        assert!(
            !matches!(result, SubmissionOutcome::Dispatched(_)),
            "/v1/audio/transcriptions should not match worker with limited prefixes"
        );
    }

    // ---- Provider Routing: empty routing struct (all fields None) ----

    #[test]
    fn empty_routing_struct_dispatches_normally() {
        let mut core = ProxyServerCore::new();
        let worker = core
            .register_worker(
                "openai",
                register_message("openai-1", &["gpt-4"], 2, Some(0)),
            )
            .worker_id;

        // ProviderRouting with all fields None should behave like no routing
        let result = submit_with_routing(&mut core, "openai", "gpt-4", ProviderRouting::default());

        match result {
            SubmissionOutcome::Dispatched(d) => assert_eq!(d.worker_id, worker),
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }
}
