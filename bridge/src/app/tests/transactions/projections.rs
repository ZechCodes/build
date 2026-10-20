use super::*;

#[derive(Clone, Copy)]
enum OwnerKind {
    Plan,
    Run,
}

enum Candidate {
    Plan(Box<crate::orchestrator::ActivePlan>),
    Run(Box<crate::orchestrator::ActiveRun>),
}

impl Candidate {
    fn take(app: &mut AppState, owner: &str) -> Self {
        if app.plans.contains_key(owner) {
            Self::Plan(Box::new(app.take_plan(owner).unwrap()))
        } else {
            Self::Run(Box::new(app.take_run(owner).unwrap()))
        }
    }

    fn change(&mut self, label: &str, failed: bool) {
        let (agents, summary) = match self {
            Self::Plan(active) => {
                active.plan.goal = format!("{label} goal");
                active.plan.state = if failed {
                    crate::plan::PlanState::Failed
                } else {
                    crate::plan::PlanState::Blocked
                };
                (&mut active.agents, &mut active.last_summary)
            }
            Self::Run(active) => {
                active.run.goal = format!("{label} goal");
                active.run.state = if failed {
                    RunState::Failed
                } else {
                    RunState::Blocked
                };
                (&mut active.agents, &mut active.last_summary)
            }
        };
        *summary = Some(format!("{label} summary"));
        agents
            .sole_thread_mut()
            .post_agent(format!("{label} news"), None, "2099-01-01T00:00:00Z");
    }

    fn finish(self, app: &mut AppState, owner: &str) -> Result<(), String> {
        match self {
            Self::Plan(active) => app.finish_plan_mutation(owner.into(), *active),
            Self::Run(active) => app.finish_run_mutation(owner.into(), *active),
        }
    }
}

fn seed_owner(
    app: &mut AppState,
    repo: &Path,
    root: &Path,
    kind: OwnerKind,
    label: &str,
) -> String {
    let owner = match kind {
        OwnerKind::Plan => file_legacy_task(app, label),
        OwnerKind::Run => adopted_run(app, repo, root, label),
    };
    let mut candidate = Candidate::take(app, &owner);
    candidate.change(label, false);
    candidate.finish(app, &owner).unwrap();
    owner
}

fn expected_fields(label: &str, failed: bool) -> Value {
    json!({"goal": format!("{label} goal"), "state": if failed { "failed" } else { "blocked" }, "summary": format!("{label} summary")})
}

fn detail_fields(view: &Value) -> Value {
    json!({"goal": view["goal"], "state": view["state"], "summary": view["summary"]})
}

fn assert_durable_fields(app: &AppState, owner: &str, expected: &Value) {
    let store = app.store.as_ref().unwrap();
    let (canonical, saved, detail) = if let Some(active) = app.plans.get(owner) {
        let records = store.load_all_plans().unwrap();
        let saved = records.iter().find(|record| record.id == owner).unwrap();
        (
            json!({"goal": active.plan.goal, "state": crate::app::plan_state_str(&active.plan.state), "summary": active.last_summary}),
            json!({"goal": saved.goal, "state": crate::app::plan_state_str(&saved.state), "summary": saved.last_summary}),
            app.plan_view(
                owner,
                active,
                crate::thread::ThreadDetail::Digest,
                crate::app::DigestScope::Detail,
            ),
        )
    } else {
        let active = &app.runs[owner];
        let records = store.load_all_runs().unwrap();
        let saved = records.iter().find(|record| record.id == owner).unwrap();
        (
            json!({"goal": active.run.goal, "state": crate::app::run_state_str(&active.run.state), "summary": active.last_summary}),
            json!({"goal": saved.goal, "state": crate::app::run_state_str(&saved.state), "summary": saved.last_summary}),
            app.run_view(
                owner,
                active,
                crate::thread::ThreadDetail::Digest,
                crate::app::DigestScope::Detail,
            ),
        )
    };
    assert_eq!(&canonical, expected, "canonical {owner}");
    assert_eq!(&saved, expected, "durable {owner}");
    assert_eq!(&detail_fields(&detail), expected, "detail {owner}");
}

fn assert_snapshot_fields(app: &AppState, owner: &str, expected: &Value) {
    let snapshots = app.snapshot_service();
    let board = snapshots.call(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "{board}");
    let row = board["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == owner);
    if app.plans.contains_key(owner) {
        // Legacy plans intentionally have no board row or workspace detail.
        assert!(row.is_none(), "{board}");
        return;
    }
    let row = row.unwrap_or_else(|| panic!("missing {owner} in {board}"));
    assert_eq!(row["title"], expected["goal"]);
    assert_eq!(row["state"], expected["state"]);
    assert!(row.get("summary").is_none());
    let workspace = app
        .workspace_snapshot_for_conversation(owner, None)
        .expect("the fixture warms its workspace");
    let detail = snapshots.call(req("workspace.get", json!({"workspace_id": workspace.id})));
    assert_eq!(detail["ok"], true, "{detail}");
    assert_eq!(
        detail_fields(&detail["result"]["run"]),
        *expected,
        "{detail}"
    );
}

fn assert_push_fields(frames: &[Value], owner: &str, kind: OwnerKind, expected: &Value) {
    let row = frames
        .iter()
        .filter(|frame| frame["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().into_iter().flatten())
        .find(|item| item["entity_id"] == owner && item["state"].is_object())
        .unwrap_or_else(|| panic!("no state for {owner}: {frames:?}"));
    let state = &row["state"];
    match kind {
        OwnerKind::Plan => assert_eq!(state["run"], expected["state"], "{row}"),
        OwnerKind::Run => {
            assert_eq!(state["title"], expected["goal"], "{row}");
            assert_eq!(state["state"], expected["state"], "{row}");
        }
    }
    assert!(
        state.get("summary").is_none(),
        "summary is a detail-view field"
    );
}

#[tokio::test]
async fn failed_plan_fields_stay_committed_in_detail_snapshot_and_push_after_refresh() {
    assert_failed_fields_stay_committed(OwnerKind::Plan).await;
}

#[tokio::test]
async fn failed_run_fields_stay_committed_in_board_detail_and_push_after_refresh() {
    assert_failed_fields_stay_committed(OwnerKind::Run).await;
}

async fn assert_failed_fields_stay_committed(kind: OwnerKind) {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let owner = {
        let mut app = state.lock().unwrap();
        let owner = seed_owner(&mut app, &repo, dir.path(), kind, "committed");
        app.refresh_workspace_snapshot_observations();
        app.publish_all_snapshot_projection();
        owner
    };
    let initial = push::settled_pushes(&mut rx, &key).await;
    let expected = expected_fields("committed", false);
    assert_push_fields(&initial, &owner, kind, &expected);
    {
        let mut app = state.lock().unwrap();
        assert_durable_fields(&app, &owner, &expected);
        assert_snapshot_fields(&app, &owner, &expected);
        let mut candidate = Candidate::take(&mut app, &owner);
        candidate.change("rejected", true);
        app.store.as_ref().unwrap().fail_next_write();
        assert!(candidate.finish(&mut app, &owner).is_err());
        assert_durable_fields(&app, &owner, &expected);
        assert_snapshot_fields(&app, &owner, &expected);
    }
    let failed = push::settled_pushes(&mut rx, &key).await;
    assert!(push::changed_entities(&failed).is_empty(), "{failed:?}");
    {
        let mut app = state.lock().unwrap();
        file_legacy_task(&mut app, "unrelated successful save");
        app.publish_read_metadata();
        app.note_entity_changed(&owner);
        assert_durable_fields(&app, &owner, &expected);
        assert_snapshot_fields(&app, &owner, &expected);
    }
    let refreshed = push::settled_pushes(&mut rx, &key).await;
    assert_push_fields(&refreshed, &owner, kind, &expected);
    assert!(!serde_json::to_string(&refreshed)
        .unwrap()
        .contains("rejected"));
    // Positive control: all candidate fields really can reach the projections
    // and push, so their absence above is not an unobserved field or stale feed.
    {
        let mut app = state.lock().unwrap();
        let mut candidate = Candidate::take(&mut app, &owner);
        candidate.change("accepted", true);
        candidate.finish(&mut app, &owner).unwrap();
        assert_durable_fields(&app, &owner, &expected_fields("accepted", true));
        assert_snapshot_fields(&app, &owner, &expected_fields("accepted", true));
    }
    let accepted = push::settled_pushes(&mut rx, &key).await;
    assert_push_fields(&accepted, &owner, kind, &expected_fields("accepted", true));
}

#[tokio::test]
async fn committed_plan_waits_for_failed_borrowed_run_without_losing_fields() {
    assert_mixed_borrowing(OwnerKind::Plan, OwnerKind::Run).await;
}

#[tokio::test]
async fn committed_run_waits_for_failed_borrowed_plan_without_leaking_fields() {
    assert_mixed_borrowing(OwnerKind::Run, OwnerKind::Plan).await;
}

async fn assert_mixed_borrowing(first_kind: OwnerKind, second_kind: OwnerKind) {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (first, second, snapshots) = {
        let mut app = state.lock().unwrap();
        let first = seed_owner(&mut app, &repo, dir.path(), first_kind, "first-before");
        let second = seed_owner(&mut app, &repo, dir.path(), second_kind, "second-before");
        app.refresh_workspace_snapshot_observations();
        app.publish_all_snapshot_projection();
        (first, second, app.snapshot_service())
    };
    push::settled_pushes(&mut rx, &key).await;
    let before = snapshots.call(req("board.list", json!({})));
    let failed = {
        let mut app = state.lock().unwrap();
        let mut committed = Candidate::take(&mut app, &first);
        let mut failed = Candidate::take(&mut app, &second);
        committed.change("first-accepted", true);
        failed.change("second-rejected", true);
        committed.finish(&mut app, &first).unwrap();
        failed
    };
    assert_eq!(snapshots.call(req("board.list", json!({}))), before);
    let premature = push::settled_pushes(&mut rx, &key).await;
    assert!(
        push::changed_entities(&premature).is_empty(),
        "{premature:?}"
    );
    {
        let mut app = state.lock().unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(failed.finish(&mut app, &second).is_err());
        assert_durable_fields(&app, &first, &expected_fields("first-accepted", true));
        assert_durable_fields(&app, &second, &expected_fields("second-before", false));
        assert_snapshot_fields(&app, &first, &expected_fields("first-accepted", true));
        assert_snapshot_fields(&app, &second, &expected_fields("second-before", false));
    }
    let settled = push::settled_pushes(&mut rx, &key).await;
    assert_push_fields(
        &settled,
        &first,
        first_kind,
        &expected_fields("first-accepted", true),
    );
    assert!(
        !push::changed_entities(&settled).contains(&second),
        "{settled:?}"
    );
    {
        let app = state.lock().unwrap();
        app.note_entity_changed(&second);
    }
    let refreshed = push::settled_pushes(&mut rx, &key).await;
    assert_push_fields(
        &refreshed,
        &second,
        second_kind,
        &expected_fields("second-before", false),
    );
    assert!(!serde_json::to_string(&refreshed)
        .unwrap()
        .contains("second-rejected"));
}
