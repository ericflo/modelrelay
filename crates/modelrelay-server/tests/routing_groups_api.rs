#![allow(clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use modelrelay_protocol::admin_api::{
    ModelGroupMode, ModelGroupTarget, ModelRoutingGroupConfig, RoutingPolicyConfig,
    RoutingStateResponse, RoutingStrategyDto, TenantRoutingConfig, UpdateRoutingConfigRequest,
};
use modelrelay_protocol::{HeaderMap, ModelMetadataMap, RegisterMessage, ResponseCompleteMessage};
use modelrelay_server::{
    ApiKeyStore, FileRoutingConfigStore, InMemoryApiKeyStore, ProxyHttpApp, ProxyServerCore,
    RouteReason, RoutingConfigStore, SubmissionOutcome,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

fn reg_msg(
    name: &str,
    models: &[&str],
    max_concurrent: u32,
    meta: ModelMetadataMap,
) -> RegisterMessage {
    RegisterMessage {
        worker_name: name.to_string(),
        models: models.iter().map(|m| (*m).to_string()).collect(),
        max_concurrent,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: meta,
    }
}

async fn send_http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        let _ = write!(req, "{k}: {v}\r\n");
    }
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
        let _ = write!(req, "Content-Length: {}\r\n", body.len());
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    (status, head.to_string(), resp_body.to_string())
}

#[test]
fn exact_model_override_and_shared_policy_group_apply_independently() {
    let mut core = ProxyServerCore::new();
    let w_mac = core
        .register_worker(
            "local",
            reg_msg(
                "mac-studio",
                &["qwen3-coder", "nomic-embed", "glm-4.7"],
                4,
                ModelMetadataMap::new(),
            ),
        )
        .worker_id;
    let w_5090 = core
        .register_worker(
            "local",
            reg_msg(
                "rtx-5090",
                &["qwen3-coder", "nomic-embed", "glm-4.7"],
                4,
                ModelMetadataMap::new(),
            ),
        )
        .worker_id;

    let mut model_overrides = BTreeMap::new();
    model_overrides.insert(
        "nomic-embed".to_string(),
        RoutingPolicyConfig {
            strategy: RoutingStrategyDto::RoundRobin,
            ..RoutingPolicyConfig::default()
        },
    );

    let cfg = TenantRoutingConfig {
        default_policy: RoutingPolicyConfig::default(),
        model_overrides,
        groups: vec![ModelRoutingGroupConfig {
            id: "coding-priority".to_string(),
            name: "Coding Priority".to_string(),
            mode: ModelGroupMode::SharedPolicy,
            pool_model_id: None,
            targets: vec![
                ModelGroupTarget {
                    worker_name: None,
                    model_id: "qwen3-coder".to_string(),
                },
                ModelGroupTarget {
                    worker_name: None,
                    model_id: "glm-4.7".to_string(),
                },
            ],
            policy: RoutingPolicyConfig {
                strategy: RoutingStrategyDto::PriorityFailover,
                worker_priority: vec!["rtx-5090".to_string(), "mac-studio".to_string()],
                ..RoutingPolicyConfig::default()
            },
        }],
        expose_debug_headers: false,
    };
    core.set_routing_config_for_owner(None, cfg);

    // 1. qwen3-coder & glm-4.7 share the Coding Priority group -> prefer rtx-5090 even though mac-studio registered first.
    let d1 = match core.submit_transport_request(
        "local",
        "qwen3-coder",
        "/v1/chat/completions",
        "POST",
        false,
        json!({"model": "qwen3-coder", "messages": [{"role": "user", "content": "hi"}]})
            .to_string(),
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(d1.worker_id, w_5090);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::PriorityPrimary)
    );
    core.finish_request(&w_5090, &d1.request_id);

    // 2. nomic-embed has an exact override to RoundRobin -> alternates mac-studio then rtx-5090.
    let e1 = match core.submit_transport_request(
        "local",
        "nomic-embed",
        "/v1/embeddings",
        "POST",
        false,
        json!({"model": "nomic-embed", "input": "hello"}).to_string(),
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(e1.worker_id, w_mac);
    core.finish_request(&w_mac, &e1.request_id);

    let e2 = match core.submit_transport_request(
        "local",
        "nomic-embed",
        "/v1/embeddings",
        "POST",
        false,
        json!({"model": "nomic-embed", "input": "hello"}).to_string(),
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(e2.worker_id, w_5090);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::RoundRobin)
    );
}

#[test]
fn unified_pool_routes_across_different_model_ids_and_rewrites_request_body() {
    let mut core = ProxyServerCore::new();

    let mut ollama_meta = ModelMetadataMap::new();
    ollama_meta.insert(
        "qwen3:32b".to_string(),
        json!({"id": "qwen3:32b", "context_length": 65_536})
            .as_object()
            .unwrap()
            .clone(),
    );
    let w_ollama = core
        .register_worker(
            "local",
            reg_msg("ollama-mac", &["qwen3:32b"], 4, ollama_meta),
        )
        .worker_id;
    let w_lms = core
        .register_worker(
            "local",
            reg_msg(
                "lmstudio-pc",
                &["qwen/qwen3-32b-q4_k_m"],
                4,
                ModelMetadataMap::new(),
            ),
        )
        .worker_id;

    core.set_routing_config_for_owner(
        None,
        TenantRoutingConfig {
            groups: vec![ModelRoutingGroupConfig {
                id: "pool-qwen3-32b".to_string(),
                name: "Qwen3 32B Pool".to_string(),
                mode: ModelGroupMode::UnifiedPool,
                pool_model_id: Some("qwen3-32b".to_string()),
                targets: vec![
                    ModelGroupTarget {
                        worker_name: Some("ollama-mac".to_string()),
                        model_id: "qwen3:32b".to_string(),
                    },
                    ModelGroupTarget {
                        worker_name: Some("lmstudio-pc".to_string()),
                        model_id: "qwen/qwen3-32b-q4_k_m".to_string(),
                    },
                ],
                policy: RoutingPolicyConfig::default(),
            }],
            ..TenantRoutingConfig::default()
        },
    );

    // 1. /v1/models exposes the unified pool ID with inherited metadata from ollama-mac.
    let catalog = core.provider_model_entries_for_owner("local", None);
    let pool_entry = catalog
        .iter()
        .find(|m| m.get("id").and_then(Value::as_str) == Some("qwen3-32b"))
        .expect("unified pool model should appear in /v1/models");
    assert_eq!(
        pool_entry.get("context_length").and_then(Value::as_u64),
        Some(65_536)
    );

    // 2. Request 1 for "qwen3-32b" lands on ollama-mac with body rewritten to "qwen3:32b".
    let shared_prompt = json!({
        "model": "qwen3-32b",
        "messages": [{"role": "user", "content": "Unified pool shared prefix. ".repeat(40)}]
    })
    .to_string();

    let d1 = match core.submit_transport_request(
        "local",
        "qwen3-32b",
        "/v1/chat/completions",
        "POST",
        false,
        shared_prompt.clone(),
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(d1.worker_id, w_ollama);
    let dispatched_ollama = core.take_pending_worker_requests(&w_ollama);
    assert_eq!(dispatched_ollama.len(), 1);
    assert_eq!(dispatched_ollama[0].model, "qwen3:32b");
    let parsed_ollama_body: Value = serde_json::from_str(&dispatched_ollama[0].body).unwrap();
    assert_eq!(parsed_ollama_body["model"], "qwen3:32b");

    // 3. Concurrent Request 2 for "qwen3-32b" spills to lmstudio-pc with body rewritten to "qwen/qwen3-32b-q4_k_m"!
    let d2 = match core.submit_transport_request(
        "local",
        "qwen3-32b",
        "/v1/chat/completions",
        "POST",
        false,
        shared_prompt,
        HeaderMap::new(),
        None,
    ) {
        SubmissionOutcome::Dispatched(d) => d,
        other => panic!("expected Dispatched, got {other:?}"),
    };
    assert_eq!(d2.worker_id, w_lms);
    assert_eq!(
        core.last_route_decision().map(|d| d.reason),
        Some(RouteReason::ConcurrentSpillover)
    );
    let dispatched_lms = core.take_pending_worker_requests(&w_lms);
    assert_eq!(dispatched_lms.len(), 1);
    assert_eq!(dispatched_lms[0].model, "qwen/qwen3-32b-q4_k_m");
    let parsed_lms_body: Value = serde_json::from_str(&dispatched_lms[0].body).unwrap();
    assert_eq!(parsed_lms_body["model"], "qwen/qwen3-32b-q4_k_m");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn unified_routing_http_api_auth_persistence_and_debug_headers() {
    let tmp_path = std::env::temp_dir().join(format!(
        "modelrelay-routing-test-{}.json",
        uuid::Uuid::new_v4()
    ));
    let routing_store = Arc::new(FileRoutingConfigStore::new(&tmp_path));
    let key_store = Arc::new(InMemoryApiKeyStore::new());
    let (meta_a, secret_a) = key_store.create_key("tenant-a".to_string()).await.unwrap();
    let (meta_b, secret_b) = key_store.create_key("tenant-b".to_string()).await.unwrap();

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let w_a1 = {
        let mut g = core.lock().await;
        let w1 = g
            .register_worker_for_owner(
                "local",
                reg_msg("box-a1", &["qwen3"], 4, ModelMetadataMap::new()),
                Some(meta_a.id.clone()),
            )
            .worker_id;
        let _w2 = g
            .register_worker_for_owner(
                "local",
                reg_msg("box-a2", &["qwen3"], 4, ModelMetadataMap::new()),
                Some(meta_a.id.clone()),
            )
            .worker_id;
        let _wb = g
            .register_worker_for_owner(
                "local",
                reg_msg("box-b1", &["secret-model-b"], 4, ModelMetadataMap::new()),
                Some(meta_b.id.clone()),
            )
            .worker_id;
        w1
    };

    let app = ProxyHttpApp::new(Arc::clone(&core))
        .with_models_provider("local")
        .with_admin_token(Some("adm-secret".to_string()))
        .with_require_api_keys(true)
        .with_api_key_store(key_store)
        .with_routing_config_store(routing_store.clone())
        .router();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Unauthenticated /v1/routing returns 401; /admin/routing returns 403.
    let (st, _, _) = send_http(addr, "GET", "/v1/routing", &[], "").await;
    assert_eq!(st, 401);
    let (st, _, _) = send_http(addr, "GET", "/admin/routing", &[], "").await;
    assert_eq!(st, 403);

    // 2. Tenant A calls GET /v1/routing with their API key -> sees only Tenant A's workers & models!
    let auth_a = format!("Bearer {secret_a}");
    let (st, _, body) = send_http(
        addr,
        "GET",
        "/v1/routing",
        &[("Authorization", &auth_a)],
        "",
    )
    .await;
    assert_eq!(st, 200);
    let snap: RoutingStateResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(snap.workers.len(), 2);
    assert_eq!(snap.live_models.len(), 1);
    assert_eq!(snap.live_models[0].model_id, "qwen3");
    assert_eq!(snap.live_models[0].workers.len(), 2);

    // 3. Invalid config (wildcard "*" target) returns 400 Bad Request.
    let bad_req = UpdateRoutingConfigRequest {
        config: TenantRoutingConfig {
            groups: vec![ModelRoutingGroupConfig {
                id: "bad".to_string(),
                name: "Bad".to_string(),
                mode: ModelGroupMode::SharedPolicy,
                pool_model_id: None,
                targets: vec![ModelGroupTarget {
                    worker_name: None,
                    model_id: "*".to_string(),
                }],
                policy: RoutingPolicyConfig::default(),
            }],
            ..TenantRoutingConfig::default()
        },
    };
    let (st, _, _) = send_http(
        addr,
        "PUT",
        "/v1/routing",
        &[("Authorization", &auth_a)],
        &serde_json::to_string(&bad_req).unwrap(),
    )
    .await;
    assert_eq!(st, 400);

    // 4. Tenant A enables debug headers & persists config to FileRoutingConfigStore.
    let mut new_cfg = TenantRoutingConfig {
        expose_debug_headers: true,
        ..TenantRoutingConfig::default()
    };
    new_cfg.model_overrides.insert(
        "qwen3".to_string(),
        RoutingPolicyConfig {
            strategy: RoutingStrategyDto::PriorityFailover,
            worker_priority: vec!["box-a1".to_string(), "box-a2".to_string()],
            ..RoutingPolicyConfig::default()
        },
    );
    let (st, _, body) = send_http(
        addr,
        "PUT",
        "/v1/routing",
        &[("Authorization", &auth_a)],
        &serde_json::to_string(&UpdateRoutingConfigRequest { config: new_cfg }).unwrap(),
    )
    .await;
    assert_eq!(st, 200);
    let updated: RoutingStateResponse = serde_json::from_str(&body).unwrap();
    assert!(updated.config.expose_debug_headers);
    assert_eq!(
        updated.live_models[0].effective_policy.strategy,
        RoutingStrategyDto::PriorityFailover
    );

    // Verify Tenant B does NOT see Tenant A's config or workers!
    let auth_b = format!("Bearer {secret_b}");
    let (st, _, body_b) = send_http(
        addr,
        "GET",
        "/v1/routing",
        &[("Authorization", &auth_b)],
        "",
    )
    .await;
    assert_eq!(st, 200);
    let snap_b: RoutingStateResponse = serde_json::from_str(&body_b).unwrap();
    assert_eq!(snap_b.workers.len(), 1);
    assert_eq!(snap_b.workers[0].worker_name, "box-b1");
    assert!(!snap_b.config.expose_debug_headers);

    // Verify FileRoutingConfigStore persisted Tenant A's config to disk!
    let reloaded = routing_store.load_all().await.unwrap();
    let persisted = reloaded
        .get(&Some(meta_a.id.clone()))
        .expect("tenant A config persisted on disk");
    assert!(persisted.expose_debug_headers);
    assert!(persisted.model_overrides.contains_key("qwen3"));

    // 5. Inference request by Tenant A emits diagnostic X-ModelRelay-* headers!
    let core_bg = Arc::clone(&core);
    let w_a1_bg = w_a1.clone();
    let responder = tokio::spawn(async move {
        for _ in 0..100 {
            let pending = {
                let mut g = core_bg.lock().await;
                g.take_pending_worker_requests(&w_a1_bg)
            };
            if let Some(req) = pending.into_iter().next() {
                core_bg.lock().await.complete_http_response(
                    &w_a1_bg,
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
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("worker never received request");
    });

    let (st, resp_headers, _) = send_http(
        addr,
        "POST",
        "/v1/chat/completions",
        &[("Authorization", &auth_a)],
        &json!({"model": "qwen3", "messages": [{"role": "user", "content": "hi"}]}).to_string(),
    )
    .await;
    responder.await.unwrap();
    assert_eq!(st, 200);
    let lower = resp_headers.to_ascii_lowercase();
    assert!(
        lower.contains("x-modelrelay-worker: box-a1"),
        "headers: {resp_headers}"
    );
    assert!(
        lower.contains("x-modelrelay-route-reason: priority_primary"),
        "headers: {resp_headers}"
    );
    assert!(
        lower.contains("x-modelrelay-prefix-match-bytes: 0"),
        "headers: {resp_headers}"
    );

    let _ = tokio::fs::remove_file(&tmp_path).await;
}
