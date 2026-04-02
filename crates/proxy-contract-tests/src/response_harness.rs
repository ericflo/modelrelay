use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseHarness {
    next_request_id: usize,
    active_requests: HashMap<String, ActiveRequest>,
    max_stream_bytes: usize,
}

impl ResponseHarness {
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_stream_bytes(usize::MAX)
    }

    #[must_use]
    pub fn with_max_stream_bytes(max_stream_bytes: usize) -> Self {
        Self {
            next_request_id: 1,
            active_requests: HashMap::new(),
            max_stream_bytes,
        }
    }

    pub fn start_request(&mut self, _path: &str) -> String {
        let request_id = format!("request-{}", self.next_request_id);
        self.next_request_id += 1;
        self.active_requests
            .insert(request_id.clone(), ActiveRequest::default());
        request_id
    }

    pub fn deliver_response_chunk(
        &mut self,
        request_id: &str,
        chunk: ResponseChunk,
    ) -> Option<StreamChunkDelivery> {
        let active = self.active_requests.get_mut(request_id)?;
        let next_total = active.streamed_bytes.saturating_add(chunk.data.len());

        if next_total > self.max_stream_bytes {
            self.active_requests.remove(request_id);
            return Some(StreamChunkDelivery::Terminated(
                StreamTermination::oversized(),
            ));
        }

        active.streamed_bytes = next_total;
        let forwarded = ForwardedChunk::new(
            active.forwarded_chunks.len() + 1,
            chunk.data,
            chunk.flush_immediately,
        );
        active.forwarded_chunks.push(forwarded.clone());

        Some(StreamChunkDelivery::Forwarded(forwarded))
    }

    pub fn deliver_response_complete(
        &mut self,
        request_id: &str,
        response: ResponseComplete,
    ) -> Option<PassThroughOutcome> {
        let active = self.active_requests.remove(request_id)?;

        Some(PassThroughOutcome {
            status: response.status,
            headers: response.headers,
            body: response.body,
            completion: response.completion,
            streamed_chunks: active.forwarded_chunks,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ActiveRequest {
    forwarded_chunks: Vec<ForwardedChunk>,
    streamed_bytes: usize,
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
    pub streamed_chunks: Vec<ForwardedChunk>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseChunk {
    pub data: String,
    pub flush_immediately: bool,
}

impl ResponseChunk {
    #[must_use]
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            flush_immediately: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardedChunk {
    pub sequence: usize,
    pub data: String,
    pub flushed: bool,
}

impl ForwardedChunk {
    #[must_use]
    pub fn new(sequence: usize, data: impl Into<String>, flushed: bool) -> Self {
        Self {
            sequence,
            data: data.into(),
            flushed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamChunkDelivery {
    Forwarded(ForwardedChunk),
    Terminated(StreamTermination),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamTermination {
    pub reason: StreamTerminationReason,
    pub error_sse: String,
    pub flushed: bool,
}

impl StreamTermination {
    #[must_use]
    pub fn oversized() -> Self {
        Self {
            reason: StreamTerminationReason::Oversized,
            error_sse:
                "event: error\ndata: {\"error\":{\"type\":\"stream_error\",\"message\":\"stream exceeded size limit\"}}\n\n"
                    .to_string(),
            flushed: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamTerminationReason {
    Oversized,
}
