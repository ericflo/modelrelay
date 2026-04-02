pub mod dispatch_harness;
pub mod registration_harness;
pub mod response_harness;

/// Returns the pinned Katamari commit used as the starting behavior contract.
#[must_use]
pub fn source_behavior_commit() -> &'static str {
    "ab5e90f6a2ff05a063663ce478146bf0b6829429"
}

#[cfg(test)]
mod tests {
    use super::source_behavior_commit;
    use crate::dispatch_harness::{
        CancelReason, CancellationOutcome, ChunkDelivery, DispatchAssignment, DispatchHarness,
        QueuedAssignment, SubmissionOutcome, WorkerCancelSignal,
    };
    use crate::registration_harness::{
        CloseCode, ConnectRequest, HandshakeFailure, ProviderConfig, RegisterAck, RegisterMessage,
        RegistrationHarness, ServerToWorker, WorkerToServer,
    };
    use crate::response_harness::{
        CompletionMetadata, ForwardedChunk, PassThroughOutcome, ResponseChunk, ResponseComplete,
        ResponseHarness, ResponseHeader, StreamChunkDelivery, StreamTermination,
    };

    #[test]
    fn pins_the_behavior_source_commit() {
        assert_eq!(
            source_behavior_commit(),
            "ab5e90f6a2ff05a063663ce478146bf0b6829429"
        );
    }

    #[test]
    fn behavior_contract_doc_mentions_next_characterization_tests() {
        let doc = include_str!("../../../docs/behavior-contract.md");

        assert!(doc.contains("First Characterization Tests To Write Next"));
        assert!(doc.contains("Immediate dispatch vs queue"));
        assert!(doc.contains("Worker registration handshake"));
        assert!(doc.contains("Worker disconnect handling"));
        assert!(!doc.contains("4. Client cancellation:"));
        assert!(!doc.contains("4. Streaming pass-through:"));
    }

    #[test]
    fn request_for_supported_model_dispatches_immediately_without_queueing() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let outcome = harness.submit_request("openai", "llama-3.1-70b");

        assert_eq!(
            outcome,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            })
        );
        assert!(harness.queued_request_ids("openai").is_empty());
        assert_eq!(
            harness.worker_in_flight_request_ids(&worker_id),
            vec!["request-1".to_string()]
        );
    }

    #[test]
    fn request_queues_when_all_matching_workers_are_at_capacity() {
        let mut harness = DispatchHarness::new();
        let llama_worker = harness.register_worker("openai", ["llama-3.1-70b"], 1);
        let _mistral_worker = harness.register_worker("openai", ["mistral-large"], 1);

        let first = harness.submit_request("openai", "llama-3.1-70b");
        let second = harness.submit_request("openai", "llama-3.1-70b");

        assert_eq!(
            first,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: llama_worker.clone(),
            })
        );
        assert_eq!(
            second,
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
        assert_eq!(
            harness.worker_in_flight_request_ids(&llama_worker),
            vec!["request-1".to_string()]
        );
    }

    #[test]
    fn selection_prefers_the_lowest_load_among_exact_model_matches() {
        let mut harness = DispatchHarness::new();
        let first_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let second_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);

        let first = harness.submit_request("openai", "llama-3.1-70b");
        let second = harness.submit_request("openai", "llama-3.1-70b");

        assert_eq!(
            first,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: first_worker.clone(),
            })
        );
        assert_eq!(
            second,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-2".to_string(),
                worker_id: second_worker.clone(),
            })
        );
        assert_eq!(
            harness.worker_in_flight_request_ids(&first_worker),
            vec!["request-1".to_string()]
        );
        assert_eq!(
            harness.worker_in_flight_request_ids(&second_worker),
            vec!["request-2".to_string()]
        );
    }

    #[test]
    fn equally_loaded_exact_matches_rotate_in_round_robin_order() {
        let mut harness = DispatchHarness::new();
        let first_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let second_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let third_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);

        let outcomes = [
            harness.submit_request("openai", "llama-3.1-70b"),
            harness.submit_request("openai", "llama-3.1-70b"),
            harness.submit_request("openai", "llama-3.1-70b"),
            harness.submit_request("openai", "llama-3.1-70b"),
        ];

        assert_eq!(
            outcomes,
            [
                SubmissionOutcome::Dispatched(DispatchAssignment {
                    request_id: "request-1".to_string(),
                    worker_id: first_worker.clone(),
                }),
                SubmissionOutcome::Dispatched(DispatchAssignment {
                    request_id: "request-2".to_string(),
                    worker_id: second_worker.clone(),
                }),
                SubmissionOutcome::Dispatched(DispatchAssignment {
                    request_id: "request-3".to_string(),
                    worker_id: third_worker.clone(),
                }),
                SubmissionOutcome::Dispatched(DispatchAssignment {
                    request_id: "request-4".to_string(),
                    worker_id: first_worker.clone(),
                }),
            ]
        );
    }

    #[test]
    fn idle_workers_without_the_exact_requested_model_remain_ineligible() {
        let mut harness = DispatchHarness::new();
        let exact_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let mismatched_worker = harness.register_worker("openai", ["llama-3.1-70b-q4"], 2);

        let first = harness.submit_request("openai", "llama-3.1-70b");
        let second = harness.submit_request("openai", "llama-3.1-70b");

        assert_eq!(
            first,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: exact_worker.clone(),
            })
        );
        assert_eq!(
            second,
            SubmissionOutcome::Dispatched(DispatchAssignment {
                request_id: "request-2".to_string(),
                worker_id: exact_worker.clone(),
            })
        );
        assert_eq!(
            harness.worker_in_flight_request_ids(&mismatched_worker),
            Vec::<String>::new()
        );
    }

    #[test]
    fn workers_without_the_exact_requested_model_do_not_receive_the_request() {
        let mut harness = DispatchHarness::new();
        let mismatched_worker = harness.register_worker("openai", ["llama-3.1-70b-q4"], 1);

        let outcome = harness.submit_request("openai", "llama-3.1-70b");

        assert_eq!(
            outcome,
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-1".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            harness.worker_in_flight_request_ids(&mismatched_worker),
            Vec::<String>::new()
        );
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-1".to_string()]
        );
    }

    #[test]
    fn queued_requests_are_fifo_within_a_provider_among_compatible_models() {
        let mut harness = DispatchHarness::new();
        let llama_worker = harness.register_worker("openai", ["llama-3.1-70b"], 1);
        let mistral_worker = harness.register_worker("openai", ["mistral-large"], 1);

        let initial_llama = harness.submit_request("openai", "llama-3.1-70b");
        let initial_mistral = harness.submit_request("openai", "mistral-large");
        let queued_mistral = harness.submit_request("openai", "mistral-large");
        let queued_llama = harness.submit_request("openai", "llama-3.1-70b");
        let queued_mistral_tail = harness.submit_request("openai", "mistral-large");

        assert!(matches!(initial_llama, SubmissionOutcome::Dispatched(_)));
        assert!(matches!(initial_mistral, SubmissionOutcome::Dispatched(_)));
        assert!(matches!(queued_mistral, SubmissionOutcome::Queued(_)));
        assert!(matches!(queued_llama, SubmissionOutcome::Queued(_)));
        assert!(matches!(queued_mistral_tail, SubmissionOutcome::Queued(_)));
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec![
                "request-3".to_string(),
                "request-4".to_string(),
                "request-5".to_string(),
            ]
        );

        let next_for_llama = harness
            .finish_request(&llama_worker, "request-1")
            .expect("llama worker should take the earliest compatible queued request");
        let next_for_mistral = harness
            .finish_request(&mistral_worker, "request-2")
            .expect("mistral worker should take the earliest compatible queued request");

        assert_eq!(
            next_for_llama,
            DispatchAssignment {
                request_id: "request-4".to_string(),
                worker_id: llama_worker.clone(),
            }
        );
        assert_eq!(
            next_for_mistral,
            DispatchAssignment {
                request_id: "request-3".to_string(),
                worker_id: mistral_worker.clone(),
            }
        );
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-5".to_string()]
        );
    }

    #[test]
    fn canceling_a_queued_request_removes_it_before_dispatch() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let first = harness.submit_request("openai", "llama-3.1-70b");
        let queued = harness.submit_request("openai", "llama-3.1-70b");

        assert!(matches!(first, SubmissionOutcome::Dispatched(_)));
        assert_eq!(
            queued,
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );

        let cancellation = harness
            .cancel_request("request-2", CancelReason::ClientDisconnected)
            .expect("queued request should be canceled");

        assert_eq!(
            cancellation,
            CancellationOutcome::RemovedFromQueue {
                request_id: "request-2".to_string(),
            }
        );
        assert!(harness.queued_request_ids("openai").is_empty());
        assert_eq!(
            harness.finish_request(&worker_id, "request-1"),
            None,
            "the canceled queued request must not dispatch after capacity frees up"
        );
    }

    #[test]
    fn canceling_an_in_flight_request_emits_a_worker_cancel_signal() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let dispatched = harness.submit_request("openai", "llama-3.1-70b");

        assert!(matches!(dispatched, SubmissionOutcome::Dispatched(_)));

        let cancellation = harness
            .cancel_request("request-1", CancelReason::ClientDisconnected)
            .expect("in-flight request should emit a cancel signal");

        assert_eq!(
            cancellation,
            CancellationOutcome::WorkerCancelSent(WorkerCancelSignal {
                worker_id: worker_id.clone(),
                request_id: "request-1".to_string(),
                reason: CancelReason::ClientDisconnected,
            })
        );
        assert_eq!(
            harness.worker_cancel_signals(),
            vec![WorkerCancelSignal {
                worker_id,
                request_id: "request-1".to_string(),
                reason: CancelReason::ClientDisconnected,
            }]
        );
    }

    #[test]
    fn late_worker_chunks_are_dropped_after_the_request_is_canceled() {
        let mut harness = DispatchHarness::new();
        harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let dispatched = harness.submit_request("openai", "llama-3.1-70b");

        assert!(matches!(dispatched, SubmissionOutcome::Dispatched(_)));

        let cancellation = harness
            .cancel_request("request-1", CancelReason::RequestTimedOut)
            .expect("active request should still accept timeout cancellation");

        assert!(matches!(
            cancellation,
            CancellationOutcome::WorkerCancelSent(_)
        ));
        assert_eq!(
            harness.deliver_worker_chunk("request-1", "data: late-token\n\n"),
            Some(ChunkDelivery::DroppedAfterCancellation)
        );
        assert!(
            harness.forwarded_chunks("request-1").is_empty(),
            "late worker output should be ignored once the client has canceled"
        );
    }

    #[test]
    fn worker_can_authenticate_and_receive_a_sanitized_register_ack() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let session = harness
            .connect(ConnectRequest::with_header_secret("openai", "top-secret"))
            .expect("worker should authenticate");

        let worker_message = serde_json::to_string(&WorkerToServer::Register(RegisterMessage {
            worker_name: "  edge-box-01-with-an-overly-long-suffix  ".to_string(),
            models: vec![
                " llama-3.1-70b ".to_string(),
                String::new(),
                "llama-3.1-70b".to_string(),
                " mistral-large ".to_string(),
                "qwen2.5-coder".to_string(),
                "phi-4".to_string(),
                "too-many".to_string(),
            ],
            max_concurrent: 0,
            protocol_version: Some("katamari-worker-v1".to_string()),
        }))
        .expect("register message should encode");

        let server_message = session
            .exchange_text(&worker_message)
            .expect("register should be accepted");

        let ServerToWorker::RegisterAck(ack) =
            serde_json::from_str::<ServerToWorker>(&server_message)
                .expect("register ack should decode");

        assert_eq!(
            ack,
            RegisterAck {
                worker_id: "worker-1".to_string(),
                worker_name: "edge-box-01-with-an-overly-long-".to_string(),
                models: vec![
                    "llama-3.1-70b".to_string(),
                    "mistral-large".to_string(),
                    "qwen2.5-coder".to_string(),
                    "phi-4".to_string(),
                ],
                max_concurrent: 1,
                protocol_version: "katamari-worker-v1".to_string(),
                warnings: vec![
                    "worker_name truncated to 32 characters".to_string(),
                    "model list truncated to 4 entries".to_string(),
                ],
            }
        );
    }

    #[test]
    fn legacy_query_secret_can_authenticate_but_wrong_secret_is_rejected() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let session = harness
            .connect(ConnectRequest::with_query_secret("openai", "top-secret"))
            .expect("query-string fallback should still work");

        let ack = session
            .exchange_text(
                &serde_json::to_string(&WorkerToServer::Register(RegisterMessage {
                    worker_name: "gpu-box".to_string(),
                    models: vec!["llama-3.1-8b".to_string()],
                    max_concurrent: 2,
                    protocol_version: None,
                }))
                .expect("register message should encode"),
            )
            .expect("legacy auth path should still permit registration");

        let ack = serde_json::from_str::<ServerToWorker>(&ack).expect("register ack should decode");
        assert!(matches!(ack, ServerToWorker::RegisterAck(_)));

        let failure = harness
            .connect(ConnectRequest::with_header_secret("openai", "wrong-secret"))
            .expect_err("bad secret should be rejected");

        assert_eq!(
            failure,
            HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "worker authentication failed".to_string(),
            }
        );
    }

    #[test]
    fn protocol_version_mismatch_is_rejected_before_registration_completes() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let session = harness
            .connect(ConnectRequest::with_header_secret("openai", "top-secret"))
            .expect("worker should authenticate");

        let failure = session
            .exchange_text(
                &serde_json::to_string(&WorkerToServer::Register(RegisterMessage {
                    worker_name: "gpu-box".to_string(),
                    models: vec!["llama-3.1-8b".to_string()],
                    max_concurrent: 2,
                    protocol_version: Some("katamari-worker-v2".to_string()),
                }))
                .expect("register message should encode"),
            )
            .expect_err("mismatched protocol version should fail");

        assert_eq!(
            failure,
            HandshakeFailure {
                code: CloseCode::ProtocolError,
                reason: "unsupported protocol version `katamari-worker-v2`; expected `katamari-worker-v1`".to_string(),
            }
        );
    }

    #[test]
    fn unknown_or_disabled_providers_are_rejected_before_register() {
        let mut harness = RegistrationHarness::new([
            ("openai", ProviderConfig::enabled("top-secret")),
            ("anthropic", ProviderConfig::disabled("other-secret")),
        ]);

        let unknown = harness
            .connect(ConnectRequest::with_header_secret("missing", "top-secret"))
            .expect_err("unknown provider should fail");
        assert_eq!(
            unknown,
            HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "unknown provider `missing`".to_string(),
            }
        );

        let disabled = harness
            .connect(ConnectRequest::with_header_secret(
                "anthropic",
                "other-secret",
            ))
            .expect_err("disabled provider should fail");
        assert_eq!(
            disabled,
            HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "provider `anthropic` is disabled".to_string(),
            }
        );
    }

    #[test]
    fn response_complete_preserves_status_headers_body_and_token_counts_for_success() {
        let mut harness = ResponseHarness::new();
        let request_id = harness.start_request("/v1/chat/completions");

        let delivered = harness
            .deliver_response_complete(
                &request_id,
                ResponseComplete::new(
                    200,
                    vec![
                        ResponseHeader::new("content-type", "application/json"),
                        ResponseHeader::new("x-request-id", "req_123"),
                        ResponseHeader::new("set-cookie", "route=a"),
                        ResponseHeader::new("set-cookie", "worker=box-01"),
                    ],
                    r#"{"id":"chatcmpl-123","choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#,
                )
                .with_token_counts(12, 5, 17),
            )
            .expect("response_complete should resolve the client response");

        assert_eq!(
            delivered,
            PassThroughOutcome {
                status: 200,
                headers: vec![
                    ResponseHeader::new("content-type", "application/json"),
                    ResponseHeader::new("x-request-id", "req_123"),
                    ResponseHeader::new("set-cookie", "route=a"),
                    ResponseHeader::new("set-cookie", "worker=box-01"),
                ],
                body: r#"{"id":"chatcmpl-123","choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#.to_string(),
                completion: Some(CompletionMetadata::new(12, 5, 17)),
                streamed_chunks: Vec::new(),
            }
        );
    }

    #[test]
    fn response_complete_preserves_upstream_error_body_without_flattening_headers() {
        let mut harness = ResponseHarness::new();
        let request_id = harness.start_request("/v1/responses");

        let delivered = harness
            .deliver_response_complete(
                &request_id,
                ResponseComplete::new(
                    503,
                    vec![
                        ResponseHeader::new("content-type", "application/json"),
                        ResponseHeader::new("retry-after", "15"),
                        ResponseHeader::new("x-upstream-status", "overloaded"),
                    ],
                    r#"{"error":{"type":"server_error","message":"backend overloaded"}}"#,
                )
                .with_token_counts(321, 0, 321),
            )
            .expect("upstream errors should still produce a client response");

        assert_eq!(delivered.status, 503);
        assert_eq!(
            delivered.headers,
            vec![
                ResponseHeader::new("content-type", "application/json"),
                ResponseHeader::new("retry-after", "15"),
                ResponseHeader::new("x-upstream-status", "overloaded"),
            ]
        );
        assert_eq!(
            delivered.body,
            r#"{"error":{"type":"server_error","message":"backend overloaded"}}"#
        );
        assert_eq!(
            delivered.completion,
            Some(CompletionMetadata::new(321, 0, 321))
        );
        assert!(delivered.streamed_chunks.is_empty());
        assert!(
            !delivered.body.contains("prompt_tokens"),
            "token counts stay in completion metadata rather than mutating the client-visible body"
        );
    }

    #[test]
    fn streaming_pass_through_preserves_chunk_order_done_termination_and_completion_metadata() {
        let mut harness = ResponseHarness::new();
        let request_id = harness.start_request("/v1/chat/completions");

        let first = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"Hel"}}]}\n\n"#,
            ),
        );
        let second = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"lo"}}]}\n\n"#,
            ),
        );
        let done =
            harness.deliver_response_chunk(&request_id, ResponseChunk::new("data: [DONE]\n\n"));

        assert_eq!(
            first,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                1,
                r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"Hel"}}]}\n\n"#,
                true,
            )))
        );
        assert_eq!(
            second,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                2,
                r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"lo"}}]}\n\n"#,
                true,
            )))
        );
        assert_eq!(
            done,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                3,
                "data: [DONE]\n\n",
                true,
            )))
        );

        let delivered = harness
            .deliver_response_complete(
                &request_id,
                ResponseComplete::new(
                    200,
                    vec![
                        ResponseHeader::new("content-type", "text/event-stream"),
                        ResponseHeader::new("cache-control", "no-cache"),
                    ],
                    "",
                )
                .with_token_counts(12, 2, 14),
            )
            .expect("stream completion metadata should resolve the response");

        assert_eq!(delivered.status, 200);
        assert_eq!(
            delivered.headers,
            vec![
                ResponseHeader::new("content-type", "text/event-stream"),
                ResponseHeader::new("cache-control", "no-cache"),
            ]
        );
        assert_eq!(delivered.body, "");
        assert_eq!(
            delivered.streamed_chunks,
            vec![
                ForwardedChunk::new(
                    1,
                    r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"Hel"}}]}\n\n"#,
                    true,
                ),
                ForwardedChunk::new(
                    2,
                    r#"data: {"id":"chatcmpl-123","choices":[{"delta":{"content":"lo"}}]}\n\n"#,
                    true,
                ),
                ForwardedChunk::new(3, "data: [DONE]\n\n", true),
            ]
        );
        assert_eq!(
            delivered.completion,
            Some(CompletionMetadata::new(12, 2, 14))
        );
    }

    #[test]
    fn oversized_stream_emits_sse_error_and_terminates_without_delivering_completion() {
        let first_chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n";
        let oversized_chunk = "data: {\"choices\":[{\"delta\":{\"content\":\"this pushes the stream over the configured limit\"}}]}\n\n";
        let mut harness = ResponseHarness::with_max_stream_bytes(first_chunk.len() + 8);
        let request_id = harness.start_request("/v1/chat/completions");

        let first = harness.deliver_response_chunk(&request_id, ResponseChunk::new(first_chunk));
        let terminated =
            harness.deliver_response_chunk(&request_id, ResponseChunk::new(oversized_chunk));

        assert_eq!(
            first,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                1,
                first_chunk,
                true,
            )))
        );
        assert_eq!(
            terminated,
            Some(StreamChunkDelivery::Terminated(
                StreamTermination::oversized()
            ))
        );
        assert_eq!(
            harness.deliver_response_complete(
                &request_id,
                ResponseComplete::new(
                    200,
                    vec![ResponseHeader::new("content-type", "text/event-stream")],
                    "",
                )
                .with_token_counts(12, 99, 111),
            ),
            None,
            "oversized streams terminate before late completion metadata is delivered"
        );
        assert_eq!(
            harness.deliver_response_chunk(&request_id, ResponseChunk::new("data: [DONE]\n\n")),
            None,
            "late chunks after oversized termination are dropped"
        );
    }
}
