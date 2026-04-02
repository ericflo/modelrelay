use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchHarness {
    next_request_id: usize,
    next_worker_id: usize,
    worker_order: Vec<String>,
    workers: HashMap<String, WorkerState>,
    provider_queues: HashMap<String, VecDeque<QueuedRequest>>,
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
            worker_order: Vec::new(),
            workers: HashMap::new(),
            provider_queues: HashMap::new(),
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
        let request = QueuedRequest {
            request_id: format!("request-{}", self.next_request_id),
            provider: provider.into(),
            model: model.into(),
        };
        self.next_request_id += 1;
        self.active_requests.insert(
            request.request_id.clone(),
            ActiveRequestState::Queued {
                provider: request.provider.clone(),
            },
        );

        if let Some(worker_id) = self.find_eligible_worker_id(&request.provider, &request.model) {
            self.assign_to_worker(&worker_id, &request.request_id);
            return SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: request.request_id,
                worker_id,
            });
        }

        let provider_queue = self
            .provider_queues
            .entry(request.provider.clone())
            .or_default();
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
        let worker = self.workers.get_mut(worker_id)?;
        let position = worker
            .in_flight_requests
            .iter()
            .position(|active_request_id| active_request_id == request_id)?;
        worker.in_flight_requests.remove(position);
        self.active_requests.remove(request_id);

        self.dispatch_next_compatible(worker_id)
    }

    pub fn cancel_request(
        &mut self,
        request_id: &str,
        reason: CancelReason,
    ) -> Option<CancellationOutcome> {
        match self.active_requests.get(request_id)?.clone() {
            ActiveRequestState::Queued { provider } => {
                let queue = self.provider_queues.get_mut(&provider)?;
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
            ActiveRequestState::InFlight { worker_id } => {
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

    fn dispatch_next_compatible(&mut self, worker_id: &str) -> Option<DispatchAssignment> {
        let (provider, models, has_capacity) = {
            let worker = self.workers.get(worker_id)?;
            (
                worker.provider.clone(),
                worker.models.clone(),
                worker.in_flight_requests.len() < worker.max_concurrent,
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

        self.assign_to_worker(worker_id, &request.request_id);

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
                let has_capacity = worker.in_flight_requests.len() < worker.max_concurrent;

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

    fn assign_to_worker(&mut self, worker_id: &str, request_id: &str) {
        if let Some(worker) = self.workers.get_mut(worker_id) {
            worker.in_flight_requests.push(request_id.to_string());
        }
        self.active_requests.insert(
            request_id.to_string(),
            ActiveRequestState::InFlight {
                worker_id: worker_id.to_string(),
            },
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerState {
    provider: String,
    models: Vec<String>,
    max_concurrent: usize,
    in_flight_requests: Vec<String>,
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
struct QueuedRequest {
    request_id: String,
    provider: String,
    model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveRequestState {
    Queued { provider: String },
    InFlight { worker_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Dispatched(DispatchAssignment),
    Queued(QueuedAssignment),
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
pub enum ChunkDelivery {
    Forwarded(ForwardedChunk),
    DroppedAfterCancellation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardedChunk {
    pub request_id: String,
    pub data: String,
}
