pub mod registration_harness;

/// Returns the pinned Katamari commit used as the starting behavior contract.
#[must_use]
pub fn source_behavior_commit() -> &'static str {
    "ab5e90f6a2ff05a063663ce478146bf0b6829429"
}

#[cfg(test)]
mod tests {
    use super::source_behavior_commit;
    use crate::registration_harness::{
        CloseCode, ConnectRequest, HandshakeFailure, ProviderConfig, RegisterAck, RegisterMessage,
        RegistrationHarness, ServerToWorker, WorkerToServer,
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
        assert!(doc.contains("Worker registration handshake"));
        assert!(doc.contains("Streaming pass-through"));
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
}
