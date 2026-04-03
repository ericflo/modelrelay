use std::collections::{HashMap, HashSet, VecDeque};

use worker_protocol::{ModelsUpdateMessage, RegisterAck, RegisterMessage};

const MAX_REQUEUE_COUNT: usize = 3;

#[derive(Debug, Default)]
pub struct ProxyServerCore {
    next_request_id: usize,
    next_worker_id: usize,
    worker_order: Vec<String>,
    workers: HashMap<String, WorkerState>,
    provider_queues: HashMap<String, VecDeque<RequestRecord>>,
    provider_queue_policies: HashMap<String, ProviderQueuePolicy>,
    selection_cursors: HashMap<SelectionKey, usize>,
    active_requests: HashMap<String, ActiveRequestState>,
    worker_cancel_signals: Vec<WorkerCancelSignal>,
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

    pub fn register_worker(
        &mut self,
        provider: impl Into<String>,
        register: RegisterMessage,
    ) -> RegisteredWorker {
        let provider = provider.into();
        let worker_id = format!("worker-{}", self.next_worker_id);
        self.next_worker_id += 1;

        let sanitized_models = sanitize_models(register.models);
        let warnings = registration_warnings(&register.worker_name, &sanitized_models);
        let current_load = register.current_load.unwrap_or(0) as usize;

        self.worker_order.push(worker_id.clone());
        self.workers.insert(
            worker_id.clone(),
            WorkerState {
                provider,
                worker_name: register.worker_name,
                models: sanitized_models.clone(),
                max_concurrent: register.max_concurrent.max(1) as usize,
                reported_load: current_load,
                in_flight_requests: Vec::new(),
            },
        );

        RegisteredWorker {
            worker_id: worker_id.clone(),
            ack: RegisterAck {
                worker_id,
                models: sanitized_models,
                warnings,
                protocol_version: register.protocol_version,
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
        worker.reported_load = update.current_load as usize;

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
        let request = RequestRecord {
            request_id: format!("request-{}", self.next_request_id),
            provider: provider.into(),
            model: model.into(),
            requeue_count: 0,
        };
        self.next_request_id += 1;
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::Queued {
                request: request.clone(),
            },
        );

        if let Some(worker_id) = self.find_eligible_worker_id(&request.provider, &request.model) {
            self.assign_to_worker(&worker_id, request.clone());
            return SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: request.request_id,
                worker_id,
            });
        }

        let queue = self
            .provider_queues
            .entry(request.provider.clone())
            .or_default();
        let policy = self
            .provider_queue_policies
            .get(&request.provider)
            .copied()
            .unwrap_or_default();

        if queue.len() >= policy.max_queue_len {
            self.active_requests.remove(&request.request_id);
            return SubmissionOutcome::Rejected(RequestFailure {
                request_id: request.request_id,
                reason: RequestFailureReason::QueueFull,
            });
        }

        queue.push_back(request.clone());
        SubmissionOutcome::Queued(QueuedAssignment {
            request_id: request.request_id,
            queue_len: queue.len(),
        })
    }

    pub fn finish_request(
        &mut self,
        worker_id: &str,
        request_id: &str,
    ) -> Option<DispatchAssignment> {
        let worker = self.workers.get_mut(worker_id)?;
        let position = worker
            .in_flight_requests
            .iter()
            .position(|active_request_id| active_request_id == request_id)?;
        worker.in_flight_requests.remove(position);
        worker.reported_load = worker.reported_load.saturating_sub(1);
        self.active_requests.remove(request_id);

        self.dispatch_next_compatible(worker_id)
    }

    pub fn disconnect_worker(&mut self, worker_id: &str) -> Option<WorkerDisconnectOutcome> {
        let worker = self.workers.remove(worker_id)?;
        self.worker_order
            .retain(|registered_worker_id| registered_worker_id != worker_id);

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
                ..
            } = active_request
            else {
                continue;
            };

            if cancellation.is_some() {
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::RequestAlreadyCanceled,
                });
                continue;
            }

            if request.requeue_count >= MAX_REQUEUE_COUNT {
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::MaxRequeuesExceeded,
                });
                continue;
            }

            request.requeue_count += 1;
            requeued_request_ids.push(request.request_id.clone());
            requeued_requests.push(request.clone());
            self.active_requests.insert(
                request.request_id.clone(),
                ActiveRequestState::Queued { request },
            );
        }

        let had_requeued_requests = !requeued_request_ids.is_empty();
        if let Some(queue) = self.provider_queues.get_mut(&worker.provider) {
            for request in requeued_requests.into_iter().rev() {
                queue.push_front(request);
            }
        } else if had_requeued_requests {
            self.provider_queues
                .insert(worker.provider, requeued_requests.into_iter().collect());
        }

        Some(WorkerDisconnectOutcome {
            requeued_request_ids,
            failed_requests,
        })
    }

    pub fn cancel_request(
        &mut self,
        request_id: &str,
        reason: CancelReason,
    ) -> Option<CancellationOutcome> {
        match self.active_requests.get(request_id)?.clone() {
            ActiveRequestState::Queued { request } => {
                let queue = self.provider_queues.get_mut(&request.provider)?;
                let index = queue
                    .iter()
                    .position(|queued_request| queued_request.request_id == request_id)?;
                queue.remove(index)?;
                self.active_requests.remove(request_id);

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
                    if let Some(ActiveRequestState::InFlight { cancellation, .. }) =
                        self.active_requests.get_mut(request_id)
                    {
                        *cancellation = Some(reason);
                    }
                    self.worker_cancel_signals.push(WorkerCancelSignal {
                        worker_id: worker_id.clone(),
                        request_id: request_id.to_string(),
                        reason,
                    });
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
    pub fn provider_models(&self, provider: &str) -> Vec<String> {
        let mut seen = HashSet::new();

        self.worker_order
            .iter()
            .filter_map(|worker_id| self.workers.get(worker_id))
            .filter(|worker| worker.provider == provider)
            .flat_map(|worker| worker.models.iter())
            .filter(|model| seen.insert((*model).clone()))
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
        let (provider, models) = {
            let worker = self.workers.get(worker_id)?;
            (worker.provider.clone(), worker.models.clone())
        };

        if !self.worker_has_capacity(worker_id) {
            return None;
        }

        let queue = self.provider_queues.get_mut(&provider)?;
        let queue_index = queue
            .iter()
            .position(|request| models.iter().any(|model| model == &request.model))?;
        let request = queue.remove(queue_index)?;

        self.assign_to_worker(worker_id, request.clone());
        Some(DispatchAssignment {
            request_id: request.request_id,
            worker_id: worker_id.to_string(),
        })
    }

    fn find_eligible_worker_id(&mut self, provider: &str, model: &str) -> Option<String> {
        let eligible_workers = self
            .worker_order
            .iter()
            .enumerate()
            .filter_map(|(position, worker_id)| {
                let worker = self.workers.get(worker_id)?;
                let supports_exact_model = worker
                    .models
                    .iter()
                    .any(|worker_model| worker_model == model);
                let selection_load = worker.selection_load();
                let has_capacity = selection_load < worker.max_concurrent;

                if worker.provider == provider && supports_exact_model && has_capacity {
                    Some((position, worker_id.clone(), selection_load))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let lowest_load = eligible_workers.iter().map(|(_, _, load)| *load).min()?;
        let tied_workers = eligible_workers
            .into_iter()
            .filter(|(_, _, load)| *load == lowest_load)
            .collect::<Vec<_>>();

        let key = SelectionKey::new(provider, model);
        let last_position = self.selection_cursors.get(&key).copied();

        let (position, worker_id, _) = tied_workers
            .iter()
            .find(|(position, _, _)| last_position.is_none_or(|last| *position > last))
            .or_else(|| tied_workers.first())?
            .clone();

        self.selection_cursors.insert(key, position);
        Some(worker_id)
    }

    fn assign_to_worker(&mut self, worker_id: &str, request: RequestRecord) {
        if let Some(worker) = self.workers.get_mut(worker_id) {
            worker.in_flight_requests.push(request.request_id.clone());
            worker.reported_load = worker.reported_load.saturating_add(1);
        }
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::InFlight {
                request,
                worker_id: worker_id.to_string(),
                cancellation: None,
            },
        );
    }

    fn worker_has_capacity(&self, worker_id: &str) -> bool {
        self.workers
            .get(worker_id)
            .is_some_and(|worker| worker.selection_load() < worker.max_concurrent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredWorker {
    pub worker_id: String,
    pub ack: RegisterAck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderQueuePolicy {
    pub max_queue_len: usize,
}

impl Default for ProviderQueuePolicy {
    fn default() -> Self {
        Self {
            max_queue_len: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Dispatched(DispatchAssignment),
    Queued(QueuedAssignment),
    Rejected(RequestFailure),
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
    QueueFull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkerState {
    provider: String,
    worker_name: String,
    models: Vec<String>,
    max_concurrent: usize,
    reported_load: usize,
    in_flight_requests: Vec<String>,
}

impl WorkerState {
    fn selection_load(&self) -> usize {
        self.reported_load.max(self.in_flight_requests.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestRecord {
    request_id: String,
    provider: String,
    model: String,
    requeue_count: usize,
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
    use worker_protocol::{ModelsUpdateMessage, RegisterMessage};

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
        core.configure_provider_queue("openai", ProviderQueuePolicy { max_queue_len: 1 });

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
            core.submit_request("openai", "mistral-large"),
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
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::Queued)
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-1".to_string()]
        );

        assert_eq!(
            core.update_worker_models(
                &second,
                ModelsUpdateMessage {
                    models: vec!["llama-3.1-70b".to_string()],
                    current_load: 0,
                }
            ),
            vec![DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: second.clone(),
            }]
        );
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
                core.update_worker_models(
                    &replacement_worker,
                    ModelsUpdateMessage {
                        models: vec!["llama-3.1-70b".to_string()],
                        current_load: 0,
                    }
                ),
                vec![DispatchAssignment {
                    request_id: "request-1".to_string(),
                    worker_id: replacement_worker.clone(),
                }]
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
}
