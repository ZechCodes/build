use super::*;

fn policy_app(root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        root.join("worktrees"),
        "main",
        true,
        crate::harness::HarnessContext {
            mcp_socket: root.join("mcp.sock"),
            state_root: root.join("state"),
            bridge_exe: root.join("unused-bridge"),
        },
    )
}

fn saved_config(state: &AppState) -> Value {
    state.config_value(&state.projects_dir, state.default_harness, state.isolation)
}

#[test]
fn compaction_policy_migrates_legacy_once_and_preserves_an_empty_map() {
    for (legacy, expected) in [(None, 200_000), (Some(0), 200_000), (Some(123_456), 123_456)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = json!({});
        if let Some(tokens) = legacy {
            config["compact_above_tokens"] = json!(tokens);
        }
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut state = policy_app(dir.path()).with_config(&path).unwrap();
        let migrated = saved_config(&state);
        for provider in AgentProvider::ALL {
            assert_eq!(migrated["compaction_thresholds"][provider.wire_id()], expected);
        }
        state.settings_set(&json!({"watch_agent_filed_tasks": true})).unwrap();
        let restarted = policy_app(dir.path()).with_config(&path).unwrap();
        assert_eq!(saved_config(&restarted), saved_config(&state));
        config["compaction_thresholds"] = json!({});
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let empty = policy_app(dir.path()).with_config(&path).unwrap();
        assert_eq!(saved_config(&empty)["compaction_thresholds"], json!({}));
    }
}

#[test]
fn compaction_policy_old_scalar_broadcast_is_atomic_and_rejects_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = policy_app(dir.path()).with_config(dir.path().join("config.json")).unwrap();
    state.settings_set(&json!({"compact_above_tokens": 123_456})).unwrap();
    for provider in AgentProvider::ALL {
        assert_eq!(saved_config(&state)["compaction_thresholds"][provider.wire_id()], 123_456);
    }
    let before = saved_config(&state);
    for invalid in [json!(0), json!(-1), json!(1.5), json!("123"), json!(9_007_199_254_740_992_u64)] {
        assert!(state.settings_set(&json!({"compact_above_tokens": invalid})).is_err());
        assert_eq!(saved_config(&state), before);
    }
    assert!(state.settings_set(&json!({"compact_above_tokens": 100, "compaction_thresholds": {}})).is_err());
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        assert!(state.settings_set(&json!({"compact_above_tokens": 999})).is_err());
        assert_eq!(saved_config(&state), before);
        let on_disk: Value = serde_json::from_slice(&std::fs::read(dir.path().join("config.json")).unwrap()).unwrap();
        assert_eq!(on_disk, before);
    }
}
