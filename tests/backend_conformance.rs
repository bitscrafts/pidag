//! Conformance test battery for agent backends (G10).
//!
//! This battery is shared across all registered backends.
//! Each backend declares capabilities, and this test verifies:
//! - Each declared capability actually works
//! - Each undeclared capability returns Unsupported (not panic/silent no-op)

use pidag::{AgentBackend, BackendRegistry, SessionSpec, ThinkingLevel, backend::ModelRef};

/// Run conformance tests for a single backend.
/// Verifies that declared capabilities work and undeclared ones return Unsupported.
async fn conformance_for_backend(backend: &dyn AgentBackend) {
    let name = backend.name();
    let caps = backend.capabilities();

    // Test: sessions capability
    if caps.sessions {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session with sessions=true: {}", name, e));

        // Should be able to prompt
        let reply = session
            .prompt("test prompt")
            .await
            .unwrap_or_else(|e| panic!("{}: can prompt with sessions=true: {}", name, e));
        assert!(!reply.text.is_empty());

        session
            .close()
            .await
            .unwrap_or_else(|e| panic!("{}: can close session: {}", name, e));
    } else {
        // Without sessions, opening a session should still work (it just won't be persistent)
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session with sessions=false: {}", name, e));

        let reply = session
            .prompt("test prompt")
            .await
            .unwrap_or_else(|e| panic!("{}: can prompt even without sessions: {}", name, e));
        assert!(!reply.text.is_empty());

        session
            .close()
            .await
            .unwrap_or_else(|e| panic!("{}: can close session: {}", name, e));
    }

    // Test: model_switch capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test1".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session for model_switch test: {}", name, e));

        let new_model = ModelRef {
            name: "test2".to_string(),
            paid: false,
        };

        let result = session.set_model(&new_model).await;
        if caps.model_switch {
            result.unwrap_or_else(|e| {
                panic!("{}: set_model works with model_switch=true: {}", name, e)
            });
        } else {
            assert!(
                result.is_err(),
                "{}: set_model should return error with model_switch=false",
                name
            );
        }

        session.close().await.unwrap_or_else(|e| {
            panic!("{}: can close session after model_switch test: {}", name, e)
        });
    }

    // Test: thinking_levels capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend.open_session(spec).await.unwrap_or_else(|e| {
            panic!("{}: can open session for thinking_levels test: {}", name, e)
        });

        let result = session.set_thinking(ThinkingLevel::High).await;
        if caps.thinking_levels {
            result.unwrap_or_else(|e| {
                panic!(
                    "{}: set_thinking works with thinking_levels=true: {}",
                    name, e
                )
            });
        } else {
            assert!(
                result.is_err(),
                "{}: set_thinking should return error with thinking_levels=false",
                name
            );
        }

        session.close().await.unwrap_or_else(|e| {
            panic!(
                "{}: can close session after thinking_levels test: {}",
                name, e
            )
        });
    }

    // Test: fork capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session for fork test: {}", name, e));

        let result = session.fork().await;
        if caps.fork {
            let mut forked =
                result.unwrap_or_else(|e| panic!("{}: fork works with fork=true: {}", name, e));
            forked
                .close()
                .await
                .unwrap_or_else(|e| panic!("{}: can close forked session: {}", name, e));
        } else {
            assert!(
                result.is_err(),
                "{}: fork should return error with fork=false",
                name
            );
        }

        session
            .close()
            .await
            .unwrap_or_else(|e| panic!("{}: can close session after fork test: {}", name, e));
    }

    // Test: compact capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session for compact test: {}", name, e));

        let result = session.compact().await;
        if caps.compact {
            result.unwrap_or_else(|e| panic!("{}: compact works with compact=true: {}", name, e));
        } else {
            assert!(
                result.is_err(),
                "{}: compact should return error with compact=false",
                name
            );
        }

        session
            .close()
            .await
            .unwrap_or_else(|e| panic!("{}: can close session after compact test: {}", name, e));
    }

    // Test: token_usage capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session for token_usage test: {}", name, e));

        let result = session.usage().await;
        if caps.token_usage {
            let usage = result
                .unwrap_or_else(|e| panic!("{}: usage works with token_usage=true: {}", name, e));
            assert!(usage.total_tokens > 0 || usage.input_tokens > 0 || usage.output_tokens > 0);
        } else {
            assert!(
                result.is_err(),
                "{}: usage should return error with token_usage=false",
                name
            );
        }
    }

    // Test: cancellation capability
    {
        let spec = SessionSpec {
            model: ModelRef {
                name: "test".to_string(),
                paid: false,
            },
            thinking_level: ThinkingLevel::Low,
        };

        let mut session = backend
            .open_session(spec)
            .await
            .unwrap_or_else(|e| panic!("{}: can open session for cancellation test: {}", name, e));

        let result = session.abort().await;
        if caps.cancellation {
            result
                .unwrap_or_else(|e| panic!("{}: abort works with cancellation=true: {}", name, e));
        } else {
            assert!(
                result.is_err(),
                "{}: abort should return error with cancellation=false",
                name
            );
        }

        session.close().await.unwrap_or_else(|e| {
            panic!("{}: can close session after cancellation test: {}", name, e)
        });
    }
}

/// G10: Conformance suite runs all registered backends.
#[tokio::test]
async fn test_conformance_suite_runs_all_registered() {
    let registry = BackendRegistry::default();
    let backend_names = registry.names();

    assert!(
        !backend_names.is_empty(),
        "Registry should have at least one backend"
    );

    for name in backend_names {
        let backend = registry
            .get(name)
            .unwrap_or_else(|| panic!("Backend {} should be in registry", name));

        // Run conformance tests for this backend
        conformance_for_backend(backend).await;
    }
}
