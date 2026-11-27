use std::time::{Duration, Instant};

use modelrelay_protocol::{HeaderMap, ModelMetadataMap, ProviderRouting, RegisterMessage};
use modelrelay_server::{
    CancelReason, ProviderQueuePolicy, ProxyServerCore, RequestFailureReason, SubmissionOutcome,
};

fn register_msg(name: &str, models: &[&str], max_concurrent: u32) -> RegisterMessage {
    RegisterMessage {
        worker_name: name.to_string(),
        models: models.iter().map(|m| (*m).to_string()).collect(),
        max_concurrent,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: ModelMetadataMap::new(),
    }
}

/// Live-implementation contract test (F-42): exercises the real `ProxyServerCore`
/// rather than a shadow harness to prevent contract/implementation drift.
#[test]
fn live_core_drains_queued_work_when_worker_registers_and_on_disconnect() {
    let mut core = ProxyServerCore::new();
    core.set_default_queue_policy(ProviderQueuePolicy {
        max_queue_len: 10,
        queue_timeout_ticks: Some(30),
    });

    let queued = core.submit_request("local", "llama-3");
    assert!(matches!(queued, SubmissionOutcome::Queued(_)));

    // Registering a compatible worker immediately drains the queue.
    let w1 = core
        .register_worker("local", register_msg("gpu-1", &["llama-3"], 1))
        .worker_id;
    assert_eq!(core.worker_in_flight_request_ids(&w1), vec!["request-1"]);

    // Registering a second idle worker and disconnecting w1 immediately moves request-1 to w2.
    let w2 = core
        .register_worker("local", register_msg("gpu-2", &["llama-3"], 1))
        .worker_id;
    let outcome = core.disconnect_worker(&w1).expect("disconnect w1");
    assert_eq!(outcome.requeued_request_ids, vec!["request-1"]);
    assert_eq!(core.worker_in_flight_request_ids(&w2), vec!["request-1"]);
}

/// Live-implementation contract test (F-42): verifies tenant isolation and
/// cancel-grace slot reclamation on the real `ProxyServerCore`.
#[test]
fn live_core_enforces_tenant_isolation_and_cancel_reap() {
    let mut core = ProxyServerCore::new();
    let w_a = core
        .register_worker_for_owner(
            "local",
            register_msg("gpu-a", &["model-x"], 1),
            Some("key-a".to_string()),
        )
        .worker_id;
    let _w_b = core
        .register_worker_for_owner(
            "local",
            register_msg("gpu-b", &["model-x"], 1),
            Some("key-b".to_string()),
        )
        .worker_id;

    // Tenant A's catalog only lists Tenant A's models; requests only go to w_a.
    let pending = core
        .submit_http_response_request_for_owner(
            "local",
            "model-x",
            "/v1/chat/completions",
            "POST",
            "{}".to_string(),
            HeaderMap::new(),
            None,
            Some("key-a".to_string()),
        )
        .expect("tenant A submits request");
    assert_eq!(
        core.worker_in_flight_request_ids(&w_a),
        vec![pending.request_id.clone()]
    );

    // Tenant A's second request must queue (cannot spill over to Tenant B's idle worker).
    let pending2 = core
        .submit_http_response_request_for_owner(
            "local",
            "model-x",
            "/v1/chat/completions",
            "POST",
            "{}".to_string(),
            HeaderMap::new(),
            Some(ProviderRouting {
                allow_fallbacks: Some(true),
                ..Default::default()
            }),
            Some("key-a".to_string()),
        )
        .expect("tenant A second request queues");
    assert_eq!(
        core.request_state(&pending2.request_id),
        Some(modelrelay_server::RequestState::Queued)
    );

    // Cancel first request; after grace period without ack, core reaps slot and dispatches pending2.
    let _ = core.cancel_request(&pending.request_id, CancelReason::ClientDisconnected);
    let reaped = core.expire_cancelled_requests(
        Instant::now() + Duration::from_secs(6),
        Duration::from_secs(5),
    );
    assert_eq!(reaped, vec![pending.request_id]);
    assert_eq!(
        core.worker_in_flight_request_ids(&w_a),
        vec![pending2.request_id]
    );
}

/// Live-implementation contract test (F-42): verifies mid-stream disconnect fails
/// immediately rather than requeueing.
#[test]
fn live_core_does_not_requeue_mid_stream_disconnect() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker("local", register_msg("gpu-1", &["m1"], 1))
        .worker_id;
    let _w2 = core
        .register_worker("local", register_msg("gpu-2", &["m1"], 1))
        .worker_id;

    let pending = core
        .submit_http_streaming_request(
            "local",
            "m1",
            "/v1/chat/completions",
            "POST",
            "{}".to_string(),
            HeaderMap::new(),
            None,
        )
        .expect("streaming request dispatched");

    assert!(core.stream_http_response_chunk(&w1, &pending.request_id, "data: 1\n\n".to_string()));
    let outcome = core.disconnect_worker(&w1).expect("disconnect w1");
    assert!(outcome.requeued_request_ids.is_empty());
    assert_eq!(outcome.failed_requests.len(), 1);
    assert_eq!(
        outcome.failed_requests[0].reason,
        RequestFailureReason::WorkerDisconnectedMidStream
    );
}
