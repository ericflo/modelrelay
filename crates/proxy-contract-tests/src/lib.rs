pub mod dispatch_harness;
pub mod heartbeat_harness;
pub mod registration_harness;
pub mod response_harness;

/// Returns the pinned Katamari commit used as the starting behavior contract.
#[must_use]
pub fn source_behavior_commit() -> &'static str {
    "ab5e90f6a2ff05a063663ce478146bf0b6829429"
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};

    use serde_json::{Value, json};

    use super::source_behavior_commit;
    use crate::dispatch_harness::{
        CancelReason, CancellationOutcome, ChunkDelivery, DispatchAssignment, DispatchHarness,
        GracefulShutdownOutcome, GracefulShutdownSignal, ProviderDeletionOutcome,
        ProviderQueuePolicy, QueuedAssignment, RequestFailure, RequestFailureReason,
        SubmissionOutcome, WorkerCancelSignal, WorkerDisconnectOutcome,
    };
    use crate::heartbeat_harness::{
        DispatchAssignment as HeartbeatDispatchAssignment, ExpiredWorker, HeartbeatHarness,
        PongReceipt, QueuedAssignment as HeartbeatQueuedAssignment, ServerPing,
        SubmissionOutcome as HeartbeatSubmissionOutcome,
    };
    use crate::registration_harness::{
        CloseCode, ConnectRequest, HandshakeFailure, ProviderConfig, RegisterAck, RegisterMessage,
        RegistrationHarness, ServerToWorker, WorkerToServer,
    };
    use crate::response_harness::{
        CompletionMetadata, ForwardedChunk, PassThroughOutcome, ResponseChunk, ResponseComplete,
        ResponseHarness, ResponseHeader, StreamChunkDelivery, StreamTermination,
    };

    fn serialize_register_message(register: RegisterMessage) -> String {
        serde_json::to_string(&WorkerToServer::Register(register))
            .expect("register payload should serialize")
    }

    fn parse_server_message(message: &str) -> ServerToWorker {
        serde_json::from_str(message).expect("server message should deserialize")
    }

    fn parse_register_ack(message: &str) -> RegisterAck {
        let ServerToWorker::RegisterAck(ack) = parse_server_message(message);
        ack
    }

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
        assert!(doc.contains("Registration sanitization edge warnings"));
        assert!(doc.contains("truncated worker names"));
        assert!(doc.contains("max_concurrent"));
        assert!(!doc.contains("Dynamic model catalog updates and `/v1/models` coherence"));
        assert!(!doc.contains("1. OpenAI-style and Anthropic-style compatibility:"));
        assert!(!doc.contains("1. Graceful shutdown and drain:"));
        assert!(!doc.contains("5. Queue timeout and queue-full surfaces:"));
        assert!(!doc.contains("4. Client cancellation:"));
        assert!(!doc.contains("4. Streaming pass-through:"));
        assert!(!doc.contains("OpenAI `/v1/responses` compatibility coverage"));
    }

    #[test]
    fn graceful_shutdown_stops_new_assignments_and_disconnects_after_in_flight_completion() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        assert!(matches!(
            harness.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert_eq!(
            harness.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );

        assert_eq!(
            harness.begin_graceful_shutdown(5),
            vec![GracefulShutdownSignal {
                worker_id: worker_id.clone(),
                disconnect_deadline_tick: 5,
            }]
        );
        assert!(harness.worker_is_draining(&worker_id));

        assert_eq!(
            harness.finish_request(&worker_id, "request-1"),
            None,
            "a draining worker should disconnect instead of taking more queued work"
        );
        assert!(!harness.has_worker(&worker_id));
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
    }

    #[test]
    fn graceful_shutdown_times_out_in_flight_work_that_does_not_finish_before_deadline() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        assert!(matches!(
            harness.submit_request("openai", "llama-3.1-70b"),
            SubmissionOutcome::Dispatched(_)
        ));
        assert_eq!(
            harness.begin_graceful_shutdown(3),
            vec![GracefulShutdownSignal {
                worker_id: worker_id.clone(),
                disconnect_deadline_tick: 3,
            }]
        );

        harness.advance_time(3);

        assert_eq!(
            harness.expire_graceful_shutdown(),
            GracefulShutdownOutcome {
                disconnected_worker_ids: vec![worker_id.clone()],
                failed_requests: vec![RequestFailure {
                    request_id: "request-1".to_string(),
                    reason: RequestFailureReason::GracefulShutdownTimedOut,
                }],
            }
        );
        assert!(!harness.has_worker(&worker_id));
    }

    #[test]
    fn provider_deletion_drains_queued_requests_with_explicit_errors_and_closes_workers() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        assert_eq!(
            harness.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-1".to_string(),
                queue_len: 1,
            })
        );

        assert_eq!(
            harness.delete_provider("openai"),
            ProviderDeletionOutcome {
                disconnected_worker_ids: vec![worker_id.clone()],
                failed_requests: vec![RequestFailure {
                    request_id: "request-1".to_string(),
                    reason: RequestFailureReason::ProviderDeleted,
                }],
            }
        );
        assert!(harness.queued_request_ids("openai").is_empty());
        assert!(!harness.has_worker(&worker_id));
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
    fn models_update_immediately_changes_routing_without_worker_reconnect() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        assert_eq!(
            harness.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-1".to_string(),
                queue_len: 1,
            })
        );

        assert_eq!(
            harness.update_worker_models(&worker_id, ["llama-3.1-70b", "mistral-large"]),
            vec![DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: worker_id.clone(),
            }]
        );
        assert!(harness.queued_request_ids("openai").is_empty());
        assert_eq!(
            harness.worker_in_flight_request_ids(&worker_id),
            vec!["request-1".to_string()]
        );

        assert_eq!(harness.finish_request(&worker_id, "request-1"), None);
        assert!(harness.has_worker(&worker_id));

        assert!(
            harness
                .update_worker_models(&worker_id, ["llama-3.1-70b"])
                .is_empty()
        );
        assert_eq!(
            harness.submit_request("openai", "mistral-large"),
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
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
    fn pong_updates_live_load_used_for_selection() {
        let mut harness = HeartbeatHarness::new(3);
        let first_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let second_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);

        assert_eq!(
            harness.send_ping(&first_worker),
            Some(ServerPing {
                worker_id: first_worker.clone(),
            })
        );
        assert_eq!(
            harness.send_ping(&second_worker),
            Some(ServerPing {
                worker_id: second_worker.clone(),
            })
        );

        assert_eq!(
            harness.receive_pong(&first_worker, 1),
            Some(PongReceipt {
                worker_id: first_worker.clone(),
                reported_load: 1,
                recorded_at_tick: 0,
            })
        );
        assert_eq!(
            harness.receive_pong(&second_worker, 0),
            Some(PongReceipt {
                worker_id: second_worker.clone(),
                reported_load: 0,
                recorded_at_tick: 0,
            })
        );
        assert_eq!(harness.worker_reported_load(&first_worker), Some(1));
        assert_eq!(harness.worker_reported_load(&second_worker), Some(0));

        assert_eq!(
            harness.submit_request("openai", "llama-3.1-70b"),
            HeartbeatSubmissionOutcome::Dispatched(HeartbeatDispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: second_worker,
            })
        );
    }

    #[test]
    fn stale_workers_are_expired_after_the_pong_window() {
        let mut harness = HeartbeatHarness::new(3);
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        harness.advance_time(2);
        let _fresh = harness.receive_pong(&worker_id, 0);

        harness.advance_time(3);

        assert_eq!(
            harness.expire_stale_workers(),
            vec![ExpiredWorker {
                worker_id: worker_id.clone(),
                last_heartbeat_tick: 2,
            }]
        );
        assert!(!harness.has_worker(&worker_id));
    }

    #[test]
    fn stale_workers_are_not_selected_for_new_requests() {
        let mut harness = HeartbeatHarness::new(3);
        let stale_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);
        let fresh_worker = harness.register_worker("openai", ["llama-3.1-70b"], 2);

        assert!(harness.receive_pong(&stale_worker, 0).is_some());
        assert!(harness.receive_pong(&fresh_worker, 1).is_some());

        harness.advance_time(3);
        assert!(harness.receive_pong(&fresh_worker, 1).is_some());

        assert_eq!(
            harness.submit_request("openai", "llama-3.1-70b"),
            HeartbeatSubmissionOutcome::Dispatched(HeartbeatDispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: fresh_worker.clone(),
            })
        );

        let mut only_stale_harness = HeartbeatHarness::new(3);
        let only_worker = only_stale_harness.register_worker("openai", ["llama-3.1-70b"], 1);
        assert!(only_stale_harness.receive_pong(&only_worker, 0).is_some());
        only_stale_harness.advance_time(3);

        assert_eq!(
            only_stale_harness.submit_request("openai", "llama-3.1-70b"),
            HeartbeatSubmissionOutcome::Queued(HeartbeatQueuedAssignment {
                request_id: "request-1".to_string(),
            })
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
    fn queued_request_times_out_after_waiting_for_worker_capacity() {
        let mut harness = DispatchHarness::new();
        harness.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 2,
                queue_timeout_ticks: Some(5),
            },
        );
        harness.register_worker("openai", ["llama-3.1-70b"], 1);

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

        harness.advance_time(5);

        assert_eq!(
            harness.expire_queue_timeouts(),
            vec![RequestFailure {
                request_id: "request-2".to_string(),
                reason: RequestFailureReason::QueueTimedOut,
            }]
        );
        assert!(
            harness.queued_request_ids("openai").is_empty(),
            "a timed-out queued request must leave the provider queue"
        );
    }

    #[test]
    fn queue_capacity_exhaustion_rejects_without_waiting_for_timeout() {
        let mut harness = DispatchHarness::new();
        harness.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 1,
                queue_timeout_ticks: Some(30),
            },
        );
        harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let first = harness.submit_request("openai", "llama-3.1-70b");
        let queued = harness.submit_request("openai", "llama-3.1-70b");
        let rejected = harness.submit_request("openai", "llama-3.1-70b");

        assert!(matches!(first, SubmissionOutcome::Dispatched(_)));
        assert_eq!(
            queued,
            SubmissionOutcome::Queued(QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            rejected,
            SubmissionOutcome::Rejected(RequestFailure {
                request_id: "request-3".to_string(),
                reason: RequestFailureReason::QueueFull,
            })
        );
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-2".to_string()],
            "queue-full rejection must not displace or mutate already queued work"
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
            .expect("in-flight request should be canceled");

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

        assert_eq!(
            harness.deliver_worker_chunk("request-1", "data: first-token\n\n"),
            Some(ChunkDelivery::Forwarded(
                crate::dispatch_harness::ForwardedChunk {
                    request_id: "request-1".to_string(),
                    data: "data: first-token\n\n".to_string(),
                }
            ))
        );
        assert!(
            harness
                .cancel_request("request-1", CancelReason::ClientDisconnected)
                .is_some()
        );
        assert_eq!(
            harness.deliver_worker_chunk("request-1", "data: late-token\n\n"),
            Some(ChunkDelivery::DroppedAfterCancellation),
            "late chunks after cancellation should be dropped instead of forwarded"
        );
        assert_eq!(
            harness.forwarded_chunks("request-1"),
            vec![crate::dispatch_harness::ForwardedChunk {
                request_id: "request-1".to_string(),
                data: "data: first-token\n\n".to_string(),
            }]
        );
    }

    #[test]
    fn disconnecting_a_worker_requeues_a_live_in_flight_request() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let dispatched = harness.submit_request("openai", "llama-3.1-70b");
        assert!(matches!(dispatched, SubmissionOutcome::Dispatched(_)));

        assert_eq!(
            harness.disconnect_worker(&worker_id),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: vec!["request-1".to_string()],
                failed_requests: vec![],
            })
        );
        assert_eq!(
            harness.queued_request_ids("openai"),
            vec!["request-1".to_string()]
        );
        assert!(!harness.has_worker(&worker_id));
    }

    #[test]
    fn disconnecting_a_worker_does_not_requeue_a_request_after_it_was_canceled() {
        let mut harness = DispatchHarness::new();
        let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let dispatched = harness.submit_request("openai", "llama-3.1-70b");
        assert!(matches!(dispatched, SubmissionOutcome::Dispatched(_)));
        assert!(
            harness
                .cancel_request("request-1", CancelReason::ClientDisconnected)
                .is_some()
        );

        assert_eq!(
            harness.disconnect_worker(&worker_id),
            Some(WorkerDisconnectOutcome {
                requeued_request_ids: vec![],
                failed_requests: vec![RequestFailure {
                    request_id: "request-1".to_string(),
                    reason: RequestFailureReason::RequestAlreadyCanceled,
                }],
            })
        );
        assert!(
            harness.queued_request_ids("openai").is_empty(),
            "a canceled request should fail immediately instead of requeueing on worker disconnect"
        );
    }

    #[test]
    fn repeated_worker_disconnects_stop_requeueing_after_the_max_attempts() {
        let mut harness = DispatchHarness::new();
        let mut active_worker = harness.register_worker("openai", ["llama-3.1-70b"], 1);

        let dispatched = harness.submit_request("openai", "llama-3.1-70b");
        assert!(matches!(dispatched, SubmissionOutcome::Dispatched(_)));

        for _ in 0..3 {
            let disconnected = harness
                .disconnect_worker(&active_worker)
                .expect("worker should disconnect while request is in flight");
            assert_eq!(
                disconnected.requeued_request_ids,
                vec!["request-1".to_string()]
            );
            assert!(disconnected.failed_requests.is_empty());

            let worker_id = harness.register_worker("openai", ["llama-3.1-70b"], 1);
            let redispatched = harness.dispatch_next_for_worker(&worker_id);
            assert_eq!(
                redispatched,
                Some(DispatchAssignment {
                    request_id: "request-1".to_string(),
                    worker_id: worker_id.clone(),
                })
            );
            active_worker = worker_id;
        }

        let redispatched = harness.dispatch_next_for_worker(&active_worker);
        assert_eq!(
            redispatched, None,
            "after three requeues the request should already be in flight on the replacement worker"
        );

        let exhausted = harness
            .disconnect_worker(&active_worker)
            .expect("final worker should disconnect while request is in flight");
        assert_eq!(exhausted.requeued_request_ids, Vec::<String>::new());
        assert_eq!(
            exhausted.failed_requests,
            vec![RequestFailure {
                request_id: "request-1".to_string(),
                reason: RequestFailureReason::MaxRequeuesExceeded,
            }]
        );
        assert!(
            harness.queued_request_ids("openai").is_empty(),
            "requeue exhaustion should fail the request instead of leaving it queued"
        );
    }

    #[test]
    fn worker_can_authenticate_and_receive_a_sanitized_register_ack() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let connection = harness
            .connect(ConnectRequest::with_header_secret("openai", "top-secret"))
            .expect("worker should authenticate");

        let registration = RegisterMessage {
            worker_name: "gpu-box-a".to_string(),
            models: vec![
                " llama-3.1-70b ".to_string(),
                "llama-3.1-70b".to_string(),
                " ".to_string(),
                "mistral-large".to_string(),
            ],
            max_concurrent: 3,
            protocol_version: None,
        };

        let ack_text = connection
            .exchange_text(&serialize_register_message(registration))
            .expect("registration should succeed");
        let ack = parse_register_ack(&ack_text);

        assert_eq!(
            ack,
            RegisterAck {
                worker_id: "worker-1".to_string(),
                worker_name: "gpu-box-a".to_string(),
                models: vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()],
                max_concurrent: 3,
                warnings: Vec::new(),
                protocol_version: "katamari-worker-v1".to_string(),
            }
        );
        assert_eq!(
            parse_server_message(&ack_text),
            ServerToWorker::RegisterAck(ack)
        );
    }

    #[test]
    fn legacy_query_secret_can_authenticate_but_wrong_secret_is_rejected() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let accepted = harness
            .connect(ConnectRequest::with_query_secret("openai", "top-secret"))
            .expect("legacy query secret should still work for backward compatibility");
        let ack = parse_register_ack(
            &accepted
                .exchange_text(&serialize_register_message(RegisterMessage {
                    worker_name: "gpu-box-a".to_string(),
                    models: vec!["llama-3.1-70b".to_string()],
                    max_concurrent: 1,
                    protocol_version: None,
                }))
                .expect("legacy-authenticated worker should complete registration"),
        );
        assert_eq!(ack.worker_id, "worker-1".to_string());

        let rejected =
            harness.connect(ConnectRequest::with_header_secret("openai", "wrong-secret"));
        assert_eq!(
            rejected,
            Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "worker authentication failed".to_string(),
            })
        );
    }

    #[test]
    fn repeated_failed_auth_attempts_are_rate_limited_by_client_identity() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let repeated_bad_secret = || {
            ConnectRequest::with_header_secret("openai", "wrong-secret")
                .with_client_identity("198.51.100.24")
        };

        for _ in 0..3 {
            assert_eq!(
                harness.connect(repeated_bad_secret()),
                Err(HandshakeFailure {
                    code: CloseCode::PolicyViolation,
                    reason: "worker authentication failed".to_string(),
                })
            );
        }

        assert_eq!(
            harness.connect(repeated_bad_secret()),
            Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "worker authentication rate limited for client `198.51.100.24`".to_string(),
            })
        );

        let other_client = harness
            .connect(
                ConnectRequest::with_header_secret("openai", "top-secret")
                    .with_client_identity("203.0.113.10"),
            )
            .expect("rate limit should be scoped to the failing client identity");
        assert_eq!(
            parse_register_ack(
                &other_client
                    .exchange_text(&serialize_register_message(RegisterMessage {
                        worker_name: "gpu-box-b".to_string(),
                        models: vec!["llama-3.1-70b".to_string()],
                        max_concurrent: 1,
                        protocol_version: None,
                    }))
                    .expect("other client should finish registration"),
            )
            .worker_id,
            "worker-1".to_string()
        );
    }

    #[test]
    fn failed_auth_rate_limit_entries_expire_and_allow_a_later_valid_connection() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let rate_limited_client = || {
            ConnectRequest::with_header_secret("openai", "wrong-secret")
                .with_client_identity("198.51.100.24")
        };

        for _ in 0..3 {
            assert_eq!(
                harness.connect(rate_limited_client()),
                Err(HandshakeFailure {
                    code: CloseCode::PolicyViolation,
                    reason: "worker authentication failed".to_string(),
                })
            );
        }
        assert_eq!(
            harness.connect(rate_limited_client()),
            Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "worker authentication rate limited for client `198.51.100.24`".to_string(),
            })
        );

        harness.advance_time(5);

        let accepted = harness
            .connect(
                ConnectRequest::with_header_secret("openai", "top-secret")
                    .with_client_identity("198.51.100.24"),
            )
            .expect("expired failed-auth limiter should allow a later valid connection");
        assert_eq!(
            parse_register_ack(
                &accepted
                    .exchange_text(&serialize_register_message(RegisterMessage {
                        worker_name: "gpu-box-a".to_string(),
                        models: vec!["llama-3.1-70b".to_string()],
                        max_concurrent: 1,
                        protocol_version: None,
                    }))
                    .expect("valid worker should complete registration after limiter expiry"),
            )
            .worker_id,
            "worker-1".to_string()
        );
    }

    #[test]
    fn protocol_version_mismatch_is_rejected_before_registration_completes() {
        let mut harness =
            RegistrationHarness::new([("openai", ProviderConfig::enabled("top-secret"))]);

        let connection = harness
            .connect(ConnectRequest::with_header_secret("openai", "top-secret"))
            .expect("worker should authenticate");

        let rejected = connection.exchange_text(&serialize_register_message(RegisterMessage {
            worker_name: "gpu-box-a".to_string(),
            models: vec!["llama-3.1-70b".to_string()],
            max_concurrent: 1,
            protocol_version: Some("katamari-pre-release".to_string()),
        }));

        assert_eq!(
            rejected,
            Err(HandshakeFailure {
                code: CloseCode::ProtocolError,
                reason: "unsupported protocol version `katamari-pre-release`; expected `katamari-worker-v1`".to_string(),
            })
        );
    }

    #[test]
    fn unknown_or_disabled_providers_are_rejected_before_register() {
        let mut harness = RegistrationHarness::new([
            ("openai", ProviderConfig::enabled("top-secret")),
            ("anthropic", ProviderConfig::disabled("other-secret")),
        ]);

        let unknown = harness.connect(ConnectRequest::with_header_secret("mystery", "top-secret"));
        assert_eq!(
            unknown,
            Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "unknown provider `mystery`".to_string(),
            })
        );

        let disabled = harness.connect(ConnectRequest::with_header_secret(
            "anthropic",
            "other-secret",
        ));
        assert_eq!(
            disabled,
            Err(HandshakeFailure {
                code: CloseCode::PolicyViolation,
                reason: "provider `anthropic` is disabled".to_string(),
            })
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
                        ResponseHeader::new("x-upstream-id", "cmp_123"),
                    ],
                    r#"{"id":"chatcmpl-123","choices":[{"message":{"content":"hello"}}]}"#,
                )
                .with_token_counts(42, 7, 49),
            )
            .expect("response should complete");

        assert_eq!(
            delivered,
            PassThroughOutcome {
                status: 200,
                headers: vec![
                    ResponseHeader::new("content-type", "application/json"),
                    ResponseHeader::new("x-upstream-id", "cmp_123"),
                ],
                body: r#"{"id":"chatcmpl-123","choices":[{"message":{"content":"hello"}}]}"#
                    .to_string(),
                streamed_chunks: Vec::new(),
                completion: Some(CompletionMetadata::new(42, 7, 49)),
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
            .expect("error response should still complete");

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
                r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"#,
            ),
        );
        let second = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"#,
            ),
        );
        let done =
            harness.deliver_response_chunk(&request_id, ResponseChunk::new("data: [DONE]\n\n"));

        assert_eq!(
            first,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                1,
                r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"#,
                true,
            )))
        );
        assert_eq!(
            second,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                2,
                r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"#,
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
                    r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"#,
                    true,
                ),
                ForwardedChunk::new(
                    2,
                    r#"data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"#,
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

    #[test]
    fn openai_chat_completions_http_boundary_preserves_model_stream_flag_and_body() {
        let body = json!({
            "model": "llama-3.1-70b",
            "messages": [
                {"role": "system", "content": "You are terse."},
                {"role": "user", "content": "say hi"}
            ],
            "stream": true
        })
        .to_string();

        let forwarded =
            HttpCompatibilityHarness::parse_client_request("/v1/chat/completions", &body)
                .expect("OpenAI-style request should parse");

        assert_eq!(
            forwarded,
            ForwardedHttpRequest {
                path: "/v1/chat/completions".to_string(),
                model: "llama-3.1-70b".to_string(),
                is_streaming: true,
                raw_body: body,
            }
        );
    }

    #[test]
    fn anthropic_messages_http_boundary_preserves_model_stream_flag_and_body() {
        let body = json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 256,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            ],
            "stream": true
        })
        .to_string();

        let forwarded = HttpCompatibilityHarness::parse_client_request("/v1/messages", &body)
            .expect("Anthropic-style request should parse");

        assert_eq!(
            forwarded,
            ForwardedHttpRequest {
                path: "/v1/messages".to_string(),
                model: "claude-3-7-sonnet".to_string(),
                is_streaming: true,
                raw_body: body,
            }
        );
    }

    #[test]
    fn openai_responses_worker_request_preserves_exact_envelope_and_compatibility_headers() {
        let body = json!({
            "model": "gpt-4.1-mini",
            "input": [{"role": "user", "content": "hello"}],
            "stream": true,
            "metadata": {"trace_id": "trace-123"}
        })
        .to_string();

        let forwarded = HttpCompatibilityHarness::forward_worker_request(
            "/v1/responses",
            &body,
            [
                ("authorization", "Bearer sk-openai"),
                ("content-type", "application/json"),
                ("openai-organization", "org_123"),
                ("user-agent", "codex-test"),
            ],
        )
        .expect("OpenAI-style request should forward to a worker envelope");

        assert_eq!(
            forwarded,
            ForwardedWorkerRequest {
                path: "/v1/responses".to_string(),
                model: "gpt-4.1-mini".to_string(),
                is_streaming: true,
                raw_body: body,
                headers: BTreeMap::from([
                    ("authorization".to_string(), "Bearer sk-openai".to_string()),
                    ("content-type".to_string(), "application/json".to_string()),
                    ("openai-organization".to_string(), "org_123".to_string()),
                ]),
            }
        );
    }

    #[test]
    fn anthropic_messages_worker_request_preserves_exact_envelope_and_compatibility_headers() {
        let body = json!({
            "model": "claude-3-7-sonnet",
            "max_tokens": 256,
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            ],
            "stream": true
        })
        .to_string();

        let forwarded = HttpCompatibilityHarness::forward_worker_request(
            "/v1/messages",
            &body,
            [
                ("x-api-key", "anthropic-key"),
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "tools-2024-04-04"),
                ("content-type", "application/json"),
                ("user-agent", "claude-code"),
            ],
        )
        .expect("Anthropic-style request should forward to a worker envelope");

        assert_eq!(
            forwarded,
            ForwardedWorkerRequest {
                path: "/v1/messages".to_string(),
                model: "claude-3-7-sonnet".to_string(),
                is_streaming: true,
                raw_body: body,
                headers: BTreeMap::from([
                    ("anthropic-beta".to_string(), "tools-2024-04-04".to_string()),
                    ("anthropic-version".to_string(), "2023-06-01".to_string()),
                    ("content-type".to_string(), "application/json".to_string()),
                    ("x-api-key".to_string(), "anthropic-key".to_string()),
                ]),
            }
        );
    }

    #[test]
    fn models_endpoint_returns_openai_compatible_catalog_shape_for_advertised_models() {
        let harness =
            HttpCompatibilityHarness::new(["llama-3.1-70b", "mistral-large", "llama-3.1-70b"]);

        let response = harness.models_response();

        assert_eq!(response.status, 200);
        assert_eq!(
            response.body,
            json!({
                "object": "list",
                "data": [
                    {
                        "id": "llama-3.1-70b",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    },
                    {
                        "id": "mistral-large",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    }
                ]
            })
        );
    }

    #[test]
    fn models_endpoint_tracks_live_worker_model_updates_without_stale_entries() {
        let mut dispatch = DispatchHarness::new();
        let first_worker = dispatch.register_worker("openai", ["llama-3.1-70b"], 1);
        let second_worker = dispatch.register_worker("openai", ["mistral-large"], 1);

        let initial =
            HttpCompatibilityHarness::new(dispatch.provider_models("openai")).models_response();
        assert_eq!(
            initial.body,
            json!({
                "object": "list",
                "data": [
                    {
                        "id": "llama-3.1-70b",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    },
                    {
                        "id": "mistral-large",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    }
                ]
            })
        );

        assert!(
            dispatch
                .update_worker_models(&first_worker, ["llama-3.1-70b", "gpt-oss-120b"])
                .is_empty()
        );
        assert!(
            dispatch
                .update_worker_models(&second_worker, ["gpt-oss-120b"])
                .is_empty()
        );

        let updated =
            HttpCompatibilityHarness::new(dispatch.provider_models("openai")).models_response();
        assert_eq!(
            updated.body,
            json!({
                "object": "list",
                "data": [
                    {
                        "id": "llama-3.1-70b",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    },
                    {
                        "id": "gpt-oss-120b",
                        "object": "model",
                        "owned_by": "worker-proxy"
                    }
                ]
            })
        );
    }

    #[test]
    fn openai_streaming_http_boundary_preserves_sse_shape_and_done_marker() {
        let mut harness = ResponseHarness::new();
        let request_id = harness.start_request("/v1/chat/completions");

        let first = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            ),
        );
        let second = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            ),
        );
        let done =
            harness.deliver_response_chunk(&request_id, ResponseChunk::new("data: [DONE]\n\n"));

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
                ),
            )
            .expect("stream completion should close the OpenAI SSE response");

        assert_eq!(
            first,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                1,
                "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
                true,
            )))
        );
        assert_eq!(
            second,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                2,
                "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
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
                    "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
                    true,
                ),
                ForwardedChunk::new(
                    2,
                    "data: {\"id\":\"chatcmpl-123\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                    true,
                ),
                ForwardedChunk::new(3, "data: [DONE]\n\n", true),
            ]
        );
    }

    #[test]
    fn anthropic_streaming_http_boundary_preserves_event_sse_shape() {
        let mut harness = ResponseHarness::new();
        let request_id = harness.start_request("/v1/messages");

        let start = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
            ),
        );
        let delta = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            ),
        );
        let stop = harness.deliver_response_chunk(
            &request_id,
            ResponseChunk::new("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        );

        let delivered = harness
            .deliver_response_complete(
                &request_id,
                ResponseComplete::new(
                    200,
                    vec![ResponseHeader::new("content-type", "text/event-stream")],
                    "",
                ),
            )
            .expect("stream completion should close the Anthropic SSE response");

        assert_eq!(
            start,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                1,
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
                true,
            )))
        );
        assert_eq!(
            delta,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                2,
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
                true,
            )))
        );
        assert_eq!(
            stop,
            Some(StreamChunkDelivery::Forwarded(ForwardedChunk::new(
                3,
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                true,
            )))
        );
        assert_eq!(delivered.status, 200);
        assert_eq!(
            delivered.headers,
            vec![ResponseHeader::new("content-type", "text/event-stream")]
        );
        assert_eq!(delivered.body, "");
        assert_eq!(
            delivered.streamed_chunks,
            vec![
                ForwardedChunk::new(
                    1,
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-3-7-sonnet\"}}\n\n",
                    true,
                ),
                ForwardedChunk::new(
                    2,
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
                    true,
                ),
                ForwardedChunk::new(
                    3,
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                    true,
                ),
            ]
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HttpCompatibilityHarness {
        advertised_models: Vec<String>,
    }

    impl HttpCompatibilityHarness {
        fn new(models: impl IntoIterator<Item = impl Into<String>>) -> Self {
            Self {
                advertised_models: models.into_iter().map(Into::into).collect(),
            }
        }

        fn parse_client_request(
            path: &str,
            body: &str,
        ) -> Result<ForwardedHttpRequest, CompatibilityParseError> {
            let payload: Value = serde_json::from_str(body)
                .map_err(|error| CompatibilityParseError::InvalidJson(error.to_string()))?;

            match path {
                "/v1/chat/completions" | "/v1/responses" | "/v1/messages" => {
                    let model = payload
                        .get("model")
                        .and_then(Value::as_str)
                        .ok_or(CompatibilityParseError::MissingModel)?;
                    let is_streaming = payload
                        .get("stream")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);

                    Ok(ForwardedHttpRequest {
                        path: path.to_string(),
                        model: model.to_string(),
                        is_streaming,
                        raw_body: body.to_string(),
                    })
                }
                unsupported => Err(CompatibilityParseError::UnsupportedPath(
                    unsupported.to_string(),
                )),
            }
        }

        fn forward_worker_request(
            path: &str,
            body: &str,
            headers: impl IntoIterator<Item = (&'static str, &'static str)>,
        ) -> Result<ForwardedWorkerRequest, CompatibilityParseError> {
            let parsed = Self::parse_client_request(path, body)?;
            let allowlist = match path {
                "/v1/chat/completions" | "/v1/responses" => {
                    &["authorization", "content-type", "openai-organization"][..]
                }
                "/v1/messages" => &[
                    "x-api-key",
                    "anthropic-version",
                    "anthropic-beta",
                    "content-type",
                ][..],
                unsupported => {
                    return Err(CompatibilityParseError::UnsupportedPath(
                        unsupported.to_string(),
                    ));
                }
            };

            let headers = headers
                .into_iter()
                .filter_map(|(name, value)| {
                    let normalized = name.to_ascii_lowercase();
                    allowlist
                        .contains(&normalized.as_str())
                        .then(|| (normalized, value.to_string()))
                })
                .collect();

            Ok(ForwardedWorkerRequest {
                path: parsed.path,
                model: parsed.model,
                is_streaming: parsed.is_streaming,
                raw_body: parsed.raw_body,
                headers,
            })
        }

        fn models_response(&self) -> ModelsEndpointResponse {
            let mut seen = HashSet::new();
            let models = self
                .advertised_models
                .iter()
                .filter(|model| seen.insert((*model).clone()))
                .map(|model| {
                    json!({
                        "id": model,
                        "object": "model",
                        "owned_by": "worker-proxy"
                    })
                })
                .collect::<Vec<_>>();

            ModelsEndpointResponse {
                status: 200,
                body: json!({
                    "object": "list",
                    "data": models
                }),
            }
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ForwardedHttpRequest {
        path: String,
        model: String,
        is_streaming: bool,
        raw_body: String,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ForwardedWorkerRequest {
        path: String,
        model: String,
        is_streaming: bool,
        raw_body: String,
        headers: BTreeMap<String, String>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ModelsEndpointResponse {
        status: u16,
        body: Value,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum CompatibilityParseError {
        InvalidJson(String),
        MissingModel,
        UnsupportedPath(String),
    }
}
