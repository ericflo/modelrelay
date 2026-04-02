use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHarness {
    next_request_id: usize,
    next_worker_id: usize,
    workers: HashMap<String, WorkerState>,
    active_requests: HashMap<String, ActiveRequest>,
}

impl Default for ResponseHarness {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            next_worker_id: 1,
            workers: HashMap::new(),
            active_requests: HashMap::new(),
        }
    }
}

impl ResponseHarness {
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

        self.workers.insert(
            worker_id.clone(),
            WorkerState {
                provider: provider.into(),
                models: models.into_iter().map(Into::into).collect(),
                max_concurrent: max_concurrent.max(1),
                in_flight_requests: Vec::new(),
            },
        );

        worker_id
    }

    /// Dispatches a non-streaming request to an eligible worker.
    ///
    /// # Panics
    ///
    /// Panics when no registered worker matches the provider, model, and capacity
    /// needed for the request. The characterization tests use this harness only in
    /// scenarios where dispatch eligibility is part of the setup.
    #[must_use]
    pub fn submit_request(&mut self, request: NonStreamingRequest) -> DispatchAssignment {
        let request_id = format!("request-{}", self.next_request_id);
        self.next_request_id += 1;

        let worker_id = self
            .workers
            .iter()
            .find_map(|(worker_id, worker)| {
                let supports_model = worker.models.iter().any(|model| model == &request.model);
                let has_capacity = worker.in_flight_requests.len() < worker.max_concurrent;

                if worker.provider == request.provider && supports_model && has_capacity {
                    Some(worker_id.clone())
                } else {
                    None
                }
            })
            .expect("test harness requires an eligible worker for submitted requests");

        self.workers
            .get_mut(&worker_id)
            .expect("worker should exist")
            .in_flight_requests
            .push(request_id.clone());

        self.active_requests.insert(
            request_id.clone(),
            ActiveRequest {
                worker_id: worker_id.clone(),
                request,
            },
        );

        DispatchAssignment {
            request_id,
            worker_id,
        }
    }

    pub fn complete_response(
        &mut self,
        worker_id: &str,
        request_id: &str,
        completion: ResponseComplete,
    ) -> Option<CompletionOutcome> {
        let active_request = self.active_requests.remove(request_id)?;
        if active_request.worker_id != worker_id {
            return None;
        }

        let worker = self.workers.get_mut(worker_id)?;
        let request_position = worker
            .in_flight_requests
            .iter()
            .position(|active_request_id| active_request_id == request_id)?;
        worker.in_flight_requests.remove(request_position);

        Some(CompletionOutcome::new(
            completion.status,
            completion.headers,
            completion.body,
            completion.token_counts,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerState {
    provider: String,
    models: Vec<String>,
    max_concurrent: usize,
    in_flight_requests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveRequest {
    worker_id: String,
    request: NonStreamingRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonStreamingRequest {
    pub provider: String,
    pub model: String,
    pub path: String,
    pub body: String,
}

impl NonStreamingRequest {
    #[must_use]
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        path: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            path: path.into(),
            body: body.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchAssignment {
    pub request_id: String,
    pub worker_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseComplete {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: String,
    pub token_counts: TokenCounts,
}

impl ResponseComplete {
    #[must_use]
    pub fn new(
        status: u16,
        headers: impl IntoIterator<Item = Header>,
        body: impl Into<String>,
        token_counts: TokenCounts,
    ) -> Self {
        Self {
            status,
            headers: headers.into_iter().collect(),
            body: body.into(),
            token_counts,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionOutcome {
    pub client_response: ClientResponse,
    pub completion: CompletionMetadata,
}

impl CompletionOutcome {
    #[must_use]
    pub fn new(
        status: u16,
        headers: Vec<Header>,
        body: impl Into<String>,
        token_counts: TokenCounts,
    ) -> Self {
        let body = body.into();

        Self {
            client_response: ClientResponse {
                status,
                headers,
                body,
            },
            completion: CompletionMetadata { token_counts },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionMetadata {
    pub token_counts: TokenCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenCounts {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl TokenCounts {
    #[must_use]
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
        }
    }
}
