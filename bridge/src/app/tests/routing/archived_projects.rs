use super::router::captured;
use super::*;
use crate::harness::{test_session_factory, ActivityReport, OpenedSession, SessionOutput};
use std::sync::atomic::{AtomicUsize, Ordering};

fn two_projects(dir: &tempfile::TempDir) -> (AppState, String, String) {
    let first = init_repo_named(dir.path(), "first");
    let second = init_repo_named(dir.path(), "second");
    let mut app = qa_state(&first, dir.path());
    let first = app.project_at(0).id.clone();
    let second = app.add_project(second, "main".into());
    (app, first, second)
}

#[test]
fn default_project_skips_archived_and_prefers_restored_registration_order() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, first, second) = two_projects(&dir);
    app.projects.at_mut(0).archived_at = Some(1);
    assert_eq!(app.default_project().unwrap(), second);
    let restored = app.handle(req("project.restore", json!({"project_id": first})));
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(app.default_project().unwrap(), first);
}

#[test]
fn default_project_distinguishes_all_archived_from_none_registered() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, first, second) = two_projects(&dir);
    app.projects.at_mut(0).archived_at = Some(1);
    app.projects.at_mut(1).archived_at = Some(1);
    assert_eq!(
        app.default_project().unwrap_err(),
        "Every project is archived; restore a project first."
    );
    app.projects.remove(&first);
    app.projects.remove(&second);
    assert_eq!(app.default_project().unwrap_err(), "no projects configured");
}

#[test]
fn capture_is_refused_plainly_when_every_project_is_archived() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, _) = two_projects(&dir);
    app.projects.at_mut(0).archived_at = Some(1);
    app.projects.at_mut(1).archived_at = Some(1);
    let created = app.handle(req("capture.create", json!({"text": "keep my words"})));
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(created["result"]["state"], "failed", "{created}");
    let id = created["result"]["id"].as_str().unwrap();
    let refused = app.handle(req("capture.reroute", json!({"capture_id": id})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refused["error"],
        "Every project is archived; restore a project first."
    );
    assert!(app.router_sessions.is_empty());
    assert_eq!(app.delivery_queue.queued_len(), 0);
    assert!(app
        .captures
        .values()
        .all(|capture| capture.state != crate::capture::CaptureState::Routing));
    let saved = Store::new(dir.path().join("store"))
        .unwrap()
        .load_all_captures()
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].text, "keep my words");
    assert_eq!(saved[0].state, crate::capture::CaptureState::Failed);
}

#[test]
fn router_list_projects_omits_archived() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, first, second) = two_projects(&dir);
    let (capture, _) = captured(&mut app, "choose a destination");
    app.projects.at_mut(0).archived_at = Some(1);
    let listed = app
        .router_action(&capture, BridgeAction::ListProjects)
        .unwrap();
    let ids: Vec<_> = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["project_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![second.as_str()]);
    // Ordinary project reads retain archived identities for settings and display.
    let listed = app.handle(req("project.list", json!({})));
    assert!(listed["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["project_id"] == first));
}

async fn routing_skips_archived_first(retry: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, second) = two_projects(&dir);
    let opens = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&opens);
    let log = SessionLog::default();
    let recorded = log.clone();
    let _provider = test_session_factory::install(
        std::fs::canonicalize(dir.path()).unwrap(),
        Arc::new(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            let (activity, _) = tokio::sync::broadcast::channel::<ActivityReport>(16);
            Ok(OpenedSession {
                session: Arc::new(
                    DictatedSession::reporting(AgentStatus::Waiting)
                        .reporting_activity(activity.clone())
                        .recording_into(&recorded),
                ),
                output: SessionOutput::reporting(activity.subscribe(), None),
            })
        }),
    );
    let capture = if retry {
        let (id, _) = captured(&mut app, "route to the active project");
        app.abandon_router_session(&id);
        app.mark_routing_failed(&id);
        Some(id)
    } else {
        None
    };
    app.projects.at_mut(0).archived_at = Some(1);
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let created = if let Some(capture) = capture {
        call(&handler, "capture.reroute", json!({"capture_id": capture}))
    } else {
        call(
            &handler,
            "capture.create",
            json!({"text": "route to the active project"}),
        )
    };
    assert_eq!(created["ok"], true, "{created}");
    let id = created["result"]["id"].as_str().unwrap();
    let delivered = wait_for(Duration::from_secs(3), || {
        (!log.turns().is_empty()).then_some(())
    })
    .await;
    if delivered.is_some() {
        wait_for_deliveries(&state).await;
    }
    let mut app = state.lock().unwrap();
    let opened = opens.load(Ordering::SeqCst);
    let queued = app.delivery_queue.queued_len();
    let bound = app.project_of(id);
    // A real router can now settle its decision instead of remaining queued.
    app.router_action(
        id,
        BridgeAction::RouterMessage {
            body: "Which branch?".into(),
            waiting: true,
        },
    )
    .unwrap();
    let settled = app.captures[id].state;
    app.abandon_router_session(id);
    assert!(
        delivered.is_some(),
        "opens={opened}, queued={queued}, bound={bound:?}"
    );
    assert_eq!(opened, 1);
    assert_eq!(queued, 0);
    assert_eq!(bound.unwrap(), second);
    assert_ne!(settled, crate::capture::CaptureState::Routing);
}

#[tokio::test]
async fn capture_routes_when_first_project_is_archived() {
    routing_skips_archived_first(false).await;
}

#[tokio::test]
async fn reroute_without_a_project_skips_an_archived_first_project() {
    routing_skips_archived_first(true).await;
}

#[test]
fn router_launch_fallback_requires_an_active_project() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, _, second) = two_projects(&dir);
    let (owner, agent) = captured(&mut app, "route without a project binding");
    let router = app.router_sessions[&owner].clone();
    app.projects.unbind_entity(&owner);
    app.projects.at_mut(0).archived_at = Some(1);
    app.projects.at_mut(1).archived_at = Some(1);
    let key = TabKey::agent(&AppState::canonical_root(router.scratch_dir()), &agent);
    let request = AgentSpawnRequest {
        owner: &owner,
        agent_id: &agent,
        conversation_id: &agent,
        model_choice: router.choice(),
        force_fresh: false,
        phase: "route",
    };
    assert!(
        reserve_agent_spawn(&mut app, &key, &request).is_err(),
        "the fallback must not launch from an archived project"
    );
    let restored = app.handle(req("project.restore", json!({"project_id": second})));
    assert_eq!(restored["ok"], true, "{restored}");
    let state = app.shared();
    let reserved = reserve_agent_spawn(&mut state.lock().unwrap(), &key, &request)
        .expect("restoring the second project makes the fallback launchable");
    reserved.holding.abandon(&state, (), &a_frame(&state));
}
