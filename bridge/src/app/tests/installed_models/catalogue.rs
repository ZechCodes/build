//! Shared resolver preserves picker visibility, role startability and settings.
use super::*;
use crate::models::{AgentCapability, AgentProvider, AgentRole, RoleModel, RoleModels};

#[test]
fn harness_catalogue_keeps_hidden_role_startability_and_wire_order() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock")
        .with_cli_readings(Readings::answering_inline(InstalledClis::leaked("2.1.280")));
    state.default_harness = AgentProvider::Codex;
    let reviewer = |model: &str, capability| RoleModel {
        provider: None,
        model: model.into(),
        roles: vec![AgentRole::Reviewer],
        capability,
    };
    state.role_models = RoleModels(vec![
        reviewer("not-listed", AgentCapability::Generalist),
        reviewer("gpt-reserve", AgentCapability::Scoped),
        reviewer("gpt-6-sol", AgentCapability::Generalist),
    ]);
    let table = state.harness_table();
    assert_eq!(table["default_harness"], "codex");
    assert_eq!(table["role_models"], json!(state.role_models));
    assert_eq!(table["roles_in_effect"]["reviewer"]["provider"], "codex");
    assert_eq!(table["roles_in_effect"]["reviewer"]["model"], "gpt-reserve");
    assert_eq!(table["roles_in_effect"]["reviewer"]["capability"], "scoped");
    assert_eq!(
        state
            .role_model_here(AgentRole::Reviewer, Some(AgentCapability::Generalist))
            .unwrap()
            .model,
        "gpt-6-sol"
    );
    let codex = table["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "codex")
        .unwrap();
    assert_eq!(
        model_ids(codex),
        ["gpt-6-sol"],
        "hidden may start but never appears in picker"
    );
    assert_eq!(
        table["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        AgentProvider::ALL.map(AgentProvider::wire_id)
    );
    assert_eq!(
        table["roles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        AgentRole::ALL.map(AgentRole::wire_id)
    );
    assert_eq!(
        table["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        AgentCapability::ALL.map(AgentCapability::wire_id)
    );
}

#[test]
fn harness_catalogue_observes_updated_cli_readings_without_a_new_settings_capture() {
    let clis = InstalledClis::leaked("2.1.280");
    let readings = Readings::answering_inline(clis);
    let (dir, repo) = init_repo();
    let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock")
        .with_cli_readings(Arc::clone(&readings));
    state.default_harness = AgentProvider::ClaudeAdk;
    state.role_models = reviewers_sonnet_5_5_then_opus_5_5();
    let before = state.harness_table();
    assert_eq!(
        before["roles_in_effect"]["reviewer"]["model"],
        "claude-opus-5-5"
    );
    claude_now_at(&readings, clis, "2.1.284");
    let after = state.harness_table();
    assert_eq!(
        after["roles_in_effect"]["reviewer"]["model"],
        "claude-sonnet-5-5"
    );
    let claude = after["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "claude_adk")
        .unwrap();
    assert_eq!(claude["cli_version"], "2.1.284");
    assert_eq!(claude["unavailable"], json!([]));
}

#[test]
fn harness_catalogue_failed_settings_keep_the_committed_default_and_roles() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock")
        .with_config(dir.path().join("config.json"))
        .unwrap()
        .with_cli_readings(Readings::answering_inline(InstalledClis::leaked("2.1.284")));
    state.initialize_snapshot_projection();
    state.initialize_read_context();
    state.settings_set(&json!({"default_harness":"codex","role_models":[{"model":"gpt-reserve","roles":["reviewer"],"capability":"scoped"}]})).unwrap();
    let committed = state.harness_table();
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        assert!(state.settings_set(&json!({"default_harness":"claude_adk","role_models":[{"model":"claude-opus-5-5","roles":["reviewer"],"capability":"generalist"}]})).is_err());
        state.config_persist_failure = None;
        assert_eq!(state.harness_table(), committed);
        state
            .settings_set(&json!({"workspace_idle_secs":1234}))
            .unwrap();
        assert_eq!(
            state.harness_table(),
            committed,
            "later success cannot publish failed settings"
        );
    }
}
