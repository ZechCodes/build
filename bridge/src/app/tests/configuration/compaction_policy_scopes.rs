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
            channel: crate::config::Channel::Prod,
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
            let wire = call(&mut restarted, "settings.get", json!({}));
            for provider in AgentProvider::ALL {
                assert_eq!(
                    wire["compaction_thresholds"][provider.wire_id()],
                    json!({"tokens_decimal":tokens.to_string()})
                );
                assert_eq!(
                    wire["compaction"]["effective"][provider.wire_id()],
                    resolved.observation()
                );
            }
            let scoped = call(
                &mut restarted,
                "project.set_compaction",
                json!({"project_id":project,"compaction_thresholds":{}}),
            );
            assert_eq!(
                scoped["compaction"]["inherited"]["pi"],
                resolved.observation()
            );

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
    let before_wire_refusal = config_on_disk(dir.path());
    let refused = parked.handle(req(
        "project.set_compaction",
        json!({"project_id":project,"compaction_thresholds":{"pi":12}}),
    ));
    assert_eq!(refused["error_code"], "not_found");
    assert_eq!(config_on_disk(dir.path()), before_wire_refusal);

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

fn assert_locked_workspace_policy_retained(state: &mut AppState, root: &Path, workspace: &str) {
    let before = config_on_disk(root);
    call(
        state,
        "workspace.set_locked",
        json!({"workspace_id": workspace, "locked": true}),
    );
    let refused = state.handle(req("workspace.delete", json!({"workspace_id": workspace})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(config_on_disk(root), before);
    assert_eq!(
        state
            .compaction_thresholds(&CompactionScope::Workspace(workspace.into()))
            .unwrap()
            .stored_value(),
        all_cells(Some(123_456))
    );
    call(
        state,
        "workspace.set_locked",
        json!({"workspace_id": workspace, "locked": false}),
    );
}

fn assert_project_policy_retained_on_failed_delete(
    state: &mut AppState,
    root: &Path,
    project: &str,
    workspace: &str,
) {
    let disk = config_on_disk(root);
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        state.config_persist_failure = Some(failure);
        let refused = state.handle(req(
            "project.delete",
            json!({"project_id": project, "confirm": true}),
        ));
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(config_on_disk(root), disk);
        for scope in [
            CompactionScope::Project(project.into()),
            CompactionScope::Workspace(workspace.into()),
        ] {
            assert_eq!(
                state.compaction_thresholds(&scope).unwrap().stored_value(),
                all_cells(Some(123_456))
            );
        }
        state.config_persist_failure = None;
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
    assert_locked_workspace_policy_retained(&mut state, dir.path(), &removed_workspace);
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
    assert_project_policy_retained_on_failed_delete(
        &mut state,
        dir.path(),
        &removed_project,
        &sibling,
    );
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
fn compaction_policy_adopted_checkout_uses_workspace_scope_and_keeps_it_when_gone() {
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
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        all_cells(Some(234_567))
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

/// A missing checkout is not confirmed deletion. Its deterministic path ID
/// recovers the configured policy when the checkout returns.
#[test]
fn compaction_policy_missing_adopted_checkout_retains_policy_across_save_restart_and_return() {
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
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(state.workspaces.get(&workspace).is_none());
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        cell(AgentProvider::Pi, Some(234_567))
    );
    state
        .settings_set(&json!({"watch_agent_filed_tasks": true}))
        .unwrap();
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        cell(AgentProvider::Pi, Some(234_567))
    );
    drop(state);
    let mut state = scope_app(dir.path());
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

#[cfg(unix)]
#[test]
fn compaction_policy_inaccessible_adopted_checkout_keeps_policy_until_access_returns() {
    use std::os::unix::fs::PermissionsExt;

    struct RestorePermissions {
        path: PathBuf,
        permissions: std::fs::Permissions,
    }
    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.path, self.permissions.clone())
                .expect("restore the fixture directory's original permissions");
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "source");
    let parent = dir.path().join("external-parent");
    std::fs::create_dir(&parent).unwrap();
    let checkout = parent.join("checkout");
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
    let contents = std::fs::read(checkout.join("README.md")).unwrap();
    let mut state = scope_app(dir.path());
    let project = state.add_project(repo, "main".into());
    let workspace = crate::worktree::external_worktree_id(&checkout);
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(state.workspaces.get(&workspace).is_some());
    state
        .patch_compaction_thresholds(
            CompactionScope::Workspace(workspace.clone()),
            &cell(AgentProvider::Pi, Some(234_567)),
        )
        .unwrap();

    let permissions = std::fs::metadata(&parent).unwrap().permissions();
    let original_mode = permissions.mode();
    let restore = RestorePermissions {
        path: parent.clone(),
        permissions,
    };
    std::fs::set_permissions(
        &parent,
        std::fs::Permissions::from_mode(original_mode & !0o111),
    )
    .unwrap();
    let denied = std::fs::metadata(&checkout)
        .expect_err("fixture must deny traversal; run this test as an unprivileged user");
    assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert!(state.workspaces.get(&workspace).is_none());
    assert_eq!(
        config_on_disk(dir.path())["workspace_compaction_thresholds"][&workspace],
        cell(AgentProvider::Pi, Some(234_567))
    );

    drop(restore);
    assert_eq!(
        std::fs::metadata(&parent).unwrap().permissions().mode(),
        original_mode
    );
    call(&mut state, "workspace.list", json!({"project_id": project}));
    assert_eq!(crate::worktree::external_worktree_id(&checkout), workspace);
    assert_eq!(state.workspaces.get(&workspace).unwrap().root, checkout);
    assert_eq!(
        state
            .compaction_thresholds(&CompactionScope::Workspace(workspace))
            .unwrap()
            .get(AgentProvider::Pi),
        Some(234_567)
    );
    assert_eq!(std::fs::read(checkout.join("README.md")).unwrap(), contents);
}

#[test]
fn compaction_policy_historical_terminal_owner_does_not_inherit_reused_workspace_policy() {
    for parked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut state = scope_app(dir.path());
        let (project, source) = plain_project(&mut state, dir.path(), "source");
        let workspace = named_workspace(&mut state, &project, "working");
        let historical = workspace_owner(&mut state, &workspace);
        let historical_agent = policy_agent(&mut state, &historical, AgentProvider::Pi);
        let mut finished = state.runs.remove(&historical).unwrap();
        finished.run.state = crate::run::RunState::Archived;
        state
            .finish_run_mutation(historical.clone(), finished)
            .unwrap();
        let current = workspace_owner(&mut state, &workspace);
        let current_agent = policy_agent(&mut state, &current, AgentProvider::Pi);
        assert_ne!(historical, current);
        state
            .patch_compaction_thresholds(
                CompactionScope::Project(project.clone()),
                &cell(AgentProvider::Pi, Some(123_456)),
            )
            .unwrap();
        state
            .patch_compaction_thresholds(
                CompactionScope::Workspace(workspace.clone()),
                &cell(AgentProvider::Pi, Some(234_567)),
            )
            .unwrap();
        drop(state);
        if parked {
            std::fs::rename(source, dir.path().join("source-away")).unwrap();
        }
        let restarted = scope_app(dir.path());
        assert_eq!(restarted.project(&project).is_none(), parked);
        assert_eq!(
            restarted.projects.project_id_of(&historical).is_none(),
            parked
        );
        assert!(restarted.runs[&historical].run.state.is_terminal());
        assert_eq!(
            restarted.runs[&historical].worktree.path,
            restarted.runs[&current].worktree.path
        );
        assert_eq!(restarted.workspace_of_conversation(&historical), None);
        assert_policy(
            &restarted,
            &historical,
            &historical_agent,
            123_456,
            CompactionSource::Project(project),
        );
        assert_policy(
            &restarted,
            &current,
            &current_agent,
            234_567,
            CompactionSource::Workspace(workspace),
        );
    }
}

#[test]
fn compaction_wire_scopes_and_unknown_active_are_authoritative() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = scope_app(temp.path());
    let (project, _) = plain_project(&mut state, temp.path(), "wire");
    let workspace = named_workspace(&mut state, &project, "wire-workspace");
    let owner = workspace_owner(&mut state, &workspace);
    let agent = policy_agent(&mut state, &owner, AgentProvider::Claude);
    let settings = call(
        &mut state,
        "settings.set",
        json!({"compaction_thresholds":{"claude":150000}}),
    );
    assert_eq!(settings["compaction_thresholds"]["claude"], 150000);
    assert_eq!(
        settings["compaction"]["inherited"]["claude"]["source"]["kind"],
        "default"
    );
    let row = call(
        &mut state,
        "project.set_compaction",
        json!({"project_id":project,"compaction_thresholds":{"claude":120000}}),
    );
    assert_eq!(row["compaction"]["inherited"]["claude"]["tokens"], 150000);
    let row = call(
        &mut state,
        "workspace.set_compaction",
        json!({"workspace_id":workspace,"compaction_thresholds":{"claude":100000}}),
    );
    assert_eq!(row["compaction"]["effective"]["claude"]["tokens"], 100000);
    let settings = call(
        &mut state,
        "conversation.settings",
        json!({"entity_id":owner,"agent_id":agent,"max_context_tokens":90000}),
    );
    assert_eq!(settings["compaction"]["inherited"]["tokens"], 100000);
    let row = call(
        &mut state,
        "workspace.get",
        json!({"workspace_id":workspace}),
    );
    let digest = row["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == agent)
        .unwrap();
    assert_eq!(digest["compaction"]["configured"]["tokens"], 90000);
    assert_eq!(digest["compaction"]["active"], Value::Null);
    assert_eq!(digest["compaction"]["pending_change"], Value::Null);
}

#[test]
fn compaction_wire_map_invalid_mutations_are_atomic() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = scope_app(temp.path());
    let before = call(&mut state, "settings.get", json!({}));
    for patch in [
        json!({"claude":0}),
        json!({"unknown":12}),
        json!({"pi":123,"claude":0}),
        Value::Null,
    ] {
        let response = state.handle(req(
            "settings.set",
            json!({"compaction_thresholds":patch,"watch_agent_filed_tasks":false}),
        ));
        assert_eq!(response["ok"], false, "{response}");
        assert_eq!(call(&mut state, "settings.get", json!({})), before);
    }
    assert_eq!(
        call(
            &mut state,
            "settings.set",
            json!({"compaction_thresholds":{}})
        ),
        before
    );
}

#[test]
fn compaction_wire_scope_setters_reject_unknowns_zero_and_preserve_null_patch_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = scope_app(temp.path());
    let (project, _) = plain_project(&mut state, temp.path(), "wire-scopes");
    let workspace = named_workspace(&mut state, &project, "wire-workspace");
    for (method, field, owner) in [
        ("project.set_compaction", "project_id", project),
        ("workspace.set_compaction", "workspace_id", workspace),
    ] {
        let before = call(
            &mut state,
            method,
            json!({field:owner,"compaction_thresholds":{"pi":123456,"claude":150000}}),
        );
        for patch in [
            json!({"pi":0,"claude":120000}),
            json!({"other":123}),
            json!({"pi":-1}),
            json!({"pi":1.5}),
            json!({"pi":"123"}),
            json!({"pi":9007199254740992u64}),
            Value::Null,
        ] {
            let response = state.handle(req(
                method,
                json!({field:owner,"compaction_thresholds":patch}),
            ));
            assert_eq!(response["ok"], false, "{response}");
            assert_eq!(response["error_code"], "invalid_params", "{response}");
            assert_eq!(
                call(
                    &mut state,
                    method,
                    json!({field:owner,"compaction_thresholds":{}})
                ),
                before
            );
        }
        for params in [
            json!({field:owner}),
            json!({field:owner,"compaction_thresholds":{},"extra":1}),
        ] {
            assert_eq!(
                state.handle(req(method, params))["error_code"],
                "invalid_params"
            );
        }
        let cleared = call(
            &mut state,
            method,
            json!({field:owner,"compaction_thresholds":{"pi":null}}),
        );
        assert!(cleared["compaction_thresholds"].get("pi").is_none());
        assert_eq!(cleared["compaction_thresholds"]["claude"], 150000);
        assert_eq!(
            state.handle(req(
                method,
                json!({field:"missing","compaction_thresholds":{"pi":123}})
            ))["error_code"],
            "not_found"
        );
    }
}

#[test]
fn compaction_wire_mutations_invalidate_other_clients_only_after_commit() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = scope_app(temp.path());
    state.changes = crate::changes::ChangeBus::new(std::time::Duration::ZERO);
    let bus = state.changes();
    let (saving, mut saving_rx, saving_key) = crate::carrier::SessionSender::observable("saving");
    let (other, mut other_rx, other_key) = crate::carrier::SessionSender::observable("other");
    let subscription: crate::changes::SubscriptionSpec = serde_json::from_value(json!({
        "subscription_id":"compaction", "scope":{"kind":"all"}, "kinds":["compaction.changed"], "mode":"realtime", "priority":"background"
    })).unwrap();
    for sender in [&saving, &other] {
        bus.subscribe(sender, subscription.clone());
    }
    call(
        &mut state,
        "settings.set",
        json!({"compaction_thresholds":{}}),
    );
    let rejected = state.handle(req(
        "settings.set",
        json!({"compaction_thresholds":{"pi":0}}),
    ));
    assert_eq!(rejected["ok"], false);
    assert_eq!(bus.flush(), 0);
    call(
        &mut state,
        "settings.set",
        json!({"compaction_thresholds":{"pi":123456}}),
    );
    assert_eq!(bus.flush(), 2);
    for (rx, key, sender) in [
        (&mut saving_rx, &saving_key, &saving),
        (&mut other_rx, &other_key, &other),
    ] {
        let mut expected = crate::changes::compaction_changed_payload("device", None);
        expected["bridge_epoch"] = json!(sender.bridge_epoch());
        assert_eq!(
            crate::carrier::SessionSender::decrypt_push(key, &rx.try_recv().unwrap()),
            expected
        );
    }
    let old = state.settings_get();
    let config = temp.path().join("state/config.json");
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    let rejected = state.handle(req(
        "settings.set",
        json!({"compaction_thresholds":{"pi":222222}}),
    ));
    assert_eq!(rejected["ok"], false);
    assert_eq!(state.settings_get(), old);
    assert_eq!(bus.flush(), 0);
}
