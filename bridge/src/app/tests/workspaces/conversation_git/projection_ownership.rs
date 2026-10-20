use super::*;
use crate::compaction::{CompactionScope, CompactionSource};

fn ownership_edge_cases(state: &mut AppState, root: &Path) {
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    state.runs.get_mut("run-00").unwrap().run.state = crate::run::RunState::Merged;
    for (owner, name) in [("run-00", "zz-terminal"), ("run-01", "zz-direct")] {
        state.workspaces.adopt_root(
            &project,
            owner.into(),
            name.into(),
            root.join(name),
            "source-1".into(),
            false,
        );
    }
    state.runs.get_mut("run-03").unwrap().worktree.path =
        state.runs["run-02"].worktree.path.clone();
    let other_root = root.join("other-project");
    std::fs::create_dir(&other_root).unwrap();
    let other = state.add_project(other_root, "main".into());
    state.projects.bind_entity("run-04".into(), other);
    let scratch = crate::app::projects::scratch_dir(&state.state_root, root);
    std::fs::create_dir_all(&scratch).unwrap();
    state.runs.get_mut("run-06").unwrap().worktree.path = scratch.clone();
    state.workspaces.adopt_root(
        &project,
        "run-06".into(),
        "scratch".into(),
        scratch,
        "source-1".into(),
        false,
    );
    std::fs::remove_dir(&state.runs["run-07"].worktree.path).unwrap();
    #[cfg(unix)]
    {
        let alias = root.join("linked-root");
        std::os::unix::fs::symlink(&state.runs["run-05"].worktree.path, &alias).unwrap();
        state.runs.get_mut("run-05").unwrap().worktree.path = alias;
    }
}

#[test]
fn projection_ownership_preserves_direct_fallback_terminal_project_and_path_alias_answers() {
    let (tmp, mut state, _) = ownership_fixture(8, false);
    ownership_edge_cases(&mut state, tmp.path());
    state.refresh_snapshot_observations();
    let captured = state.capture_snapshot_ownership();
    for workspace in state.workspaces.list(None) {
        assert_eq!(
            captured.workspace_owner(&workspace.id).unwrap().cloned(),
            state.workspace_conversation_owner(workspace),
            "forward owner of {}",
            workspace.id,
        );
    }
    let mut scopes = vec![None];
    scopes.extend(
        state
            .projects
            .iter()
            .map(|project| Some(project.id.as_str())),
    );
    for owner in state.runs.keys() {
        assert_eq!(
            captured.is_project_owner(owner),
            state.is_project_conversation_owner(owner)
        );
        assert_eq!(
            captured.surviving_workspace(owner),
            state.surviving_workspace_of_run(owner).as_deref()
        );
        for project in &scopes {
            assert_eq!(
                captured.workspace_for_owner(owner, *project),
                state
                    .workspace_of_conversation_in_project(owner, *project)
                    .as_deref(),
                "reverse owner {owner}, project {project:?}",
            );
        }
    }
}

fn first_agent(state: &AppState, owner: &str) -> String {
    state.runs[owner].agents.iter().next().unwrap().id.clone()
}

fn configured_policy(state: &AppState, owner: &str, agent: &str) -> Value {
    let view = state.read_view.load();
    view.get("agent.list", owner).unwrap()["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == agent)
        .unwrap()["compaction"]["configured"]
        .clone()
}

fn assert_published_policy(
    state: &AppState,
    owner: &str,
    agent: &str,
    tokens: u64,
    source: CompactionSource,
) {
    let expected = crate::compaction::ResolvedCompactionPolicy { tokens, source };
    assert_eq!(
        state.resolve_compaction_policy(owner, agent).unwrap(),
        expected
    );
    assert_eq!(
        configured_policy(state, owner, agent),
        expected.observation()
    );
    assert!(
        state.snapshot_ownership_context().is_none(),
        "prepared context must not outlive its candidate"
    );
}

fn compaction_fixture() -> (tempfile::TempDir, AppState, String, String) {
    let (tmp, mut state, _) = ownership_fixture(2, false);
    add_tip_agents(&mut state, 1);
    let canonical = first_agent(&state, "run-00");
    let alias = first_agent(&state, "run-01");
    for owner in ["run-00", "run-01"] {
        let agent = first_agent(&state, owner);
        state
            .runs
            .get_mut(owner)
            .unwrap()
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .choice
            .provider = crate::models::AgentProvider::ClaudeAdk;
    }
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&canonical);
    for (workspace, claude, codex) in [
        ("workspace-00", 111_000, 222_000),
        ("workspace-01", 333_000, 444_000),
    ] {
        state
            .patch_compaction_thresholds(
                CompactionScope::Workspace(workspace.into()),
                &json!({"claude_adk": claude, "codex_app_server": codex}),
            )
            .unwrap();
    }
    state.store = Some(crate::store::Store::new(tmp.path().join("tasks")).unwrap());
    for owner in ["run-00", "run-01"] {
        let candidate = state.take_run(owner).unwrap();
        state.finish_run_mutation(owner.into(), candidate).unwrap();
    }
    state.initialize_snapshot_projection();
    (tmp, state, canonical, alias)
}

#[test]
fn projection_compaction_follows_canonical_alias_binding_and_launch_provider_changes() {
    let (_tmp, mut state, canonical, alias) = compaction_fixture();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        111_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
    state
        .edit_agent_record_checked("change launch provider", "run-01", &alias, |agent| {
            agent.choice.provider = crate::models::AgentProvider::CodexAppServer;
        })
        .unwrap();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        222_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
    state
        .edit_agent_record_checked("change canonical binding", "run-01", &alias, |agent| {
            agent.bind_conversation(&alias);
        })
        .unwrap();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        444_000,
        CompactionSource::Workspace("workspace-01".into()),
    );
    state
        .edit_agent_record_checked("restore canonical binding", "run-01", &alias, |agent| {
            agent.bind_conversation(&canonical);
        })
        .unwrap();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        222_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
}

#[test]
fn rejected_owner_scope_cannot_contaminate_a_later_projection_context() {
    let (tmp, mut state, _canonical, alias) = compaction_fixture();
    let original = state.runs["run-00"].worktree.path.clone();
    let next = tmp.path().join("unowned-root");
    std::fs::create_dir(&next).unwrap();
    let mut rejected = state.take_run("run-00").unwrap();
    rejected.worktree.path = next.clone();
    state.store.as_ref().unwrap().fail_next_write();
    assert!(state
        .finish_run_mutation("run-00".into(), rejected)
        .is_err());
    assert_eq!(state.runs["run-00"].worktree.path, original);
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        111_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
    state
        .patch_compaction_thresholds(CompactionScope::Device, &json!({"claude_adk": 555_000}))
        .unwrap();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        111_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
    let mut accepted = state.take_run("run-00").unwrap();
    accepted.worktree.path = next;
    state
        .finish_run_mutation("run-00".into(), accepted)
        .unwrap();
    // The alias's own workspace does not become the canonical conversation's scope.
    assert_published_policy(&state, "run-01", &alias, 555_000, CompactionSource::Device);
    let mut restored = state.take_run("run-00").unwrap();
    restored.worktree.path = original;
    state
        .finish_run_mutation("run-00".into(), restored)
        .unwrap();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        111_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
}

#[test]
fn projection_compaction_keeps_parked_project_workspace_inheritance() {
    let (tmp, mut state, _canonical, alias) = compaction_fixture();
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    state.projects.remove(&project);
    state
        .projects
        .park(json!({"id": project, "path": tmp.path()}));
    for owner in ["run-00", "run-01"] {
        state
            .projects
            .retain_entity_path(owner.into(), tmp.path().display().to_string());
    }
    state.refresh_snapshot_observations();
    state.publish_all_snapshot_projection();
    assert_published_policy(
        &state,
        "run-01",
        &alias,
        111_000,
        CompactionSource::Workspace("workspace-00".into()),
    );
}
