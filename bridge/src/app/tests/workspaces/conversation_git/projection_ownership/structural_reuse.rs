//! Structural reuse may cross guards; candidate rendering and admission may not.
use super::*;
use crate::app::read_view::{ReadView, SnapshotDelta};
use crate::app::workspaces::{ownership_work_counts as work, ProjectionOwnership};
use std::collections::BTreeMap;

const CLOCK: &str = "2026-01-01T00:00:00Z";

fn fixture(count: usize) -> (tempfile::TempDir, AppState) {
    let (tmp, mut app, _) = ownership_fixture(count, false);
    add_tip_agents(&mut app, 1);
    app.store = Some(crate::store::Store::new(tmp.path().join("tasks")).unwrap());
    for index in 0..count {
        let owner = format!("run-{index:02}");
        let mut active = app.take_run(&owner).unwrap();
        active
            .agents
            .sole_thread_mut()
            .post_user("committed history", None, CLOCK);
        app.finish_run_mutation(owner, active).unwrap();
    }
    for (owner, active) in &app.runs {
        app.board.attention_mut().restore_entity_clocks(
            owner.clone(),
            CLOCK.into(),
            CLOCK.into(),
            CLOCK.into(),
            crate::app::run_state_str(&active.run.state),
        );
    }
    let projects = app
        .capture_project_snapshot_observations()
        .into_iter()
        .map(|row| (row.project_id.clone(), row.render()))
        .collect();
    app.install_project_snapshot_observations(projects);
    app.warm_snapshot_observations_for_test();
    app.initialize_read_context();
    for owner in app.runs.keys() {
        app.mark_snapshot_owner_pending(owner);
    }
    app.publish_all_snapshot_projection();
    assert!(app.read_view.load().global.is_some());
    (tmp, app)
}

fn capture(app: &AppState) -> Arc<ProjectionOwnership> {
    let _guard = app.begin_snapshot_ownership();
    app.snapshot_ownership_context().unwrap()
}

fn values(view: &ReadView) -> BTreeMap<(&'static str, String), Value> {
    view.keys()
        .map(|key| {
            (
                (key.method, key.owner.clone()),
                view.get_owned(key.method, &key.owner).unwrap(),
            )
        })
        .collect()
}

fn assert_full_view(app: &AppState) {
    let old = app.read_view.load();
    let before = values(&old);
    let histories = old
        .history_keys()
        .map(|id| (id.to_owned(), old.history(id).unwrap().render(&json!({}))))
        .collect::<BTreeMap<_, _>>();
    app.publish_all_snapshot_projection();
    let full = app.read_view.load();
    assert_eq!(before, values(&full));
    assert_eq!(before, values(&old), "retained root changed");
    for (id, history) in histories {
        assert_eq!(full.history(&id).unwrap().render(&json!({})), history);
        assert_eq!(old.history(&id).unwrap().render(&json!({})), history);
    }
}

fn assert_legacy_ownership(app: &AppState, context: &ProjectionOwnership) {
    for workspace in app.workspaces.list(None) {
        assert_eq!(
            context.workspace_owner(&workspace.id).unwrap().cloned(),
            app.workspace_conversation_owner(workspace),
            "workspace {}",
            workspace.id
        );
    }
    for owner in app.runs.keys() {
        assert_eq!(
            context.is_project_owner(owner),
            app.is_project_conversation_owner(owner)
        );
        assert_eq!(
            context.surviving_workspace(owner),
            app.surviving_workspace_of_run(owner).as_deref()
        );
        assert_eq!(
            context.workspace_for_owner(owner, None),
            app.workspace_of_conversation(owner).as_deref()
        );
        for project in app.projects.iter() {
            assert_eq!(
                context.workspace_for_owner(owner, Some(&project.id)),
                app.workspace_of_conversation_in_project(owner, Some(&project.id))
                    .as_deref()
            );
        }
    }
    for roster in app
        .plans
        .values()
        .map(|plan| &plan.agents)
        .chain(app.runs.values().map(|run| &run.agents))
    {
        for agent in roster.iter() {
            assert_eq!(
                context.rendering.agent_owner(&agent.id),
                app.entity_of_agent(&agent.id).as_ref()
            );
        }
    }
}

fn assert_reused(counts: work::Counts, inputs: usize) {
    assert!(
        counts.source_rows > 0,
        "exact witness validation must inspect real inputs: {counts:?}"
    );
    assert!(
        counts.source_rows <= 32 * inputs,
        "validation must remain linear: {counts:?}"
    );
    assert_eq!(
        (
            counts.input_builds,
            counts.map_builds,
            counts.render_index_builds
        ),
        (0, 0, 0),
        "accepted structure was reconstructed: {counts:?}"
    );
}

fn input_count(app: &AppState) -> usize {
    app.runs.len()
        + app.plans.len()
        + app.workspaces.list(None).len()
        + app.projects.iter().count()
        + app.projects.parked().len()
        + app
            .runs
            .values()
            .map(|run| run.agents.iter().count())
            .sum::<usize>()
}

#[test]
fn accepted_structure_is_shared_across_distinct_guards_with_fresh_render_memos() {
    let (_tmp, app) = fixture(16);
    let first = capture(&app);
    let key = ("memo-probe".into(), "memo-agent".into(), None, false);
    first
        .rendering
        .remember_agent_digest(key.clone(), &json!({"old_candidate":true}));
    let (second, counts) = work::measure(|| capture(&app));
    assert!(
        !Arc::ptr_eq(&first, &second),
        "outer candidate contexts must remain distinct"
    );
    assert!(
        second.rendering.agent_digest(&key).is_none(),
        "render memo crossed a guard boundary"
    );
    assert_legacy_ownership(&app, &first);
    assert_legacy_ownership(&app, &second);
    assert_reused(counts, input_count(&app));
    assert_eq!(
        first.structural_identity_for_test(),
        second.structural_identity_for_test()
    );
}

async fn task(app: &AppState, id: &str) -> Value {
    let connection = app.require_store().unwrap().read_pool().acquire().await;
    let reply = app
        .read_service()
        .call(req("tasks.get", json!({"task_id":id})), Some(connection));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"]["task"].clone()
}

#[tokio::test]
async fn metadata_and_committed_finisher_reuse_structure_but_refresh_readers_and_rows() {
    let (_tmp, mut app) = fixture(4);
    let agent = first_agent(&app, "run-00");
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    let filed = crate::app::tests::tracker::filed(&mut app, &project, &format!("@agent:{agent}"));
    let id = filed["id"].as_str().unwrap();
    let old = app.read_view.load();
    let before = task(&app, id).await;
    assert_eq!(before["identities"][&agent]["available"], true);
    let captured = capture(&app);
    let (_, metadata_work) = work::measure(|| app.publish_read_metadata());
    assert_eq!(task(&app, id).await, before);
    let mut candidate = app.take_run("run-00").unwrap();
    let changed = candidate.agents.by_id_mut(&agent).unwrap();
    changed.name = Some("saved structural reuse".into());
    changed.topic = Some("fresh topic after save".into());
    changed
        .thread
        .post_agent("fresh committed history", None, CLOCK);
    let (_, finisher_work) =
        work::measure(|| app.finish_run_mutation("run-00".into(), candidate).unwrap());
    assert_eq!(
        task(&app, id).await["identities"][&agent]["name"],
        "saved structural reuse"
    );
    assert_eq!(
        app.read_service().mcp_addresses().display_name(&agent),
        Some("saved structural reuse")
    );
    let current = app.read_view.load();
    assert!(current
        .get_owned("agent.list", "run-00")
        .unwrap()
        .to_string()
        .contains("fresh topic after save"));
    assert!(!old
        .get_owned("agent.list", "run-00")
        .unwrap()
        .to_string()
        .contains("saved structural reuse"));
    assert!(current
        .history(&agent)
        .unwrap()
        .render(&json!({}))
        .to_string()
        .contains("fresh committed history"));
    app.session_registry.retire_agent_binding("run-00", &agent);
    let (_, availability_work) = work::measure(|| app.publish_read_metadata_only());
    assert_eq!(
        task(&app, id).await["identities"][&agent]["available"],
        false,
        "structural reuse must not freeze runtime availability"
    );
    assert_legacy_ownership(&app, &capture(&app));
    assert_full_view(&app);
    assert_reused(metadata_work, input_count(&app));
    assert_reused(finisher_work, 4 * input_count(&app));
    assert_reused(availability_work, input_count(&app));
    assert_eq!(
        captured.structural_identity_for_test(),
        capture(&app).structural_identity_for_test()
    );
}

#[test]
fn failed_save_and_borrowed_restoration_keep_the_accepted_structure_private() {
    let (tmp, mut app) = fixture(4);
    let old = app.read_view.load();
    let before = values(&old);
    let accepted = capture(&app);
    let root = app.runs["run-00"].worktree.path.clone();
    let mut candidate = app.take_run("run-00").unwrap();
    candidate.worktree.path = tmp.path().join("rejected-root");
    let added = candidate
        .agents
        .add("run-00", candidate.model_choice.clone(), CLOCK)
        .id
        .clone();
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation("run-00".into(), candidate).is_err());
    assert_eq!(app.runs["run-00"].worktree.path, root);
    assert!(app
        .read_service()
        .mcp_addresses()
        .issued_address(&added)
        .is_err());
    assert_eq!(before, values(&app.read_view.load()));
    let peer = app.take_run("run-01").unwrap();
    let mut saved = app.take_run("run-02").unwrap();
    saved.run.goal = "saved while peer borrowed".into();
    app.finish_run_mutation("run-02".into(), saved).unwrap();
    assert!(
        Arc::ptr_eq(&old, &app.read_view.load()),
        "borrowed peer must hold publication"
    );
    assert!(app.prepare_all_snapshot_projection().is_none());
    app.finish_run_mutation("run-01".into(), peer).unwrap();
    assert!(!values(&app.read_view.load())
        .values()
        .any(|value| value.to_string().contains("rejected-root")));
    assert_eq!(before, values(&old));
    let settled = capture(&app);
    let (after, counts) = work::measure(|| capture(&app));
    assert_legacy_ownership(&app, &after);
    assert_full_view(&app);
    assert_reused(counts, input_count(&app));
    assert!(!accepted
        .task_paths()
        .contains_key(&tmp.path().join("rejected-root")));
    assert_eq!(
        settled.structural_identity_for_test(),
        after.structural_identity_for_test()
    );
}

#[test]
fn rejected_global_candidate_cannot_install_its_structural_maps() {
    let (tmp, mut app) = fixture(2);
    let old = app.read_view.load();
    let before = values(&old);
    let root = app.runs["run-00"].worktree.path.clone();
    app.runs.get_mut("run-00").unwrap().worktree.path = tmp.path().join("stale-candidate-root");
    let candidate = app.prepare_all_snapshot_projection().unwrap();
    app.runs.get_mut("run-00").unwrap().worktree.path = root;
    // Advance the semantic-cache version using the exact accepted cache. No
    // wire or structure changes, but the older candidate must be rejected.
    let newer = SnapshotDelta::new(app.read_view.next_version())
        .set_global(old.global.as_ref().unwrap().clone());
    assert!(app.read_view.publish_delta(newer));
    let advanced = app.read_view.load();
    app.publish_prepared_snapshot_projection(candidate);
    assert!(Arc::ptr_eq(&advanced, &app.read_view.load()));
    assert!(Arc::ptr_eq(
        old.global.as_ref().unwrap(),
        advanced.global.as_ref().unwrap()
    ));
    assert_eq!(before, values(&advanced));
    assert_legacy_ownership(&app, &capture(&app));
}

fn replace_workspace(app: &mut AppState, id: &str, name: &str, root: PathBuf) {
    let project = app.workspaces.get(id).unwrap().project_id.clone();
    app.workspaces.forget(id);
    app.workspaces.adopt_root(
        &project,
        id.into(),
        name.into(),
        root,
        "source-1".into(),
        false,
    );
}

#[test]
fn structural_edits_preserve_sorted_workspaces_native_run_order_and_direct_ids() {
    let (tmp, mut app) = fixture(4);
    let shared = app.runs["run-00"].worktree.path.clone();
    app.runs.get_mut("run-01").unwrap().worktree.path = shared.clone();
    replace_workspace(&mut app, "workspace-01", "aaa", shared.clone());
    app.refresh_snapshot_observations();
    app.publish_all_snapshot_projection();
    let first = capture(&app);
    assert_legacy_ownership(&app, &first);
    assert_eq!(first.surviving_workspace("run-00"), Some("workspace-01"));
    replace_workspace(&mut app, "workspace-01", "zzz", shared.clone());
    let renamed = capture(&app);
    assert_eq!(renamed.surviving_workspace("run-00"), Some("workspace-00"));
    assert_ne!(
        first.structural_identity_for_test(),
        renamed.structural_identity_for_test()
    );
    assert_legacy_ownership(&app, &renamed);
    app.publish_all_snapshot_projection();
    // Rehashing changes native traversal without changing membership. Compare
    // every fallback answer against the actual current HashMap traversal.
    app.runs.reserve(256);
    assert_legacy_ownership(&app, &capture(&app));
    for changed in ["root", "state", "project", "roster"] {
        // Each edit starts from its own accepted baseline so a prior mismatch
        // cannot accidentally hide a missing input from the reuse witness.
        let (_case_root, mut case) = fixture(4);
        let before = capture(&case);
        let old_agent = first_agent(&case, "run-00");
        match changed {
            "root" => {
                case.runs.get_mut("run-00").unwrap().worktree.path = tmp.path().join("unowned-root")
            }
            "state" => {
                case.runs.get_mut("run-00").unwrap().run.state = crate::run::RunState::Merged
            }
            "project" => {
                let other_root = tmp.path().join("other-project");
                std::fs::create_dir_all(&other_root).unwrap();
                let other = case.add_project(other_root, "main".into());
                case.projects.bind_entity("run-00".into(), other);
            }
            "roster" => {
                let selected = case
                    .runs
                    .get_mut("run-00")
                    .unwrap()
                    .agents
                    .by_id_mut(&old_agent)
                    .unwrap();
                selected.id = "replacement-structural-agent".into();
            }
            _ => unreachable!(),
        }
        let next = capture(&case);
        assert_legacy_ownership(&case, &next);
        if changed == "roster" {
            assert!(next.rendering.agent_owner(&old_agent).is_none());
            assert_eq!(
                next.rendering
                    .agent_owner("replacement-structural-agent")
                    .map(String::as_str),
                Some("run-00")
            );
        }
        assert_ne!(
            before.structural_identity_for_test(),
            next.structural_identity_for_test(),
            "{changed}"
        );
    }
    // A terminal direct-ID owner still wins even with a different retained root.
    app.runs.get_mut("run-00").unwrap().run.state = crate::run::RunState::Merged;
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.workspaces.adopt_root(
        &project,
        "run-00".into(),
        "direct terminal".into(),
        tmp.path().join("different-root"),
        "source-1".into(),
        false,
    );
    assert_legacy_ownership(&app, &capture(&app));
}

#[test]
fn cold_guards_and_changed_native_state_root_do_not_reuse_old_classification() {
    let (tmp, mut app, _) = ownership_fixture(2, false);
    add_tip_agents(&mut app, 1);
    let first = capture(&app);
    let (cold, counts) = work::measure(|| capture(&app));
    assert_ne!(
        first.structural_identity_for_test(),
        cold.structural_identity_for_test()
    );
    assert_eq!(
        (
            counts.input_builds,
            counts.map_builds,
            counts.render_index_builds
        ),
        (1, 1, 1)
    );
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    let project_path = app.projects.get(&project).unwrap().repo_path.clone();
    let scratch = crate::app::projects::scratch_dir(&app.state_root, &project_path);
    std::fs::create_dir_all(&scratch).unwrap();
    app.runs.get_mut("run-00").unwrap().worktree.path = scratch;
    app.initialize_snapshot_projection();
    let warm = capture(&app);
    assert!(warm.is_project_owner("run-00"));
    app.state_root = tmp.path().join("new-native-state-root");
    let (changed, counts) = work::measure(|| capture(&app));
    assert!(!changed.is_project_owner("run-00"));
    assert_legacy_ownership(&app, &changed);
    assert_ne!(
        warm.structural_identity_for_test(),
        changed.structural_identity_for_test()
    );
    assert_eq!(
        (
            counts.input_builds,
            counts.map_builds,
            counts.render_index_builds
        ),
        (1, 1, 1)
    );
}

#[cfg(unix)]
#[test]
fn native_path_bytes_and_first_parked_entry_preserve_exact_workspace_answers() {
    use std::os::unix::ffi::OsStringExt;
    let (tmp, mut app) = fixture(2);
    let roots = [
        tmp.path()
            .join(std::ffi::OsString::from_vec(vec![b'x', 0x80])),
        tmp.path()
            .join(std::ffi::OsString::from_vec(vec![b'x', 0x81])),
    ];
    assert_eq!(roots[0].to_string_lossy(), roots[1].to_string_lossy());
    for (index, root) in roots.iter().enumerate() {
        std::fs::create_dir(root).unwrap();
        let owner = format!("run-{index:02}");
        app.runs.get_mut(&owner).unwrap().worktree.path = root.clone();
        replace_workspace(
            &mut app,
            &format!("workspace-{index:02}"),
            &owner,
            root.clone(),
        );
    }
    app.refresh_snapshot_observations();
    let native = capture(&app);
    for (index, root) in roots.iter().enumerate() {
        assert_eq!(
            native.task_paths().get(root),
            Some(&format!("workspace-{index:02}"))
        );
    }
    assert_legacy_ownership(&app, &native);
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.projects.remove(&project);
    app.projects.park(json!({"path":tmp.path()}));
    app.projects.park(json!({"id":project,"path":tmp.path()}));
    for owner in ["run-00", "run-01"] {
        app.projects
            .retain_entity_path(owner.into(), tmp.path().display().to_string());
    }
    app.refresh_snapshot_observations();
    let parked = capture(&app);
    assert_eq!(
        parked.compaction_workspace("run-00"),
        None,
        "first matching parked record without id must not fall through to later duplicate"
    );
    assert_legacy_ownership(&app, &parked);
}
