use std::sync::Arc;
use std::time::{Duration, Instant};

use modelrelay_protocol::{
    HeaderMap, ModelMetadataMap, ProviderRouting, RegisterMessage, ResponseCompleteMessage,
    TokenCounts,
};
use modelrelay_server::{
    ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore, RouteReason, RoutingPolicy,
    RoutingStrategy, SpilloverMode, SubmissionOutcome,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

fn register_worker_msg(
    name: &str,
    models: &[&str],
    max_concurrent: u32,
    metadata: ModelMetadataMap,
) -> RegisterMessage {
    RegisterMessage {
        worker_name: name.to_string(),
        models: models.iter().map(|m| (*m).to_string()).collect(),
        max_concurrent,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: metadata,
    }
}

fn chat_body(model: &str, messages: &Value) -> String {
    json!({
        "model": model,
        "messages": messages
    })
    .to_string()
}

fn submit_chat(core: &mut ProxyServerCore, model: &str, body: String) -> (String, String) {
    match core.submit_transport_request(
        "local",
        model,
        "/v1/chat/completions",
        "POST",
        false,
        body,
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => (d.request_id, d.worker_id),
        other => panic!("expected Dispatched, got {other:?}"),
    }
}

#[test]
fn sequential_multi_turn_sticks_to_warm_worker_instead_of_ping_pong() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["qwen3-coder"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let _w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["qwen3-coder"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let sys = "You are an expert Rust systems engineer working on modelrelay. ".repeat(25);
    let mut messages = vec![
        json!({"role": "system", "content": sys}),
        json!({"role": "user", "content": "Turn 1: read crates/modelrelay-server/src/lib.rs"}),
    ];

    let (req1, chosen1) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(messages)),
    );
    assert_eq!(chosen1, w1);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ColdSpread)
    );
    core.finish_request(&chosen1, &req1);

    let mut prev_matched = 0;
    for turn in 2..=5 {
        messages.push(json!({
            "role": "assistant",
            "content": format!("Detailed analysis for turn {} with code snippets. {}", turn - 1, "x".repeat(400))
        }));
        messages.push(json!({
            "role": "user",
            "content": format!("Turn {turn}: continue refactoring the dispatcher.")
        }));

        let (req_id, chosen) = submit_chat(
            &mut core,
            "qwen3-coder",
            chat_body("qwen3-coder", &json!(messages)),
        );
        assert_eq!(
            chosen, w1,
            "turn {turn} should stick to warm worker {w1}, not ping-pong"
        );
        let decision = core.last_route_decision().expect("decision recorded");
        assert_eq!(decision.reason, RouteReason::PrefixHit);
        assert!(
            decision.matched_prefix_bytes > prev_matched,
            "matched prefix should grow every turn (prev={prev_matched}, now={})",
            decision.matched_prefix_bytes
        );
        prev_matched = decision.matched_prefix_bytes;
        core.finish_request(&chosen, &req_id);
    }

    let stats = core.routing_telemetry();
    assert_eq!(stats.prefix_hits, 4);
    assert_eq!(stats.cold_spreads, 1);
}

#[test]
fn concurrent_same_prefix_spills_to_idle_worker_and_diverging_branches_pin_separately() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["qwen3-coder"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["qwen3-coder"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let shared_sys = "Large repository context and instructions for coding agent. ".repeat(30);
    let base_messages = vec![
        json!({"role": "system", "content": shared_sys}),
        json!({"role": "user", "content": "Analyze the repository architecture."}),
    ];

    // Turn 1 warms w1.
    let (r1, chosen1) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(base_messages)),
    );
    assert_eq!(chosen1, w1);
    core.finish_request(&w1, &r1);

    // Now two concurrent requests with the same shared prefix arrive while w1 is warm.
    let mut branch_a = base_messages.clone();
    branch_a.push(json!({"role": "assistant", "content": "Architecture overview done."}));
    branch_a.push(json!({"role": "user", "content": "Subagent A: audit HTTP layer."}));

    let mut branch_b = base_messages;
    branch_b.push(json!({"role": "assistant", "content": "Architecture overview done."}));
    branch_b.push(json!({"role": "user", "content": "Subagent B: audit WebSocket layer."}));

    // Request 2A sticks to idle warm w1.
    let (req_http_t2, worker_http_t2) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(branch_a)),
    );
    assert_eq!(worker_http_t2, w1);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );

    // While 2A is still in-flight on w1 (load 1/4), concurrent 2B spills to idle w2!
    let (req_ws_t2, worker_ws_t2) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(branch_b)),
    );
    assert_eq!(worker_ws_t2, w2);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ConcurrentSpillover)
    );

    core.finish_request(&w1, &req_http_t2);
    core.finish_request(&w2, &req_ws_t2);

    // Next turn of branch A matches deeper on w1; next turn of branch B matches deeper on w2!
    branch_a.push(json!({"role": "assistant", "content": "HTTP audit findings... ".repeat(40)}));
    branch_a.push(json!({"role": "user", "content": "Fix the HTTP finding."}));

    branch_b
        .push(json!({"role": "assistant", "content": "WebSocket audit findings... ".repeat(40)}));
    branch_b.push(json!({"role": "user", "content": "Fix the WebSocket finding."}));

    let (req_http_t3, worker_http_t3) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(branch_a)),
    );
    assert_eq!(worker_http_t3, w1, "branch A should stay pinned to w1");
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );
    core.finish_request(&w1, &req_http_t3);

    let (req_ws_t3, worker_ws_t3) = submit_chat(
        &mut core,
        "qwen3-coder",
        chat_body("qwen3-coder", &json!(branch_b)),
    );
    assert_eq!(worker_ws_t3, w2, "branch B should stay pinned to w2");
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );
    core.finish_request(&w2, &req_ws_t3);
}

#[test]
fn unrelated_new_conversation_spreads_to_cold_worker_to_avoid_cache_eviction() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["glm-4.7"], 1, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["glm-4.7"], 1, ModelMetadataMap::new()),
        )
        .worker_id;

    let conv1_t1 = vec![
        json!({"role": "system", "content": "Conversation 1 Rust compiler context. ".repeat(30)}),
        json!({"role": "user", "content": "Help with borrow checker."}),
    ];
    let conv2_t1 = vec![
        json!({"role": "system", "content": "Conversation 2 Python ML training context. ".repeat(30)}),
        json!({"role": "user", "content": "Help with PyTorch DataLoader."}),
    ];

    // Conversation 1 lands on w1 and finishes.
    let (r1, c1) = submit_chat(&mut core, "glm-4.7", chat_body("glm-4.7", &json!(conv1_t1)));
    assert_eq!(c1, w1);
    core.finish_request(&w1, &r1);

    // Unrelated Conversation 2 arrives while BOTH workers are idle:
    // must spread to cold LRU worker w2 so w1 keeps Conversation 1's KV cache hot!
    let (r2, c2) = submit_chat(&mut core, "glm-4.7", chat_body("glm-4.7", &json!(conv2_t1)));
    assert_eq!(
        c2, w2,
        "unrelated new conversation should spread to LRU worker w2"
    );
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ColdSpread)
    );
    core.finish_request(&w2, &r2);

    // Follow-up turns for BOTH conversations get 100% prefix hits on their own machines.
    let mut conv1_t2 = conv1_t1;
    conv1_t2.push(json!({"role": "assistant", "content": "Use split borrows."}));
    conv1_t2.push(json!({"role": "user", "content": "Show an example."}));
    let (r3, c3) = submit_chat(&mut core, "glm-4.7", chat_body("glm-4.7", &json!(conv1_t2)));
    assert_eq!(c3, w1);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );
    core.finish_request(&w1, &r3);

    let mut conv2_t2 = conv2_t1;
    conv2_t2.push(json!({"role": "assistant", "content": "Set num_workers=4."}));
    conv2_t2.push(json!({"role": "user", "content": "What about pin_memory?"}));
    let (r4, c4) = submit_chat(&mut core, "glm-4.7", chat_body("glm-4.7", &json!(conv2_t2)));
    assert_eq!(c4, w2);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );
    core.finish_request(&w2, &r4);
}

#[test]
fn eviction_epoch_decays_stale_prefix_after_unrelated_overwrite() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["m1"], 1, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["m1"], 1, ModelMetadataMap::new()),
        )
        .worker_id;

    let conv_a = chat_body(
        "m1",
        &json!([{"role": "user", "content": "Shared alpha prompt content. ".repeat(40)}]),
    );

    // Warm conv_a on w1 first, then spill concurrently to warm conv_a on w2 too.
    let (r1, c1) = submit_chat(&mut core, "m1", conv_a.clone());
    assert_eq!(c1, w1);
    let (r2, c2) = submit_chat(&mut core, "m1", conv_a.clone());
    assert_eq!(c2, w2);
    core.finish_request(&w1, &r1);
    core.finish_request(&w2, &r2);

    // Now overwrite w1's 1-slot KV cache with an unrelated prompt (w1 is LRU since w2 was newer).
    let filler_b = chat_body(
        "m1",
        &json!([{"role": "user", "content": "Completely unrelated beta prompt. ".repeat(40)}]),
    );
    let (rb, cb) = submit_chat(&mut core, "m1", filler_b);
    assert_eq!(cb, w1);
    core.finish_request(&w1, &rb);
    // Also verify eviction epoch incremented on w1 while w2 stayed untouched.
    let m1 = core.worker_routing_metrics(&w1, "m1").expect("w1 metrics");
    let m2 = core.worker_routing_metrics(&w2, "m1").expect("w2 metrics");
    assert!(m1.eviction_epoch > m2.eviction_epoch);

    // When conv_a comes back, w2 (0 evictions since conv_a) must beat w1 (evicted)!
    let (r_back, c_back) = submit_chat(&mut core, "m1", conv_a);
    assert_eq!(c_back, w2, "un-evicted worker w2 should beat evicted w1");
    core.finish_request(&w2, &r_back);
}

#[test]
fn strict_sticky_and_cost_balanced_spillover_modes() {
    // 1. StrictSticky keeps concurrent same-prefix on busy warm worker while slots remain.
    let mut core = ProxyServerCore::new();
    core.set_default_routing_policy(RoutingPolicy {
        spillover_mode: SpilloverMode::StrictSticky,
        ..RoutingPolicy::default()
    });
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("vllm-a", &["m1"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let _w2 = core
        .register_worker(
            "local",
            register_worker_msg("vllm-b", &["m1"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let prompt = chat_body(
        "m1",
        &json!([{"role": "user", "content": "Shared context block. ".repeat(50)}]),
    );
    let (r1, c1) = submit_chat(&mut core, "m1", prompt.clone());
    assert_eq!(c1, w1);
    let (r2, c2) = submit_chat(&mut core, "m1", prompt);
    assert_eq!(
        c2, w1,
        "StrictSticky should keep concurrent request on warm w1"
    );
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::WarmBusySticky)
    );
    core.finish_request(&w1, &r1);
    core.finish_request(&w1, &r2);

    // 2. CostBalanced: tiny prefix spills to idle peer, huge cached prefix packs onto low-load warm peer.
    let mut cb_core = ProxyServerCore::new();
    cb_core.set_default_routing_policy(RoutingPolicy {
        spillover_mode: SpilloverMode::CostBalanced,
        ..RoutingPolicy::default()
    });
    let cw1 = cb_core
        .register_worker(
            "local",
            register_worker_msg("vllm-a", &["m1"], 8, ModelMetadataMap::new()),
        )
        .worker_id;
    let cw2 = cb_core
        .register_worker(
            "local",
            register_worker_msg("vllm-b", &["m1"], 8, ModelMetadataMap::new()),
        )
        .worker_id;

    // Small 1.5KB prefix: cold prefill (~95ms) < load penalty (~312ms) -> spills to idle cw2.
    let small_prompt = chat_body(
        "m1",
        &json!([{"role": "user", "content": "a".repeat(1500)}]),
    );
    let (sr1, sc1) = submit_chat(&mut cb_core, "m1", small_prompt.clone());
    assert_eq!(sc1, cw1);
    let (sr2, sc2) = submit_chat(&mut cb_core, "m1", small_prompt);
    assert_eq!(sc2, cw2, "small prefix should spill in CostBalanced mode");
    assert_eq!(
        cb_core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ConcurrentSpillover)
    );
    cb_core.finish_request(&cw1, &sr1);
    cb_core.finish_request(&cw2, &sr2);

    // Huge 40KB prefix: cold prefill (~2500ms) >> load penalty (~312ms at 1/8 load) -> packs onto warm cw1.
    let huge_prompt = chat_body(
        "m1",
        &json!([{"role": "user", "content": "b".repeat(40_000)}]),
    );
    let (hr1, hc1) = submit_chat(&mut cb_core, "m1", huge_prompt.clone());
    let (hr2, hc2) = submit_chat(&mut cb_core, "m1", huge_prompt);
    assert_eq!(
        hc2, hc1,
        "huge cached prefix should stay on low-load warm worker in CostBalanced mode"
    );
    assert_eq!(
        cb_core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::WarmBusySticky)
    );
    cb_core.finish_request(&hc1, &hr1);
    cb_core.finish_request(&hc2, &hr2);
}

#[test]
fn context_window_guard_skips_undersized_worker() {
    let mut core = ProxyServerCore::new();

    let mut small_meta = ModelMetadataMap::new();
    small_meta.insert(
        "qwen3".to_string(),
        json!({"id": "qwen3", "context_length": 4096})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut large_meta = ModelMetadataMap::new();
    large_meta.insert(
        "qwen3".to_string(),
        json!({"id": "qwen3", "context_length": 131_072})
            .as_object()
            .unwrap()
            .clone(),
    );

    let w_small = core
        .register_worker(
            "local",
            register_worker_msg("mac-small", &["qwen3"], 2, small_meta),
        )
        .worker_id;
    let w_large = core
        .register_worker(
            "local",
            register_worker_msg("rig-large", &["qwen3"], 2, large_meta),
        )
        .worker_id;

    // ~24 KB prompt ≈ ~6,000 tokens > 4096 * 0.95 -> must skip w_small and route to w_large!
    let big_body = chat_body(
        "qwen3",
        &json!([{"role": "user", "content": "token ".repeat(4000)}]),
    );
    let (r_big, c_big) = submit_chat(&mut core, "qwen3", big_body);
    assert_eq!(c_big, w_large);
    assert_eq!(core.routing_telemetry().context_guard_skips, 1);
    core.finish_request(&w_large, &r_big);

    // Small 200-byte prompt fits w_small fine (and w_small is LRU since w_large just ran!).
    let small_body = chat_body("qwen3", &json!([{"role": "user", "content": "Hi there!"}]));
    let (r_small, c_small) = submit_chat(&mut core, "qwen3", small_body);
    assert_eq!(c_small, w_small);
    core.finish_request(&w_small, &r_small);
}

#[test]
fn loaded_model_preference_beats_unloaded_router_worker() {
    let mut core = ProxyServerCore::new();

    let mut unloaded_meta = ModelMetadataMap::new();
    unloaded_meta.insert(
        "qwen3".to_string(),
        json!({"id": "qwen3", "status": {"value": "unloaded"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let mut loaded_meta = ModelMetadataMap::new();
    loaded_meta.insert(
        "qwen3".to_string(),
        json!({"id": "qwen3", "status": {"value": "loaded"}})
            .as_object()
            .unwrap()
            .clone(),
    );

    let _w_unloaded = core
        .register_worker(
            "local",
            register_worker_msg("router-cold", &["qwen3"], 2, unloaded_meta),
        )
        .worker_id;
    let w_loaded = core
        .register_worker(
            "local",
            register_worker_msg("router-hot", &["qwen3"], 2, loaded_meta),
        )
        .worker_id;

    let body = chat_body("qwen3", &json!([{"role": "user", "content": "Hello"}]));
    let (_, chosen) = submit_chat(&mut core, "qwen3", body);
    assert_eq!(
        chosen, w_loaded,
        "loaded worker should beat unloaded worker"
    );
}

#[test]
fn priority_failover_prefers_primary_and_overflows_when_busy() {
    let mut core = ProxyServerCore::new();
    core.set_default_routing_policy(RoutingPolicy {
        strategy: RoutingStrategy::PriorityFailover,
        worker_priority: vec!["rtx-5090".to_string(), "mac-studio".to_string()],
        sticky_after_overflow: true,
        ..RoutingPolicy::default()
    });

    // Register slower backup FIRST to prove priority order overrides registration order.
    let w_mac = core
        .register_worker(
            "local",
            register_worker_msg("mac-studio", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let w_5090 = core
        .register_worker(
            "local",
            register_worker_msg("rtx-5090", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let prompt_a = chat_body(
        "qwen3",
        &json!([{"role": "user", "content": "Primary task A prompt content. ".repeat(35)}]),
    );
    let prompt_b_t1 =
        vec![json!({"role": "user", "content": "Overflow task B prompt content. ".repeat(35)})];

    // Solo request goes to rtx-5090.
    let (ra, ca) = submit_chat(&mut core, "qwen3", prompt_a);
    assert_eq!(ca, w_5090);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PriorityPrimary)
    );

    // Concurrent request while rtx-5090 is busy overflows to mac-studio.
    let (rb1, cb1) = submit_chat(&mut core, "qwen3", chat_body("qwen3", &json!(prompt_b_t1)));
    assert_eq!(cb1, w_mac);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PriorityOverflow)
    );

    core.finish_request(&w_5090, &ra);
    core.finish_request(&w_mac, &rb1);

    // Follow-up turn of Overflow task B sticks to warm idle mac-studio (sticky_after_overflow=true).
    let mut prompt_b_t2 = prompt_b_t1;
    prompt_b_t2.push(json!({"role": "assistant", "content": "Reply on mac-studio."}));
    prompt_b_t2.push(json!({"role": "user", "content": "Follow up."}));
    let (rb2, cb2) = submit_chat(&mut core, "qwen3", chat_body("qwen3", &json!(prompt_b_t2)));
    assert_eq!(cb2, w_mac);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PrefixHit)
    );
    core.finish_request(&w_mac, &rb2);
}

#[test]
fn multi_tenant_prefix_isolation_prevents_cross_tenant_leakage() {
    let mut core = ProxyServerCore::new();
    // Shared fleet workers visible to both tenants.
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("fleet-1", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("fleet-2", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let secret_prompt = chat_body(
        "qwen3",
        &json!([{"role": "user", "content": "Confidential Tenant A document. ".repeat(40)}]),
    );

    // Tenant A warms fleet-1.
    let d_a = match core.submit_transport_request_for_owner(
        "local",
        "qwen3",
        "/v1/chat/completions",
        "POST",
        false,
        secret_prompt.clone(),
        HeaderMap::new(),
        None,
        Some("tenant-a".to_string()),
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(d_a.worker_id, w1);
    core.finish_request(&w1, &d_a.request_id);

    // Tenant B sends the EXACT SAME prompt: must NOT see Tenant A's prefix cache!
    let d_b = match core.submit_transport_request_for_owner(
        "local",
        "qwen3",
        "/v1/chat/completions",
        "POST",
        false,
        secret_prompt,
        HeaderMap::new(),
        None,
        Some("tenant-b".to_string()),
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(
        d_b.worker_id, w2,
        "Tenant B should cold-spread to w2, not hit Tenant A's trie"
    );
    let decision = core.last_route_decision().expect("decision recorded");
    assert_eq!(decision.reason, RouteReason::ColdSpread);
    assert_eq!(decision.matched_prefix_bytes, 0);
}

#[test]
fn responses_api_previous_response_id_and_session_header_stickiness() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["gpt-oss"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["gpt-oss"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    // 1. OpenAI Responses API: first turn completes with {"id": "resp_xyz"}.
    let pending = core
        .submit_http_response_request(
            "local",
            "gpt-oss",
            "/v1/responses",
            "POST",
            json!({"model": "gpt-oss", "input": "Start session"}).to_string(),
            HeaderMap::new(),
            None,
        )
        .expect("submit turn 1");
    assert_eq!(
        core.worker_in_flight_request_ids(&w1),
        vec![pending.request_id.clone()]
    );
    assert!(core.complete_http_response(
        &w1,
        ResponseCompleteMessage {
            request_id: pending.request_id,
            status_code: 200,
            headers: HeaderMap::new(),
            body: Some(json!({"id": "resp_xyz", "output": []}).to_string()),
            token_counts: None,
        },
    ));

    // Second turn sends tiny body with previous_response_id -> must stick to w1 via SessionHit!
    let d2 = match core.submit_transport_request(
        "local",
        "gpt-oss",
        "/v1/responses",
        "POST",
        false,
        json!({"model": "gpt-oss", "previous_response_id": "resp_xyz", "input": "next"})
            .to_string(),
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(d2.worker_id, w1);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::SessionHit)
    );
    core.finish_request(&w1, &d2.request_id);

    // 2. Explicit x-session-affinity header binds a new session to LRU worker w2,
    //    and subsequent short turns stick to w2.
    let mut sess_headers = HeaderMap::new();
    sess_headers.insert(
        "x-session-affinity".to_string(),
        "my-session-42".to_string(),
    );
    let s1 = match core.submit_transport_request(
        "local",
        "gpt-oss",
        "/v1/chat/completions",
        "POST",
        false,
        chat_body("gpt-oss", &json!([{"role": "user", "content": "hi"}])),
        sess_headers.clone(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(s1.worker_id, w2);
    core.finish_request(&w2, &s1.request_id);

    let s2 = match core.submit_transport_request(
        "local",
        "gpt-oss",
        "/v1/chat/completions",
        "POST",
        false,
        chat_body("gpt-oss", &json!([{"role": "user", "content": "ok"}])),
        sess_headers,
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(s2.worker_id, w2);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::SessionHit)
    );
}

#[test]
fn bounded_prefix_aware_queue_drain_prefers_warm_match_without_starvation() {
    let mut core = ProxyServerCore::new();
    core.set_default_queue_policy(ProviderQueuePolicy {
        max_queue_len: 16,
        queue_timeout_ticks: Some(30),
    });

    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-a", &["qwen3"], 1, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-b", &["qwen3"], 1, ModelMetadataMap::new()),
        )
        .worker_id;

    let prompt_a = chat_body(
        "qwen3",
        &json!([{"role": "user", "content": "Session A huge prefix. ".repeat(50)}]),
    );
    let prompt_b = chat_body(
        "qwen3",
        &json!([{"role": "user", "content": "Session B huge prefix. ".repeat(50)}]),
    );

    // Fill w1 with Session A, w2 with Session B.
    let (alpha_req, alpha_worker) = submit_chat(&mut core, "qwen3", prompt_a.clone());
    assert_eq!(alpha_worker, w1);
    let (beta_req, beta_worker) = submit_chat(&mut core, "qwen3", prompt_b.clone());
    assert_eq!(beta_worker, w2);

    // Queue Session B FIRST, then Session A SECOND.
    let q_b = match core.submit_transport_request(
        "local",
        "qwen3",
        "/v1/chat/completions",
        "POST",
        false,
        prompt_b,
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Queued(q) => q.request_id,
        other => panic!("expected Queued, got {other:?}"),
    };
    let q_a = match core.submit_transport_request(
        "local",
        "qwen3",
        "/v1/chat/completions",
        "POST",
        false,
        prompt_a,
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Queued(q) => q.request_id,
        other => panic!("expected Queued, got {other:?}"),
    };

    // When w1 (warm for Session A!) frees up, it pulls q_a out of the queue ahead of q_b!
    let next_for_w1 = core
        .finish_request(&w1, &alpha_req)
        .expect("w1 should pull queued request");
    assert_eq!(next_for_w1.request_id, q_a);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::QueuedPrefixMatch)
    );

    // When w2 (warm for Session B!) frees up, it pulls q_b!
    let next_for_w2 = core
        .finish_request(&w2, &beta_req)
        .expect("w2 should pull queued request");
    assert_eq!(next_for_w2.request_id, q_b);
}

#[test]
fn per_request_sort_overrides_work_for_round_robin_and_least_loaded() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-small", &["m1"], 1, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("gpu-big", &["m1"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    let repeated = chat_body(
        "m1",
        &json!([{"role": "user", "content": "Repeated prompt content. ".repeat(40)}]),
    );

    // With explicit sort="round_robin", even sequential identical prompts alternate!
    let rr = Some(ProviderRouting {
        sort: Some("round_robin".to_string()),
        ..Default::default()
    });
    let r1 = match core.submit_transport_request(
        "local",
        "m1",
        "/v1/chat/completions",
        "POST",
        false,
        repeated.clone(),
        HeaderMap::new(),
        rr.clone(),
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(r1.worker_id, w1);
    core.finish_request(&w1, &r1.request_id);

    let r2 = match core.submit_transport_request(
        "local",
        "m1",
        "/v1/chat/completions",
        "POST",
        false,
        repeated,
        HeaderMap::new(),
        rr,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(r2.worker_id, w2);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::RoundRobin)
    );
    core.finish_request(&w2, &r2.request_id);

    // Normalized LeastLoaded: put 1 request on w2 (1/4 = 0.25) and keep w1 at 0/1 (0.0);
    // first least-loaded goes to w1 (0.0), next goes to w2 (0.25) while w1 is full (1/1).
    let ll = Some(ProviderRouting {
        sort: Some("load".to_string()),
        ..Default::default()
    });
    let l1 = match core.submit_transport_request(
        "local",
        "m1",
        "/v1/chat/completions",
        "POST",
        false,
        String::new(),
        HeaderMap::new(),
        ll.clone(),
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(l1.worker_id, w1);
    let l2 = match core.submit_transport_request(
        "local",
        "m1",
        "/v1/chat/completions",
        "POST",
        false,
        String::new(),
        HeaderMap::new(),
        ll,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(l2.worker_id, w2);
}

#[test]
fn error_penalty_deprioritizes_flapping_worker_and_sweep_cleans_disconnect() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("flaky-gpu", &["m1"], 2, ModelMetadataMap::new()),
        )
        .worker_id;
    let w2 = core
        .register_worker(
            "local",
            register_worker_msg("healthy-gpu", &["m1"], 2, ModelMetadataMap::new()),
        )
        .worker_id;

    let p1 = core
        .submit_http_response_request(
            "local",
            "m1",
            "/v1/chat/completions",
            "POST",
            chat_body("m1", &json!([{"role": "user", "content": "Hello"}])),
            HeaderMap::new(),
            None,
        )
        .expect("submit");
    assert_eq!(
        core.worker_in_flight_request_ids(&w1),
        vec![p1.request_id.clone()]
    );

    // Worker 1 fails with a backend error.
    assert!(core.handle_worker_error(&w1, Some(&p1.request_id), "backend_error", "OOM"));

    // Next request must route to healthy w2.
    let (r2, c2) = submit_chat(
        &mut core,
        "m1",
        chat_body("m1", &json!([{"role": "user", "content": "Hello 2"}])),
    );
    assert_eq!(c2, w2);
    core.finish_request(&w2, &r2);

    // Even after w2 has a newer dispatch seq, a third request STILL prefers healthy w2 over penalized w1!
    let (r3, c3) = submit_chat(
        &mut core,
        "m1",
        chat_body("m1", &json!([{"role": "user", "content": "Hello 3"}])),
    );
    assert_eq!(
        c3, w2,
        "penalized worker w1 should be avoided during error window"
    );
    core.finish_request(&w2, &r3);

    // Disconnecting w2 + sweeping cleans up prefix trie nodes cleanly.
    core.disconnect_worker(&w2);
    core.sweep_routing_state(Instant::now() + Duration::from_secs(3600));
    assert_eq!(core.routing_telemetry().tracked_prefix_nodes, 0);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn http_end_to_end_prefix_stickiness_via_axum() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let (w1, _w2) = {
        let mut guard = core.lock().await;
        let w1 = guard
            .register_worker(
                "local",
                register_worker_msg("gpu-a", &["qwen3"], 4, ModelMetadataMap::new()),
            )
            .worker_id;
        let w2 = guard
            .register_worker(
                "local",
                register_worker_msg("gpu-b", &["qwen3"], 4, ModelMetadataMap::new()),
            )
            .worker_id;
        (w1, w2)
    };

    let app = ProxyHttpApp::new(Arc::clone(&core))
        .with_models_provider("local")
        .router();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let sys = "Shared system prompt across HTTP turns. ".repeat(30);
    let body_turn1 = json!({
        "model": "qwen3",
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": "Turn 1"}
        ]
    })
    .to_string();
    let body_turn2 = json!({
        "model": "qwen3",
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": "Turn 1"},
            {"role": "assistant", "content": "Turn 1 response"},
            {"role": "user", "content": "Turn 2"}
        ]
    })
    .to_string();

    for (turn_idx, body) in [body_turn1, body_turn2].into_iter().enumerate() {
        let core_bg = Arc::clone(&core);
        let expected_worker = w1.clone();
        let responder = tokio::spawn(async move {
            for _ in 0..100 {
                let pending = {
                    let mut g = core_bg.lock().await;
                    g.take_pending_worker_requests(&expected_worker)
                };
                if let Some(req) = pending.into_iter().next() {
                    let mut g = core_bg.lock().await;
                    g.complete_http_response(
                        &expected_worker,
                        ResponseCompleteMessage {
                            request_id: req.request_id,
                            status_code: 200,
                            headers: HeaderMap::from([(
                                "content-type".to_string(),
                                "application/json".to_string(),
                            )]),
                            body: Some(json!({"choices": []}).to_string()),
                            token_counts: None,
                        },
                    );
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            false
        });

        let mut stream = TcpStream::connect(addr).await.unwrap();
        let raw_req = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(raw_req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).await.unwrap();
        assert!(
            resp.starts_with("HTTP/1.1 200 OK\r\n"),
            "expected 200 OK, got: {resp}"
        );
        assert!(
            responder.await.unwrap(),
            "turn {} should have been routed to warm worker {w1}",
            turn_idx + 1
        );
    }

    let stats = core.lock().await.routing_telemetry();
    assert_eq!(stats.prefix_hits, 1);
    assert!(
        core.lock()
            .await
            .worker_routing_metrics(&w1, "qwen3")
            .unwrap()
            .ewma_ttft_ms
            .is_some()
    );
}

#[test]
fn cost_balanced_adapts_to_calibrated_fast_vs_slow_machine_prefill_speed() {
    let mut core = ProxyServerCore::new();
    core.set_default_routing_policy(RoutingPolicy {
        strategy: RoutingStrategy::PrefixAware,
        spillover_mode: SpilloverMode::CostBalanced,
        ..RoutingPolicy::default()
    });

    let w_primary = core
        .register_worker(
            "local",
            register_worker_msg("rtx-5090", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;
    let w_peer = core
        .register_worker(
            "local",
            register_worker_msg("secondary-box", &["qwen3"], 4, ModelMetadataMap::new()),
        )
        .worker_id;

    // Warm primary worker with a ~16 KiB shared system prompt.
    let big_sys = "S".repeat(16_384);
    let body_turn1 = chat_body(
        "qwen3",
        &json!([
            {"role": "system", "content": big_sys},
            {"role": "user", "content": "Turn 1"}
        ]),
    );
    let (_, warm_worker) = submit_chat(&mut core, "qwen3", body_turn1);
    let idle_peer = if warm_worker == w_primary {
        w_peer
    } else {
        w_primary
    };

    // Calibrate warm worker as fast (~32 bytes/ms) and idle peer as slow (~2 bytes/ms).
    // While warm_worker has load_ratio = 0.25 (1/4), its busy load penalty is 625ms,
    // whereas cold-prefilling 16 KiB on the slow idle peer costs ~8,192ms -> stay sticky!
    core.record_worker_prefill_observation(
        &warm_worker,
        "qwen3",
        16_000,
        Duration::from_millis(500),
    );
    core.record_worker_prefill_observation(
        &idle_peer,
        "qwen3",
        16_000,
        Duration::from_millis(8_000),
    );

    let body_turn2 = chat_body(
        "qwen3",
        &json!([
            {"role": "system", "content": big_sys},
            {"role": "user", "content": "Turn 2 while Turn 1 is still running"}
        ]),
    );
    let (_, w2) = submit_chat(&mut core, "qwen3", body_turn2);
    assert_eq!(
        w2, warm_worker,
        "CostBalanced must stay sticky when idle peer is calibrated slow"
    );
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::WarmBusySticky)
    );

    // Now re-calibrate the idle peer to be very fast (~100 bytes/ms, e.g. another RTX 5090).
    // Cold-prefilling 16 KiB on the fast idle peer costs ~160ms vs. 1,250ms load penalty
    // on warm_worker (now 2/4 busy) -> CostBalanced should spill to the fast idle peer!
    for _ in 0..8 {
        core.record_worker_prefill_observation(
            &idle_peer,
            "qwen3",
            20_000,
            Duration::from_millis(200),
        );
    }
    let body_turn3 = chat_body(
        "qwen3",
        &json!([
            {"role": "system", "content": big_sys},
            {"role": "user", "content": "Turn 3 concurrent burst"}
        ]),
    );
    let (_, w3) = submit_chat(&mut core, "qwen3", body_turn3);
    assert_eq!(
        w3, idle_peer,
        "CostBalanced should spill once idle peer is calibrated fast"
    );
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ConcurrentSpillover)
    );
}

#[test]
fn verified_token_usage_updates_telemetry_and_worker_metrics() {
    let mut core = ProxyServerCore::new();
    let w1 = core
        .register_worker(
            "local",
            register_worker_msg("rtx-5090", &["qwen3"], 2, ModelMetadataMap::new()),
        )
        .worker_id;

    let body = chat_body("qwen3", &json!([{"role": "user", "content": "hello"}]));
    let (req_id, _) = submit_chat(&mut core, "qwen3", body);

    assert!(core.complete_http_response(
        &w1,
        ResponseCompleteMessage {
            request_id: req_id,
            status_code: 200,
            headers: HeaderMap::new(),
            body: Some("{}".to_string()),
            token_counts: Some(TokenCounts {
                prompt_tokens: Some(4000),
                completion_tokens: Some(120),
                total_tokens: Some(4120),
                cached_prompt_tokens: Some(3600),
            }),
        },
    ));

    let telemetry = core.routing_telemetry();
    assert_eq!(telemetry.verified_prompt_tokens, 4000);
    assert_eq!(telemetry.verified_cached_prompt_tokens, 3600);

    let metrics = core.worker_routing_metrics(&w1, "qwen3").unwrap();
    assert_eq!(metrics.prompt_tokens_total, 4000);
    assert_eq!(metrics.cached_prompt_tokens_total, 3600);
}
