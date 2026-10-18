use super::*;
use crate::compaction::{CompactionCleanup, CompactionScope, CompactionSource};

fn scope_app(root: &Path) -> AppState {
    let state_root = root.join("state");
    std::fs::create_dir_all(&state_root).unwrap();
    AppState::new_unrooted_configured(
        root.join("worktrees"),
        "main",
        true,
        crate::harness::HarnessContext {
            mcp_socket: root.join("mcp.sock"),
            state_root: state_root.clone(),
            bridge_exe: root.join("unused-bridge"),
        },
    )
    .with_config(state_root.join("config.json"))
    .unwrap()
    .with_task_store(state_root.join("tasks"))
    .unwrap()
}

fn call(state: &mut AppState, method: &str, params: Value) -> Value {
    let response = state.handle(req(method, params));
    assert_eq!(response["ok"], true, "{method}: {response}");
    response["result"].clone()
}

fn plain_project(state: &mut AppState, root: &Path, name: &str) -> (String, PathBuf) {
    let source = root.join(name);
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("notes.txt"), "source stays unchanged\n").unwrap();
    let id = state.add_project(source.clone(), "main".into());
    (id, source)
}

fn named_workspace(state: &mut AppState, project: &str, name: &str) -> String {
    call(
        state,
        "workspace.create",
        json!({"project_id": project, "name": name, "isolation": "worktree"}),
    )["workspace_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn workspace_owner(state: &mut AppState, workspace: &str) -> String {
    call(
        state,
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace}),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn project_owner(state: &mut AppState, project: &str) -> String {
    call(
        state,
        "project.ensure_conversation",
        json!({"project_id": project}),
    )["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn policy_agent(state: &mut AppState, owner: &str, provider: AgentProvider) -> String {
    state
        .agent_add(&json!({"entity_id": owner, "provider": provider.wire_id()}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn cell(provider: AgentProvider, tokens: Option<u64>) -> Value {
    json!({(provider.wire_id()): tokens})
}

fn all_cells(tokens: Option<u64>) -> Value {
    let mut patch = json!({});
    for provider in AgentProvider::ALL {
        patch[provider.wire_id()] = json!(tokens);
    }
    patch
}

fn assert_policy(
    state: &AppState,
    owner: &str,
    agent: &str,
    tokens: u64,
    source: CompactionSource,
) {
    let resolved = state.resolve_compaction_policy(owner, agent).unwrap();
    assert_eq!(resolved.tokens, tokens);
    assert_eq!(resolved.source, source);
}

fn config_on_disk(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("state/config.json")).unwrap()).unwrap()
}

fn project_policy(config: &Value, project: &str) -> Value {
    config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == project)
        .unwrap()["compaction_thresholds"]
        .clone()
}

#[test]
fn compaction_policy_all_harnesses_resolve_and_clear_every_inheritance_scope() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, _) = plain_project(&mut state, dir.path(), "source");
    let workspace = named_workspace(&mut state, &project, "working");
    let owner = workspace_owner(&mut state, &workspace);
    state
        .patch_compaction_thresholds(CompactionScope::Device, &all_cells(None))
        .unwrap();
    let scopes = [
        (CompactionScope::Device, CompactionSource::Device, 101_001),
        (
            CompactionScope::Project(project.clone()),
            CompactionSource::Project(project),
            202_002,
        ),
        (
            CompactionScope::Workspace(workspace.clone()),
            CompactionSource::Workspace(workspace),
            303_003,
        ),
    ];

    for provider in AgentProvider::ALL {
        let agent = policy_agent(&mut state, &owner, provider);
        assert_policy(&state, &owner, &agent, 200_000, CompactionSource::Default);
        for (scope, source, tokens) in &scopes {
            state
                .patch_compaction_thresholds(scope.clone(), &cell(provider, Some(*tokens)))
                .unwrap();
            let saved = state.compaction_thresholds(scope).unwrap();
            assert_eq!(saved.get(provider), Some(*tokens));
            assert_eq!(saved.stored_value()[provider.wire_id()], *tokens);
            assert_eq!(saved.read_value()[provider.wire_id()], *tokens);
            assert_policy(&state, &owner, &agent, *tokens, source.clone());
        }
        state
            .runs
            .get_mut(&owner)
            .unwrap()
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .max_context_tokens = Some(404_004);
        assert_policy(
            &state,
            &owner,
            &agent,
            404_004,
            CompactionSource::Conversation(agent.clone()),
        );
        state
            .runs
            .get_mut(&owner)
            .unwrap()
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .max_context_tokens = None;
        assert_policy(&state, &owner, &agent, 303_003, scopes[2].1.clone());
        for index in (0..scopes.len()).rev() {
            state
                .patch_compaction_thresholds(scopes[index].0.clone(), &cell(provider, None))
                .unwrap();
            let cleared = state.compaction_thresholds(&scopes[index].0).unwrap();
            assert_eq!(cleared.get(provider), None);
            assert!(cleared.stored_value().get(provider.wire_id()).is_none());
            assert_eq!(cleared.read_value()[provider.wire_id()], Value::Null);
            let (tokens, source) = index
                .checked_sub(1)
                .map(|before| (scopes[before].2, scopes[before].1.clone()))
                .unwrap_or((200_000, CompactionSource::Default));
            assert_policy(&state, &owner, &agent, tokens, source);
        }
    }
}

#[test]
fn compaction_policy_projects_and_workspaces_are_independent_and_project_workers_skip_workspaces() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (first, _) = plain_project(&mut state, dir.path(), "first");
    let (second, _) = plain_project(&mut state, dir.path(), "second");
    let first_workspace = named_workspace(&mut state, &first, "first-work");
    let sibling_workspace = named_workspace(&mut state, &first, "sibling-work");
    let second_workspace = named_workspace(&mut state, &second, "second-work");
    state
        .patch_compaction_thresholds(CompactionScope::Device, &all_cells(Some(100_000)))
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(first.clone()),
            &all_cells(Some(110_000)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(second.clone()),
            &all_cells(Some(120_000)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(first_workspace.clone()),
            &all_cells(Some(130_000)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(second_workspace.clone()),
            &all_cells(Some(140_000)),
        )
        .unwrap();
    let owners = [
        (
            workspace_owner(&mut state, &first_workspace),
            130_000,
            CompactionSource::Workspace(first_workspace),
        ),
        (
            workspace_owner(&mut state, &sibling_workspace),
            110_000,
            CompactionSource::Project(first.clone()),
        ),
        (
            workspace_owner(&mut state, &second_workspace),
            140_000,
            CompactionSource::Workspace(second_workspace),
        ),
        (
            project_owner(&mut state, &first),
            110_000,
            CompactionSource::Project(first),
        ),
        (
            project_owner(&mut state, &second),
            120_000,
            CompactionSource::Project(second),
        ),
    ];
    for (owner, tokens, source) in owners {
        for provider in AgentProvider::ALL {
            let agent = policy_agent(&mut state, &owner, provider);
            assert_policy(&state, &owner, &agent, tokens, source.clone());
        }
    }
}

#[test]
fn compaction_policy_scope_patches_are_atomic_on_invalid_input_and_persistence_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, _) = plain_project(&mut state, dir.path(), "source");
    let workspace = named_workspace(&mut state, &project, "working");
    let scopes = [
        CompactionScope::Device,
        CompactionScope::Project(project),
        CompactionScope::Workspace(workspace),
    ];
    let invalid = [
        json!(null),
        json!([]),
        json!(100),
        json!({"unknown": 100}),
        json!({"claude": 101, "pi": 0}),
        json!({"claude": 101, "pi": -1}),
        json!({"claude": 101, "pi": 1.5}),
        json!({"claude": 101, "pi": "200"}),
        json!({"claude": 101, "pi": true}),
        json!({"claude": 101, "pi": 9_007_199_254_740_992_u64}),
        json!({"claude": 101, "compact_above_tokens": 200}),
        json!({"claude": 101, "project_id": "other-project"}),
        json!({"claude": 101, "workspace_id": "other-workspace"}),
        json!({"claude": 101, "compaction_thresholds": {"pi": 200}}),
    ];
    for scope in scopes {
        state
            .patch_compaction_thresholds(scope.clone(), &all_cells(Some(123_456)))
            .unwrap();
        let before = state.compaction_thresholds(&scope).unwrap().stored_value();
        let disk = config_on_disk(dir.path());
        for patch in &invalid {
            assert!(
                state
                    .patch_compaction_thresholds(scope.clone(), patch)
                    .is_err(),
                "accepted {patch}"
            );
            assert_eq!(
                state.compaction_thresholds(&scope).unwrap().stored_value(),
                before
            );
            assert_eq!(config_on_disk(dir.path()), disk);
        }
        for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
            state.config_persist_failure = Some(failure);
            state
                .patch_compaction_thresholds(scope.clone(), &json!({}))
                .expect("an empty patch does not attempt persistence");
            assert!(state
                .patch_compaction_thresholds(scope.clone(), &all_cells(Some(456_789)))
                .is_err());
            assert_eq!(
                state.compaction_thresholds(&scope).unwrap().stored_value(),
                before
            );
            assert_eq!(config_on_disk(dir.path()), disk);
            assert!(!dir.path().join("state/config.tmp").exists());
            state.config_persist_failure = None;
        }
    }
    let disk = config_on_disk(dir.path());
    for scope in [
        CompactionScope::Project("missing-project".into()),
        CompactionScope::Workspace("missing-workspace".into()),
    ] {
        assert!(state
            .patch_compaction_thresholds(scope, &all_cells(Some(777)))
            .is_err());
        assert_eq!(config_on_disk(dir.path()), disk);
    }
}

#[test]
fn compaction_policy_safe_boundaries_accept_maps_and_scalar_without_overwriting_narrower_scopes() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, _) = plain_project(&mut state, dir.path(), "source");
    let workspace = named_workspace(&mut state, &project, "working");
    let scopes = [
        CompactionScope::Device,
        CompactionScope::Project(project.clone()),
        CompactionScope::Workspace(workspace.clone()),
    ];
    for tokens in [1, crate::compaction::MAX_SAFE_TOKENS] {
        for scope in &scopes {
            state
                .patch_compaction_thresholds(scope.clone(), &all_cells(Some(tokens)))
                .unwrap();
            let saved = state.compaction_thresholds(scope).unwrap();
            assert_eq!(saved.stored_value(), all_cells(Some(tokens)));
            assert_eq!(saved.read_value(), all_cells(Some(tokens)));
        }
    }
    let before = config_on_disk(dir.path());
    for tokens in [1, crate::compaction::MAX_SAFE_TOKENS] {
        state
            .settings_set(&json!({"compact_above_tokens": tokens}))
            .unwrap();
        let disk = config_on_disk(dir.path());
        assert_eq!(disk["compaction_thresholds"], all_cells(Some(tokens)));
        assert_eq!(disk["compact_above_tokens"], tokens);
        assert_eq!(
            project_policy(&disk, &project),
            project_policy(&before, &project)
        );
        assert_eq!(
            disk["workspace_compaction_thresholds"],
            before["workspace_compaction_thresholds"]
        );
        for scope in &scopes[1..] {
            assert_eq!(
                state.compaction_thresholds(scope).unwrap().stored_value(),
                all_cells(Some(crate::compaction::MAX_SAFE_TOKENS))
            );
        }
    }
}

#[test]
fn compaction_policy_unsafe_legacy_values_read_exactly_without_rounding_or_clamping() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, _) = plain_project(&mut state, dir.path(), "source");
    let owner = project_owner(&mut state, &project);
    let agent = policy_agent(&mut state, &owner, AgentProvider::Pi);
    state
        .settings_set(&json!({"compact_above_tokens": 123_456}))
        .unwrap();
    let config = config_on_disk(dir.path());
    drop(state);

    for tokens in [crate::compaction::MAX_SAFE_TOKENS + 1, u64::MAX] {
        for mapped in [false, true] {
            let mut saved = config.clone();
            saved["compact_above_tokens"] = json!(tokens);
            saved
                .as_object_mut()
                .unwrap()
                .remove("compaction_thresholds");
            if mapped {
                saved["compaction_thresholds"] = all_cells(Some(tokens));
            }
            std::fs::write(
                dir.path().join("state/config.json"),
                serde_json::to_vec(&saved).unwrap(),
            )
            .unwrap();
            let mut restarted = scope_app(dir.path());
            let thresholds = restarted
                .compaction_thresholds(&CompactionScope::Device)
                .unwrap();
            assert_eq!(thresholds.stored_value(), all_cells(Some(tokens)));
            for provider in AgentProvider::ALL {
                assert_eq!(thresholds.get(provider), Some(tokens));
                assert_eq!(
                    thresholds.read_value()[provider.wire_id()],
                    json!({"tokens_decimal": tokens.to_string()})
                );
            }
            let resolved = restarted.resolve_compaction_policy(&owner, &agent).unwrap();
            assert_eq!(resolved.tokens, tokens);
            assert_eq!(
                resolved.observation(),
                json!({
                    "tokens": null,
                    "tokens_decimal": tokens.to_string(),
                    "reason": "unsupported_representation",
                    "source": {"kind": "device"},
                })
            );
            restarted
                .settings_set(&json!({"watch_agent_filed_tasks": true}))
                .unwrap();
            assert_eq!(
                config_on_disk(dir.path())["compaction_thresholds"],
                all_cells(Some(tokens))
            );
        }
    }
}

#[test]
fn compaction_policy_restart_and_workspace_rename_keep_policy_and_source_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, source) = plain_project(&mut state, dir.path(), "source");
    let workspace = named_workspace(&mut state, &project, "original-name");
    let owner = workspace_owner(&mut state, &workspace);
    let agent = policy_agent(&mut state, &owner, AgentProvider::Pi);
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(project.clone()),
            &all_cells(Some(123_456)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(workspace.clone()),
            &cell(AgentProvider::Pi, Some(345_678)),
        )
        .unwrap();
    call(
        &mut state,
        "workspace.rename",
        json!({"workspace_id": workspace, "name": "renamed"}),
    );
    let before = config_on_disk(dir.path());
    drop(state);

    let mut restarted = scope_app(dir.path());
    assert_eq!(
        restarted
            .compaction_thresholds(&CompactionScope::Project(project))
            .unwrap()
            .stored_value(),
        all_cells(Some(123_456))
    );
    assert_policy(
        &restarted,
        &owner,
        &agent,
        345_678,
        CompactionSource::Workspace(workspace.clone()),
    );
    assert_eq!(
        call(
            &mut restarted,
            "workspace.get",
            json!({"workspace_id": workspace})
        )["name"],
        "renamed"
    );
    assert_eq!(config_on_disk(dir.path()), before);
    assert_eq!(
        std::fs::read_to_string(source.join("notes.txt")).unwrap(),
        "source stays unchanged\n"
    );
    assert!(!source.join(crate::workspace::MANIFEST_FILE).exists());
    assert!(!source.join(".build").exists());
    assert_eq!(std::fs::read_dir(source).unwrap().count(), 1);
}

#[test]
fn compaction_policy_parked_project_survives_unrelated_save_and_revival() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, source) = plain_project(&mut state, dir.path(), "source");
    let owner = project_owner(&mut state, &project);
    let agents = AgentProvider::ALL
        .into_iter()
        .map(|provider| policy_agent(&mut state, &owner, provider))
        .collect::<Vec<_>>();
    let workspace = named_workspace(&mut state, &project, "working");
    let workspace_run = workspace_owner(&mut state, &workspace);
    let workspace_agent = policy_agent(&mut state, &workspace_run, AgentProvider::Pi);
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(project.clone()),
            &all_cells(Some(123_456)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(workspace.clone()),
            &cell(AgentProvider::Pi, Some(234_567)),
        )
        .unwrap();
    drop(state);
    let away = dir.path().join("source-away");
    std::fs::rename(&source, &away).unwrap();
    let mut parked = scope_app(dir.path());
    assert!(parked.project(&project).is_none());
    assert!(parked.projects.project_id_of(&owner).is_none());
    assert!(parked.projects.project_id_of(&workspace_run).is_none());
    assert_policy(
        &parked,
        &workspace_run,
        &workspace_agent,
        234_567,
        CompactionSource::Workspace(workspace.clone()),
    );
    for agent in &agents {
        assert_policy(
            &parked,
            &owner,
            agent,
            123_456,
            CompactionSource::Project(project.clone()),
        );
    }
    parked
        .patch_compaction_thresholds(
            CompactionScope::Device,
            &cell(AgentProvider::Pi, Some(999_999)),
        )
        .unwrap();
    assert_eq!(
        project_policy(&config_on_disk(dir.path()), &project),
        all_cells(Some(123_456))
    );
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        cell(AgentProvider::Pi, Some(234_567))
    );
    drop(parked);
    std::fs::rename(away, source).unwrap();
    let mut revived = scope_app(dir.path());
    assert_eq!(project_owner(&mut revived, &project), owner);
    assert_policy(
        &revived,
        &workspace_run,
        &workspace_agent,
        234_567,
        CompactionSource::Workspace(workspace),
    );
    for agent in agents {
        assert_policy(
            &revived,
            &owner,
            &agent,
            123_456,
            CompactionSource::Project(project.clone()),
        );
    }
}

#[test]
fn compaction_policy_workspace_and_project_deletion_prune_only_owned_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (removed_project, source) = plain_project(&mut state, dir.path(), "removed-source");
    let (kept_project, kept_source) = plain_project(&mut state, dir.path(), "kept-source");
    let removed_workspace = named_workspace(&mut state, &removed_project, "remove-work");
    let sibling = named_workspace(&mut state, &removed_project, "sibling-work");
    let kept_workspace = named_workspace(&mut state, &kept_project, "keep-work");
    for scope in [
        CompactionScope::Project(removed_project.clone()),
        CompactionScope::Project(kept_project.clone()),
        CompactionScope::Workspace(removed_workspace.clone()),
        CompactionScope::Workspace(sibling.clone()),
        CompactionScope::Workspace(kept_workspace.clone()),
    ] {
        state
            .patch_compaction_thresholds(scope, &all_cells(Some(123_456)))
            .unwrap();
    }
    let before = config_on_disk(dir.path());
    call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id": removed_workspace, "locked": true}),
    );
    let refused = state.handle(req(
        "workspace.delete",
        json!({"workspace_id": removed_workspace}),
    ));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(config_on_disk(dir.path()), before);
    assert_eq!(
        state
            .compaction_thresholds(&CompactionScope::Workspace(removed_workspace.clone()))
            .unwrap()
            .stored_value(),
        all_cells(Some(123_456))
    );
    call(
        &mut state,
        "workspace.set_locked",
        json!({"workspace_id": removed_workspace, "locked": false}),
    );
    call(
        &mut state,
        "workspace.delete",
        json!({"workspace_id": removed_workspace}),
    );
    let disk = config_on_disk(dir.path());
    assert!(disk["workspace_compaction_thresholds"]
        .get(&removed_workspace)
        .is_none());
    assert_eq!(
        disk["workspace_compaction_thresholds"][&sibling],
        all_cells(Some(123_456))
    );
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        let refused = state.handle(req(
            "project.delete",
            json!({"project_id": removed_project, "confirm": true}),
        ));
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(config_on_disk(dir.path()), disk);
        for scope in [
            CompactionScope::Project(removed_project.clone()),
            CompactionScope::Workspace(sibling.clone()),
        ] {
            assert_eq!(
                state.compaction_thresholds(&scope).unwrap().stored_value(),
                all_cells(Some(123_456))
            );
        }
        state.config_persist_failure = None;
    }
    call(
        &mut state,
        "project.delete",
        json!({"project_id": removed_project, "confirm": true}),
    );
    let disk = config_on_disk(dir.path());
    assert!(disk["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["id"] != removed_project));
    assert!(disk["workspace_compaction_thresholds"]
        .get(&sibling)
        .is_none());
    assert_eq!(
        disk["workspace_compaction_thresholds"][&kept_workspace],
        all_cells(Some(123_456))
    );
    assert_eq!(
        project_policy(&disk, &kept_project),
        all_cells(Some(123_456))
    );
    drop(state);
    let restarted = scope_app(dir.path());
    assert_eq!(
        restarted
            .compaction_thresholds(&CompactionScope::Workspace(kept_workspace))
            .unwrap()
            .get(AgentProvider::Pi),
        Some(123_456)
    );
    for path in [source, kept_source] {
        assert_eq!(
            std::fs::read_to_string(path.join("notes.txt")).unwrap(),
            "source stays unchanged\n"
        );
        assert_eq!(std::fs::read_dir(path).unwrap().count(), 1);
    }
}

#[test]
fn compaction_policy_adopted_checkout_uses_workspace_scope_and_prunes_it_when_gone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let checkout = dir.path().join("external");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "external",
            checkout.to_str().unwrap(),
        ],
    );
    let original_git_config = std::fs::read(repo.join(".git/config")).unwrap();
    let mut state = scope_app(dir.path());
    let project = state.add_project(repo.clone(), "main".into());
    let workspace = crate::worktree::external_worktree_id(&checkout);
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(!state.workspaces.get(&workspace).unwrap().managed);
    state
        .patch_compaction_thresholds(
            CompactionScope::Project(project.clone()),
            &all_cells(Some(123_456)),
        )
        .unwrap();
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(workspace.clone()),
            &all_cells(Some(234_567)),
        )
        .unwrap();
    assert!(!checkout.join(crate::workspace::MANIFEST_FILE).exists());
    assert_eq!(
        std::fs::read(repo.join(".git/config")).unwrap(),
        original_git_config
    );
    drop(state);
    let mut restarted = scope_app(dir.path());
    call(
        &mut restarted,
        "workspace.list",
        json!({"project_id": project}),
    );
    assert_eq!(
        restarted
            .compaction_thresholds(&CompactionScope::Workspace(workspace.clone()))
            .unwrap()
            .get(AgentProvider::Pi),
        Some(234_567)
    );
    // Restart with the stable external-workspace ID. Re-identification after
    // persisting its conversation is tracked separately by #654.
    let owner = workspace_owner(&mut restarted, &workspace);
    for provider in AgentProvider::ALL {
        let agent = policy_agent(&mut restarted, &owner, provider);
        assert_policy(
            &restarted,
            &owner,
            &agent,
            234_567,
            CompactionSource::Workspace(workspace.clone()),
        );
    }
    assert!(!checkout.join(crate::workspace::MANIFEST_FILE).exists());
    std::fs::remove_dir_all(&checkout).unwrap();
    call(
        &mut restarted,
        "workspace.list",
        json!({"project_id": project}),
    );
    assert!(restarted.workspaces.get(&workspace).is_none());
    assert!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"]
            .get(&workspace)
            .is_none()
    );
    assert_eq!(
        project_policy(&config_on_disk(dir.path()), &project),
        all_cells(Some(123_456))
    );
    assert_eq!(
        std::fs::read(repo.join(".git/config")).unwrap(),
        original_git_config
    );
}

#[test]
fn compaction_policy_workspace_delete_preflight_preserves_files_when_config_is_unwritable() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = scope_app(dir.path());
    let (project, source) = plain_project(&mut state, dir.path(), "source");
    let workspace = named_workspace(&mut state, &project, "working");
    let scope = CompactionScope::Workspace(workspace.clone());
    state
        .patch_compaction_thresholds(scope.clone(), &all_cells(Some(123_456)))
        .unwrap();
    let root = state.workspaces.get(&workspace).unwrap().root.clone();
    let manifest = root.join(crate::workspace::MANIFEST_FILE);
    let manifest_before = std::fs::read(&manifest).unwrap();
    let disk_before = config_on_disk(dir.path());
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        let refused = state.handle(req("workspace.delete", json!({"workspace_id": workspace})));
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(std::fs::read(&manifest).unwrap(), manifest_before);
        assert_eq!(
            std::fs::read_to_string(source.join("notes.txt")).unwrap(),
            "source stays unchanged\n"
        );
        assert_eq!(
            state.compaction_thresholds(&scope).unwrap().stored_value(),
            all_cells(Some(123_456))
        );
        assert_eq!(config_on_disk(dir.path()), disk_before);
        assert!(state.deferred_work.is_none());
    }
    state.config_persist_failure = None;
    call(
        &mut state,
        "workspace.delete",
        json!({"workspace_id": workspace}),
    );
    assert!(!root.exists());
    assert!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"]
            .get(&workspace)
            .is_none()
    );
}

#[test]
fn compaction_policy_late_cleanup_failure_keeps_successful_delete_and_can_be_retried() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scope_app(dir.path());
        let (project, source) = plain_project(&mut state, dir.path(), "source");
        let workspace = named_workspace(&mut state, &project, "working");
        state
            .patch_compaction_thresholds(
                CompactionScope::Workspace(workspace.clone()),
                &all_cells(Some(123_456)),
            )
            .unwrap();
        let root = state.workspaces.get(&workspace).unwrap().root.clone();
        state
            .workspace_delete(&json!({"workspace_id": workspace}))
            .unwrap();
        let Some(DeferredWork::Git(git)) = state.deferred_work.take() else {
            panic!("workspace deletion must prepare a filesystem job");
        };
        let result = git.call.run(&git.params);
        assert!(result.is_ok(), "{result:?}");
        assert!(!root.exists());
        state.config_persist_failure = Some(failure);
        let deleted = state
            .apply_git(&git, result)
            .expect("policy cleanup cannot undo a completed deletion");
        assert_eq!(deleted, json!({"workspace_id": workspace, "deleted": true}));
        assert!(state.workspaces.get(&workspace).is_none());
        assert_eq!(
            config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
            all_cells(Some(123_456))
        );
        assert_eq!(
            std::fs::read_to_string(source.join("notes.txt")).unwrap(),
            "source stays unchanged\n"
        );
        match state.cleanup_deleted_workspace_compaction(std::slice::from_ref(&workspace)) {
            CompactionCleanup::Retained { error } => assert!(!error.is_empty()),
            CompactionCleanup::Complete => {
                panic!("unwritable policy cleanup must report retained settings")
            }
        }
        state.config_persist_failure = None;
        assert!(matches!(
            state.cleanup_deleted_workspace_compaction(std::slice::from_ref(&workspace)),
            CompactionCleanup::Complete
        ));
        assert!(
            config_on_disk(dir.path())["workspace_compaction_thresholds"]
                .get(&workspace)
                .is_none()
        );
    }
}

/// Checkout IDs derive from paths. If cleanup cannot persist, reusing that
/// path also reuses its retained policy; tombstone handling belongs to #654.
#[test]
fn compaction_policy_failed_adopted_cleanup_retains_policy_for_same_path_readoption() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let checkout = dir.path().join("external");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "external",
            checkout.to_str().unwrap(),
        ],
    );
    let mut state = scope_app(dir.path());
    let project = state.add_project(repo.clone(), "main".into());
    let workspace = crate::worktree::external_worktree_id(&checkout);
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(state
        .workspace_conversation_owner(state.workspaces.get(&workspace).unwrap())
        .is_none());
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(workspace.clone()),
            &cell(AgentProvider::Pi, Some(234_567)),
        )
        .unwrap();
    std::fs::remove_dir_all(&checkout).unwrap();
    state.config_persist_failure = Some(ConfigPersistStep::Write);
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(state.workspaces.get(&workspace).is_none());
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        cell(AgentProvider::Pi, Some(234_567))
    );
    git_in(&repo, &["worktree", "prune"]);
    git_in(
        &repo,
        &["worktree", "add", checkout.to_str().unwrap(), "external"],
    );
    state.config_persist_failure = None;
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert_eq!(crate::worktree::external_worktree_id(&checkout), workspace);
    assert_eq!(
        state
            .compaction_thresholds(&CompactionScope::Workspace(workspace.clone()))
            .unwrap()
            .get(AgentProvider::Pi),
        Some(234_567)
    );
    assert!(state
        .workspace_conversation_owner(state.workspaces.get(&workspace).unwrap())
        .is_none());
    assert!(!checkout.join(crate::workspace::MANIFEST_FILE).exists());
}
