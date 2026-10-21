//! Roster reads use producer path observations throughout both guarded phases.
use super::*;
use crate::app::mcp::probe_guarded_mcp_read;
use crate::app::workspaces::count_path_canonicalizations;

const COLD_ROSTER: &str = "agent.list: snapshot was not initialized";

fn observed_alias(fixture: &Fixture) -> (String, String) {
    let mut app = fixture.state.lock().unwrap();
    let repo = app
        .projects
        .get(&fixture.project)
        .unwrap()
        .repo_path
        .clone();
    let owner = adopted_run(&mut app, &repo, fixture._root.path(), "roster-observed");
    let root = app.runs[&owner].worktree.path.clone();
    let linked = fixture._root.path().join("roster-linked-root");
    std::os::unix::fs::symlink(&root, &linked).unwrap();
    let workspace = "workspace-roster-path-alias".to_owned();
    let source = app.projects.get(&fixture.project).unwrap().sources[0]
        .id
        .clone();
    app.workspaces.adopt_root(
        &fixture.project,
        workspace.clone(),
        "Observed roster alias".into(),
        linked,
        source,
        false,
    );
    assert_ne!(workspace, owner, "the direct-ID shortcut must not win");
    app.delivery_queue.clear_queued();
    // The fixture added a real registry alias directly. Observe that producer
    // input before the measured request, just as adoption/initialization does.
    app.refresh_snapshot_observations();
    app.publish_all_snapshot_projection();
    assert!(app.snapshot_projection_initialized());
    (workspace, owner)
}

fn assert_no_read_probes(fixture: &Fixture, action: BridgeAction, expected: Value) {
    let (answer, probes) = probe_guarded_mcp_read(&fixture.state, &fixture.frame(action), || {});
    assert_eq!(answer, expected);
    assert_eq!(
        probes, 0,
        "admission, rendering and final checks must not canonicalize paths"
    );
}

#[test]
fn mcp_workspace_roster_uses_observed_symlink_owner_through_both_guards() {
    let fixture = Fixture::new();
    let (workspace, owner) = observed_alias(&fixture);
    let action = BridgeAction::ListWorkspaceAgents {
        workspace_id: workspace,
    };
    let (expected, legacy_probes) = count_path_canonicalizations(|| fixture.legacy(action.clone()));
    assert_eq!(expected["ok"], true, "{expected}");
    assert_eq!(expected["result"]["entity_id"], owner);
    assert!(!expected["result"]["agents"].as_array().unwrap().is_empty());
    assert!(
        legacy_probes > 0,
        "the legacy fallback must really probe the aliased path"
    );
    assert_no_read_probes(&fixture, action, expected);
}

#[test]
fn mcp_project_roster_uses_observed_scratch_owner_through_both_guards() {
    let fixture = Fixture::new();
    let worker = {
        let mut app = fixture.state.lock().unwrap();
        let added = app.agent_add(&json!({"entity_id":fixture.owner})).unwrap();
        app.delivery_queue.clear_queued();
        app.publish_all_snapshot_projection();
        added["agent"]["id"].as_str().unwrap().to_owned()
    };
    let action = BridgeAction::ListProjectAgents;
    let (expected, legacy_probes) = count_path_canonicalizations(|| fixture.legacy(action.clone()));
    assert_eq!(expected["ok"], true, "{expected}");
    assert_eq!(expected["result"]["entity_id"], fixture.owner);
    assert_eq!(expected["result"]["pinned_agent_id"], fixture.agent);
    for expected_id in [&fixture.agent, &worker] {
        assert!(expected["result"]["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["id"] == *expected_id));
    }
    assert!(
        legacy_probes > 0,
        "the legacy project lookup must really probe its scratch path"
    );
    assert_no_read_probes(&fixture, action, expected);
}

#[test]
fn mcp_observed_rosters_preserve_empty_and_scope_refusals() {
    let fixture = Fixture::new();
    let empty = BridgeAction::ListWorkspaceAgents {
        workspace_id: fixture.workspace.clone(),
    };
    let expected = fixture.legacy(empty.clone());
    assert_eq!(expected["result"]["entity_id"], Value::Null);
    assert_eq!(expected["result"]["agents"], json!([]));
    assert_no_read_probes(&fixture, empty, expected);
    let (_home, other_repo) = init_repo();
    let (other, foreign) = {
        let mut app = fixture.state.lock().unwrap();
        let project = super::super::project_agent::added_project(&mut app, &other_repo);
        let workspace =
            super::super::project_agent::workspace(&mut app, &project, "foreign-roster");
        (project, workspace)
    };
    for workspace in ["missing-roster-workspace".to_owned(), foreign] {
        let action = BridgeAction::ListWorkspaceAgents {
            workspace_id: workspace,
        };
        let expected = fixture.legacy(action.clone());
        assert_eq!(expected["ok"], false);
        assert_no_read_probes(&fixture, action, expected);
    }
    {
        let mut app = fixture.state.lock().unwrap();
        app.projects.bind_entity(fixture.owner.clone(), other);
        app.publish_read_metadata();
    }
    let action = BridgeAction::ListProjectAgents;
    let expected = fixture.legacy(action.clone());
    assert_eq!(expected["result"]["entity_id"], Value::Null);
    assert_eq!(expected["result"]["agents"], json!([]));
    assert_no_read_probes(&fixture, action, expected);
}

#[test]
fn mcp_cold_rosters_refuse_without_probing_or_warming_inputs() {
    let fixture = Fixture::new();
    {
        let mut app = fixture.state.lock().unwrap();
        app.snapshot_observations = Default::default();
    }
    let before = fixture.state.lock().unwrap().read_view.load();
    for action in [
        BridgeAction::ListProjectAgents,
        BridgeAction::ListWorkspaceAgents {
            workspace_id: fixture.workspace.clone(),
        },
    ] {
        assert_no_read_probes(&fixture, action, json!({"ok":false,"error":COLD_ROSTER}));
        let app = fixture.state.lock().unwrap();
        assert!(!app.snapshot_projection_initialized());
        assert!(Arc::ptr_eq(&before, &app.read_view.load()));
    }
}

#[test]
fn mcp_roster_final_check_refuses_lost_observations_without_probing() {
    for project in [false, true] {
        let fixture = Fixture::new();
        let action = if project {
            BridgeAction::ListProjectAgents
        } else {
            BridgeAction::ListWorkspaceAgents {
                workspace_id: fixture.workspace.clone(),
            }
        };
        let (answer, probes) =
            probe_guarded_mcp_read(&fixture.state, &fixture.frame(action), || {
                fixture.state.lock().unwrap().snapshot_observations = Default::default();
            });
        assert_eq!(answer, json!({"ok":false,"error":COLD_ROSTER}));
        assert_eq!(probes, 0);
    }
}

#[test]
fn mcp_observed_roster_keeps_the_target_generation_final_fence() {
    let fixture = Fixture::new();
    let (workspace, owner) = observed_alias(&fixture);
    let (agent, generation) = {
        let app = fixture.state.lock().unwrap();
        let agent = primary_agent_id(&app, &owner);
        let generation = app.mcp_conversation_generation(&owner, &agent).unwrap();
        (agent, generation)
    };
    let action = BridgeAction::ListWorkspaceAgents {
        workspace_id: workspace,
    };
    let mut expected = None;
    let (answer, probes) = probe_guarded_mcp_read(&fixture.state, &fixture.frame(action), || {
        let mut app = fixture.state.lock().unwrap();
        let reset = app.handle(req(
            "conversation.reset",
            json!({
                "project_id":fixture.project,"entity_id":owner,"agent_id":agent,
                "conversation_id":agent,"expected_thread_id":generation.thread_id
            }),
        ));
        assert_eq!(reset["ok"], true, "{reset}");
        expected = Some(
            generation
                .guard(&app)
                .expect_err("target generation changed"),
        );
    });
    assert_eq!(answer, json!({"ok":false,"error":expected.unwrap()}));
    assert_eq!(probes, 0);
}
