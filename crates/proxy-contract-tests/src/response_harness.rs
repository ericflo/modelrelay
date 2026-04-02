use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseHarness {
    next_request_id: usize,
    active_request_ids: HashSet<String>,
}

impl ResponseHarness {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            active_request_ids: HashSet::new(),
        }
    }

    pub fn start_request(&mut self, _path: &str) -> String {
        let request_id = format!("request-{}", self.next_request_id);
        self.next_request_id += 1;
        self.active_request_ids.insert(request_id.clone());
        request_id
    }

    pub fn deliver_response_complete(
        &mut self,
        request_id: &str,
        response: ResponseComplete,
    ) -> Option<PassThroughOutcome> {
        if !self.active_request_ids.remove(request_id) {
            return None;
        }

        Some(PassThroughOutcome {
            status: response.status,
            headers: response.headers,
            body: response.body,
            completion: response.completion,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseComplete {
    pub status: u16,
    pub headers: Vec<ResponseHeader>,
    pub body: String,
    pub completion: Option<CompletionMetadata>,
}

impl ResponseComplete {
    #[must_use]
    pub fn new(status: u16, headers: Vec<ResponseHeader>, body: impl Into<String>) -> Self {
        Self {
            status,
            headers,
            body: body.into(),
            completion: None,
        }
    }

    #[must_use]
    pub fn with_token_counts(
        mut self,
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    ) -> Self {
        self.completion = Some(CompletionMetadata::new(
            prompt_tokens,
            completion_tokens,
            total_tokens,
        ));
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassThroughOutcome {
    pub status: u16,
    pub headers: Vec<ResponseHeader>,
    pub body: String,
    pub completion: Option<CompletionMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHeader {
    pub name: String,
    pub value: String,
}

impl ResponseHeader {
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionMetadata {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl CompletionMetadata {
    #[must_use]
    pub fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    }
}
