//! The narrow terminal attention patch must match a fresh canonical render.
use super::*;
use crate::app::read_view::ReadView;
use crate::app::snapshot_projection::work_counts;
use std::collections::BTreeMap;

type Views = BTreeMap<(&'static str, String), Value>;
const METHODS: [&str; 4] = [
    "board.list",
    "project.list",
    "workspace.list",
    "workspace.get",
];

#[tokio::test]
async fn terminal_attention_patched_views_match_full_rebuild() {
    let directory = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracker::tracked(directory.path());
    app.term_shell = "/bin/sh".into();
    let root = app.projects.get(&project).unwrap().repo_path.clone();
    let run = adopted_run(&mut app, &root, directory.path(), "terminal-view-run");
    let workspace = "terminal-view-workspace";
    app.workspaces.adopt_root(
        &project,
        workspace.into(),
        "Terminal view workspace".into(),
        app.runs[&run].worktree.path.clone(),
        "source-1".into(),
        false,
    );
    add_external_worktree(
        &root,
        directory.path(),
        "terminal-view-external",
        "terminal-view-external",
    );
    let external = app
        .scan_external_worktrees_now(&project)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some("terminal-view-external"))
        .unwrap()
        .id;
    // Give the external row a stable creation fallback while preserving its
    // absent attention entry until the first real terminal interaction.
    app.board
        .attention_mut()
        .record_created_if_absent(&external, "2000-01-01T00:00:00Z".into());
    app.board
        .attention_mut()
        .interact(&run, "2000-01-01T00:00:00Z");
    app.board
        .attention_mut()
        .interact(workspace, "2000-01-01T00:00:00Z");
    // Presence has its own regression; this oracle isolates attention from
    // time-dependent user-session metadata without normalizing any output.
    app.user_session = crate::session_summary::UserSession {
        session_started_ms: Some(0),
        last_activity_ms: Some(i64::MAX),
        previous_session_ended_ms: None,
    };
    app.initialize_snapshot_projection();
    app.publish_snapshot_projection();
    let state = Arc::new(Mutex::new(app));
    state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
    let handler = AppState::handler(Arc::clone(&state));
    for selector in [
        json!({"run_id":run}),
        json!({"workspace_id":workspace}),
        json!({"project_id":project, "worktree_id":external}),
    ] {
        assert_terminal_views_match(&state, &handler, selector, &project);
    }
}

fn assert_terminal_views_match(
    state: &Arc<Mutex<AppState>>,
    handler: &FrameHandler,
    selector: Value,
    project: &str,
) {
    let created = call(handler, "term.create", selector.clone());
    assert_eq!(created["ok"], true, "{selector}: {created}");
    let (patched, rebuilt, counts, child) = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        let child = Arc::clone(&app.session_registry.test_tab(&key).unwrap().session);
        let patched = views(&app.read_view.load());
        // This entry calls prepare_full_global_snapshot unconditionally and
        // workspace details with None: all bases and agent variants rerender.
        // It uses the same observations, without warming or refreshing them.
        let (_, counts) = work_counts::measure(|| app.publish_snapshot_projection());
        (patched, views(&app.read_view.load()), counts, child)
    };
    let closed = call(
        handler,
        "term.close",
        json!({"term_id":created["result"]["term_id"]}),
    );
    child.end();
    let reaped = child.exited_within(Duration::from_secs(2));
    // Cleanup precedes every equivalence assertion, including missing keys.
    assert!(reaped);
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(counts.workspace_details_full, 1, "{counts:?}");
    assert_eq!(counts.workspace_details_scoped, 0, "{counts:?}");
    assert_view_coverage(&patched, project);
    assert_eq!(
        patched.keys().collect::<Vec<_>>(),
        rebuilt.keys().collect::<Vec<_>>(),
        "{selector}"
    );
    for (key, value) in patched {
        assert_eq!(Some(&value), rebuilt.get(&key), "{selector}: {key:?}");
    }
}

fn views(root: &ReadView) -> Views {
    root.keys()
        .filter(|key| METHODS.contains(&key.method))
        .filter_map(|key| {
            root.get_owned(key.method, &key.owner)
                .map(|value| ((key.method, key.owner.clone()), value))
        })
        .collect()
}

fn assert_view_coverage(views: &Views, project: &str) {
    for method in METHODS {
        assert!(
            views.keys().any(|(held, _)| *held == method),
            "missing {method}"
        );
    }
    assert!(views.contains_key(&("workspace.list", project.to_owned())));
    assert!(
        views
            .keys()
            .any(|(method, owner)| *method == "workspace.get" && owner.contains('/')),
        "workspace agent variants must participate in the oracle"
    );
}
