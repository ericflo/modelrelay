use std::collections::{HashMap, HashSet, VecDeque};

const MAX_REQUEUE_COUNT: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchHarness {
    next_request_id: usize,
    next_worker_id: usize,
    now_tick: usize,
    worker_order: Vec<String>,
    workers: HashMap<String, WorkerState>,
    provider_queues: HashMap<String, VecDeque<RequestRecord>>,
    provider_policies: HashMap<String, ProviderQueuePolicy>,
    selection_cursors: HashMap<SelectionKey, usize>,
    active_requests: HashMap<String, ActiveRequestState>,
    canceled_requests: HashSet<String>,
    worker_cancel_signals: Vec<WorkerCancelSignal>,
    forwarded_chunks: HashMap<String, Vec<ForwardedChunk>>,
}

impl Default for DispatchHarness {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            next_worker_id: 1,
            now_tick: 0,
            worker_order: Vec::new(),
            workers: HashMap::new(),
            provider_queues: HashMap::new(),
            provider_policies: HashMap::new(),
            selection_cursors: HashMap::new(),
            active_requests: HashMap::new(),
            canceled_requests: HashSet::new(),
            worker_cancel_signals: Vec::new(),
            forwarded_chunks: HashMap::new(),
        }
    }
}

impl DispatchHarness {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure_provider_queue(
        &mut self,
        provider: impl Into<String>,
        policy: ProviderQueuePolicy,
    ) {
        self.provider_policies.insert(provider.into(), policy);
    }

    pub fn register_worker(
        &mut self,
        provider: impl Into<String>,
        models: impl IntoIterator<Item = impl Into<String>>,
        max_concurrent: usize,
    ) -> String {
        let worker_id = format!("worker-{}", self.next_worker_id);
        self.next_worker_id += 1;

        let worker = WorkerState {
            provider: provider.into(),
            models: models.into_iter().map(Into::into).collect(),
            max_concurrent: max_concurrent.max(1),
            in_flight_requests: Vec::new(),
            is_draining: false,
            graceful_shutdown_deadline_tick: None,
        };

        self.worker_order.push(worker_id.clone());
        self.workers.insert(worker_id.clone(), worker);
        worker_id
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
            queued_at_tick: self.now_tick,
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

        let provider_queue = self
            .provider_queues
            .entry(request.provider.clone())
            .or_default();
        let provider_policy = self
            .provider_policies
            .get(&request.provider)
            .copied()
            .unwrap_or_default();

        if provider_queue.len() >= provider_policy.max_queue_len {
            self.active_requests.remove(&request.request_id);
            return SubmissionOutcome::Rejected(RequestFailure {
                request_id: request.request_id,
                reason: RequestFailureReason::QueueFull,
            });
        }

        provider_queue.push_back(request.clone());

        SubmissionOutcome::Queued(QueuedAssignment {
            request_id: request.request_id,
            queue_len: provider_queue.len(),
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
            worker.is_draining && worker.in_flight_requests.is_empty()
        };
        self.active_requests.remove(request_id);

        if should_disconnect_after_finish {
            self.remove_worker(worker_id);
            return None;
        }

        self.dispatch_next_compatible(worker_id)
    }

    #[must_use]
    pub fn begin_graceful_shutdown(
        &mut self,
        timeout_ticks: usize,
    ) -> Vec<GracefulShutdownSignal> {
        let disconnect_deadline_tick = self.now_tick + timeout_ticks;
        let worker_ids = self.worker_order.clone();
        let mut signals = Vec::new();

        for worker_id in worker_ids {
            let Some(worker) = self.workers.get_mut(&worker_id) else {
                continue;
            };
            worker.is_draining = true;
            worker.graceful_shutdown_deadline_tick = Some(disconnect_deadline_tick);
            signals.push(GracefulShutdownSignal {
                worker_id,
                disconnect_deadline_tick,
            });
        }

        signals
    }

    #[must_use]
    pub fn expire_graceful_shutdown(&mut self) -> GracefulShutdownOutcome {
        let expiring_workers = self
            .worker_order
            .iter()
            .filter_map(|worker_id| {
                let worker = self.workers.get(worker_id)?;
                let deadline = worker.graceful_shutdown_deadline_tick?;

                (deadline <= self.now_tick).then(|| worker_id.clone())
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
                failed_requests.push(RequestFailure {
                    request_id: request_id.clone(),
                    reason: RequestFailureReason::GracefulShutdownTimedOut,
                });
            }

            self.remove_worker(&worker_id);
            disconnected_worker_ids.push(worker_id);
        }

        GracefulShutdownOutcome {
            disconnected_worker_ids,
            failed_requests,
        }
    }

    #[must_use]
    pub fn delete_provider(&mut self, provider: &str) -> ProviderDeletionOutcome {
        let mut failed_requests = self
            .provider_queues
            .remove(provider)
            .into_iter()
            .flat_map(|queue| queue.into_iter())
            .map(|request| {
                self.active_requests.remove(&request.request_id);
                RequestFailure {
                    request_id: request.request_id,
                    reason: RequestFailureReason::ProviderDeleted,
                }
            })
            .collect::<Vec<_>>();

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

        for worker_id in &worker_ids {
            let Some(worker) = self.workers.get(worker_id).cloned() else {
                continue;
            };

            for request_id in worker.in_flight_requests {
                self.active_requests.remove(&request_id);
                failed_requests.push(RequestFailure {
                    request_id,
                    reason: RequestFailureReason::ProviderDeleted,
                });
            }

            self.remove_worker(worker_id);
        }

        ProviderDeletionOutcome {
            disconnected_worker_ids: worker_ids,
            failed_requests,
        }
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

            let ActiveRequestState::InFlight { mut request, .. } = active_request else {
                continue;
            };

            if self.canceled_requests.contains(&request_id) {
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

        let had_requeued_requests = !requeued_requests.is_empty();
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
                self.canceled_requests.insert(request_id.to_string());

                Some(CancellationOutcome::RemovedFromQueue {
                    request_id: request_id.to_string(),
                })
            }
            ActiveRequestState::InFlight { worker_id, .. } => {
                self.canceled_requests.insert(request_id.to_string());
                let signal = WorkerCancelSignal {
                    worker_id,
                    request_id: request_id.to_string(),
                    reason,
                };
                self.worker_cancel_signals.push(signal.clone());

                Some(CancellationOutcome::WorkerCancelSent(signal))
            }
        }
    }

    pub fn deliver_worker_chunk(
        &mut self,
        request_id: &str,
        chunk: impl Into<String>,
    ) -> Option<ChunkDelivery> {
        let _request_state = self.active_requests.get(request_id)?;

        if self.canceled_requests.contains(request_id) {
            return Some(ChunkDelivery::DroppedAfterCancellation);
        }

        let forwarded = ForwardedChunk {
            request_id: request_id.to_string(),
            data: chunk.into(),
        };
        self.forwarded_chunks
            .entry(request_id.to_string())
            .or_default()
            .push(forwarded.clone());

        Some(ChunkDelivery::Forwarded(forwarded))
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
    pub fn worker_cancel_signals(&self) -> Vec<WorkerCancelSignal> {
        self.worker_cancel_signals.clone()
    }

    #[must_use]
    pub fn forwarded_chunks(&self, request_id: &str) -> Vec<ForwardedChunk> {
        self.forwarded_chunks
            .get(request_id)
            .cloned()
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
            .map(|worker| worker.is_draining)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn dispatch_next_for_worker(&mut self, worker_id: &str) -> Option<DispatchAssignment> {
        self.dispatch_next_compatible(worker_id)
    }

    pub fn advance_time(&mut self, ticks: usize) {
        self.now_tick += ticks;
    }

    #[must_use]
    pub fn expire_queue_timeouts(&mut self) -> Vec<RequestFailure> {
        let mut failures = Vec::new();

        for (provider, queue) in &mut self.provider_queues {
            let queue_timeout_ticks = self
                .provider_policies
                .get(provider)
                .copied()
                .unwrap_or_default()
                .queue_timeout_ticks;

            if let Some(timeout_ticks) = queue_timeout_ticks {
                let mut retained_queue = VecDeque::with_capacity(queue.len());

                while let Some(request) = queue.pop_front() {
                    if self.now_tick.saturating_sub(request.queued_at_tick) >= timeout_ticks {
                        self.active_requests.remove(&request.request_id);
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
        }

        failures
    }

    fn dispatch_next_compatible(&mut self, worker_id: &str) -> Option<DispatchAssignment> {
        let (provider, models, has_capacity) = {
            let worker = self.workers.get(worker_id)?;
            (
                worker.provider.clone(),
                worker.models.clone(),
                !worker.is_draining && worker.in_flight_requests.len() < worker.max_concurrent,
            )
        };

        if !has_capacity {
            return None;
        }

        let provider_queue = self.provider_queues.get_mut(&provider)?;
        let queue_index = provider_queue
            .iter()
            .position(|request| models.iter().any(|model| model == &request.model))?;
        let request = provider_queue.remove(queue_index)?;

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
                let supports_model = worker
                    .models
                    .iter()
                    .any(|advertised_model| advertised_model == model);
                let has_capacity =
                    !worker.is_draining && worker.in_flight_requests.len() < worker.max_concurrent;

                if worker.provider == provider && supports_model && has_capacity {
                    Some((position, worker_id.clone(), worker.in_flight_requests.len()))
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
        }
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::InFlight {
                request,
                worker_id: worker_id.to_string(),
            },
        );
    }

    fn remove_worker(&mut self, worker_id: &str) {
        self.workers.remove(worker_id);
        self.worker_order
            .retain(|registered_worker_id| registered_worker_id != worker_id);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerState {
    provider: String,
    models: Vec<String>,
    max_concurrent: usize,
    in_flight_requests: Vec<String>,
    is_draining: bool,
    graceful_shutdown_deadline_tick: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestRecord {
    request_id: String,
    provider: String,
    model: String,
    queued_at_tick: usize,
    requeue_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderQueuePolicy {
    pub max_queue_len: usize,
    pub queue_timeout_ticks: Option<usize>,
}

impl Default for ProviderQueuePolicy {
    fn default() -> Self {
        Self {
            max_queue_len: usize::MAX,
            queue_timeout_ticks: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveRequestState {
    Queued {
        request: RequestRecord,
    },
    InFlight {
        request: RequestRecord,
        worker_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Dispatched(DispatchAssignment),
    Queued(QueuedAssignment),
    Rejected(RequestFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchAssignment {
    pub request_id: String,
    pub worker_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedAssignment {
    pub request_id: String,
    pub queue_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    ClientDisconnected,
    RequestTimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancellationOutcome {
    RemovedFromQueue { request_id: String },
    WorkerCancelSent(WorkerCancelSignal),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerCancelSignal {
    pub worker_id: String,
    pub request_id: String,
    pub reason: CancelReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerDisconnectOutcome {
    pub requeued_request_ids: Vec<String>,
    pub failed_requests: Vec<RequestFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GracefulShutdownSignal {
    pub worker_id: String,
    pub disconnect_deadline_tick: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GracefulShutdownOutcome {
    pub disconnected_worker_ids: Vec<String>,
    pub failed_requests: Vec<RequestFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDeletionOutcome {
    pub disconnected_worker_ids: Vec<String>,
    pub failed_requests: Vec<RequestFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestFailure {
    pub request_id: String,
    pub reason: RequestFailureReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestFailureReason {
    RequestAlreadyCanceled,
    MaxRequeuesExceeded,
    QueueTimedOut,
    QueueFull,
    GracefulShutdownTimedOut,
    ProviderDeleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkDelivery {
    Forwarded(ForwardedChunk),
    DroppedAfterCancellation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardedChunk {
    pub request_id: String,
    pub data: String,
}
