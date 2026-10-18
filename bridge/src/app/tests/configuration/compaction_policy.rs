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
    for (legacy, expected) in [
        (None, 200_000),
        (Some(0), 200_000),
        (Some(123_456), 123_456),
    ] {
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
            assert_eq!(
                migrated["compaction_thresholds"][provider.wire_id()],
                expected
            );
        }
        state
            .settings_set(&json!({"watch_agent_filed_tasks": true}))
            .unwrap();
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
    let mut state = policy_app(dir.path())
        .with_config(dir.path().join("config.json"))
        .unwrap();
    state
        .settings_set(&json!({"compact_above_tokens": 123_456}))
        .unwrap();
    for provider in AgentProvider::ALL {
        assert_eq!(
            saved_config(&state)["compaction_thresholds"][provider.wire_id()],
            123_456
        );
    }
    let before = saved_config(&state);
    for invalid in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("123"),
        json!(9_007_199_254_740_992_u64),
    ] {
        assert!(state
            .settings_set(&json!({"compact_above_tokens": invalid}))
            .is_err());
        assert_eq!(saved_config(&state), before);
    }
    assert!(state
        .settings_set(&json!({"compact_above_tokens": 100, "compaction_thresholds": {}}))
        .is_err());
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        assert!(state
            .settings_set(&json!({"compact_above_tokens": 999}))
            .is_err());
        assert_eq!(saved_config(&state), before);
        let on_disk: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("config.json")).unwrap())
                .unwrap();
        assert_eq!(on_disk, before);
    }
}

#[test]
fn compaction_policy_keeps_out_of_range_legacy_integers_exact() {
    for tokens in [9_007_199_254_740_992_u64, u64::MAX] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, format!("{{\"compact_above_tokens\":{tokens}}}")).unwrap();
        let state = policy_app(dir.path()).with_config(&path).unwrap();
        let config = saved_config(&state);
        assert_eq!(config["compact_above_tokens"].as_u64(), Some(tokens));
        for provider in AgentProvider::ALL {
            assert_eq!(
                config["compaction_thresholds"][provider.wire_id()].as_u64(),
                Some(tokens)
            );
        }
    }
}

#[test]
fn compaction_policy_legacy_conversation_zero_deserializes_as_inheritance() {
    let agent = crate::agent::Agent::new("agent-test", "owner", ModelChoice::default(), 1, "now");
    let mut saved = serde_json::to_value(agent).unwrap();
    for (stored, expected) in [
        (Some(0), None),
        (Some(150_000), Some(150_000)),
        (None, None),
    ] {
        saved["max_context_tokens"] = json!(stored);
        let restored: crate::agent::Agent = serde_json::from_value(saved.clone()).unwrap();
        assert_eq!(restored.max_context_tokens, expected);
    }
}

#[test]
fn compaction_policy_aliases_share_override_and_canonical_scopes_but_use_launch_provider() {
    use crate::compaction::{CompactionScope, CompactionSource};
    let dir = tempfile::tempdir().unwrap();
    let mut state = policy_app(dir.path());
    let project = state.add_project(init_repo_named(dir.path(), "canonical"), "main".into());
    let other = state.add_project(init_repo_named(dir.path(), "alias"), "main".into());
    let (owner, canonical) = super::super::project_agent::project_agent(&mut state, &project);
    let (alias_owner, alias) = super::super::project_agent::project_agent(&mut state, &other);
    state.edit_agent_record("canonical test provider", &owner, &canonical, |agent| {
        agent.choice.provider = AgentProvider::ClaudeAdk;
    });
    state.edit_agent_record("alias test binding", &alias_owner, &alias, |agent| {
        agent.bind_conversation(&canonical);
        agent.choice.provider = AgentProvider::ClaudeAdk;
        agent.max_context_tokens = Some(1); // Old alias-local state must not win.
    });
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(project.clone()),
            &json!({"claude_adk": 123_456, "codex_app_server": 234_567}),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(other),
            &json!({"claude_adk": 2, "codex_app_server": 3}),
        )
        .unwrap();
    let expected = state.resolve_compaction_policy(&owner, &canonical).unwrap();
    assert_eq!(expected.tokens, 123_456);
    assert_eq!(expected.source, CompactionSource::Project(project.clone()));
    assert_eq!(
        state
            .resolve_compaction_policy(&alias_owner, &alias)
            .unwrap(),
        expected
    );
    state.edit_agent_record("alias test provider", &alias_owner, &alias, |agent| {
        agent.choice.provider = AgentProvider::CodexAppServer;
    });
    let distinct = state
        .resolve_compaction_policy(&alias_owner, &alias)
        .unwrap();
    assert_eq!(distinct.tokens, 234_567);
    assert_eq!(distinct.source, CompactionSource::Project(project));
    let subscriber = crate::carrier::SessionSender::detached("alias-policy-subscriber");
    state.changes.subscribe(
        &subscriber,
        crate::changes::SubscriptionSpec {
            id: "alias-policy".into(),
            scope: crate::changes::Scope::Entity(alias_owner.clone()),
            kinds: [crate::changes::Kind::State].into_iter().collect(),
            mode: crate::changes::Mode::Off,
            priority: crate::changes::Priority::Foreground,
        },
    );
    assert!(!state.changes.has_pending());
    state
        .set_conversation_context_limit(&alias_owner, &alias, Some(345_678))
        .unwrap();
    assert!(
        state.changes.has_pending(),
        "an alias subscriber must hear the canonical edit"
    );
    assert_eq!(
        state
            .entity_agents(&owner)
            .unwrap()
            .by_id(&canonical)
            .unwrap()
            .max_context_tokens,
        Some(345_678)
    );
    for (entity, agent) in [(&owner, &canonical), (&alias_owner, &alias)] {
        let resolved = state.resolve_compaction_policy(entity, agent).unwrap();
        assert_eq!(resolved.tokens, 345_678);
        assert_eq!(
            resolved.source,
            CompactionSource::Conversation(canonical.clone())
        );
        let listed = state.handle(req("agent.list", json!({"entity_id": entity})));
        assert_eq!(listed["ok"], true, "{listed:?}");
        assert_eq!(listed["result"]["agents"][0]["max_context_tokens"], 345_678);
    }
    for invalid in [0, 9_007_199_254_740_992] {
        assert!(state
            .set_conversation_context_limit(&alias_owner, &alias, Some(invalid))
            .is_err());
    }
    state
        .set_conversation_context_limit(&alias_owner, &alias, None)
        .unwrap();
    assert_eq!(
        state
            .resolve_compaction_policy(&owner, &canonical)
            .unwrap()
            .tokens,
        123_456
    );
    assert_eq!(
        state
            .resolve_compaction_policy(&alias_owner, &alias)
            .unwrap()
            .tokens,
        234_567
    );
}
