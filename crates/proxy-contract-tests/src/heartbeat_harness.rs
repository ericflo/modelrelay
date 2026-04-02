use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeartbeatHarness {
    next_request_id: usize,
    next_worker_id: usize,
    now_tick: usize,
    stale_after_ticks: usize,
    worker_order: Vec<String>,
    workers: HashMap<String, WorkerState>,
    selection_cursors: HashMap<SelectionKey, usize>,
}

impl HeartbeatHarness {
    #[must_use]
    pub fn new(stale_after_ticks: usize) -> Self {
        Self {
            next_request_id: 1,
            next_worker_id: 1,
            now_tick: 0,
            stale_after_ticks,
            worker_order: Vec::new(),
            workers: HashMap::new(),
            selection_cursors: HashMap::new(),
        }
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
            reported_load: 0,
            last_heartbeat_tick: self.now_tick,
        };

        self.worker_order.push(worker_id.clone());
        self.workers.insert(worker_id.clone(), worker);
        worker_id
    }

    #[must_use]
    pub fn send_ping(&self, worker_id: &str) -> Option<ServerPing> {
        self.workers.get(worker_id).map(|_| ServerPing {
            worker_id: worker_id.to_string(),
        })
    }

    pub fn receive_pong(&mut self, worker_id: &str, reported_load: usize) -> Option<PongReceipt> {
        let worker = self.workers.get_mut(worker_id)?;
        worker.reported_load = reported_load;
        worker.last_heartbeat_tick = self.now_tick;

        Some(PongReceipt {
            worker_id: worker_id.to_string(),
            reported_load,
            recorded_at_tick: self.now_tick,
        })
    }

    pub fn advance_time(&mut self, ticks: usize) {
        self.now_tick += ticks;
    }

    #[must_use]
    pub fn worker_reported_load(&self, worker_id: &str) -> Option<usize> {
        self.workers
            .get(worker_id)
            .map(|worker| worker.reported_load)
    }

    #[must_use]
    pub fn has_worker(&self, worker_id: &str) -> bool {
        self.workers.contains_key(worker_id)
    }

    pub fn expire_stale_workers(&mut self) -> Vec<ExpiredWorker> {
        let stale_worker_ids = self
            .worker_order
            .iter()
            .filter(|worker_id| self.is_stale(worker_id))
            .cloned()
            .collect::<Vec<_>>();

        stale_worker_ids
            .into_iter()
            .filter_map(|worker_id| {
                let worker = self.workers.remove(&worker_id)?;
                self.worker_order
                    .retain(|registered_worker_id| registered_worker_id != &worker_id);

                Some(ExpiredWorker {
                    worker_id,
                    last_heartbeat_tick: worker.last_heartbeat_tick,
                })
            })
            .collect()
    }

    pub fn submit_request(
        &mut self,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> SubmissionOutcome {
        let provider = provider.into();
        let model = model.into();
        let request_id = format!("request-{}", self.next_request_id);
        self.next_request_id += 1;

        let Some(worker_id) = self.find_eligible_worker_id(&provider, &model) else {
            return SubmissionOutcome::Queued(QueuedAssignment { request_id });
        };

        if let Some(worker) = self.workers.get_mut(&worker_id) {
            worker.reported_load += 1;
        }

        SubmissionOutcome::Dispatched(DispatchAssignment {
            request_id,
            worker_id,
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
                let has_capacity = worker.reported_load < worker.max_concurrent;

                if worker.provider == provider
                    && supports_model
                    && has_capacity
                    && !self.is_stale(worker_id)
                {
                    Some((position, worker_id.clone(), worker.reported_load))
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

    fn is_stale(&self, worker_id: &str) -> bool {
        let Some(worker) = self.workers.get(worker_id) else {
            return false;
        };

        self.now_tick.saturating_sub(worker.last_heartbeat_tick) >= self.stale_after_ticks
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerState {
    provider: String,
    models: Vec<String>,
    max_concurrent: usize,
    reported_load: usize,
    last_heartbeat_tick: usize,
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
pub struct ServerPing {
    pub worker_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PongReceipt {
    pub worker_id: String,
    pub reported_load: usize,
    pub recorded_at_tick: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpiredWorker {
    pub worker_id: String,
    pub last_heartbeat_tick: usize,
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
}
