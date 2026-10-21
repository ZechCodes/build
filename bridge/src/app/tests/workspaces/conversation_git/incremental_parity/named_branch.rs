//! One fresh named branch payload should serve board and push state together.
use super::*;
use crate::app::read_view::SharedJson;
use crate::app::snapshot_projection::work_counts::named_branch as work;

mod lifecycle;

fn agent(app: &AppState, owner: &str) -> String {
    app.runs[owner].agents.iter().next().unwrap().id.clone()
}

fn stat(app: &AppState, owner: &str) -> Value {
    app.board
        .diff()
        .run_stat(owner)
        .map(|cached| cached.value.clone())
        .unwrap_or(Value::Null)
}

fn board_node(view: &ReadView, owner: &str) -> Option<SharedJson> {
    let items = view.get_shared("board.list", "")?.field("items")?;
    items
        .elements()?
        .iter()
        .find(|row| row.leaf().is_some_and(|value| value["run_id"] == owner))
        .cloned()
}

fn state_node(view: &ReadView, owner: &str) -> SharedJson {
    view.facts
        .state_for_test(owner)
        .expect("fixture owner has pushed state")
}

fn assert_shared(view: &ReadView, owner: &str) {
    let board = board_node(view, owner).expect("fixture owner wins a board row");
    let cached = view
        .global
        .as_ref()
        .unwrap()
        .named_winner_for_test(owner)
        .unwrap();
    let state = state_node(view, owner);
    assert_eq!(
        board, state,
        "sharing must preserve the exact named wire value"
    );
    assert!(
        board.ptr_eq(&cached),
        "wire board did not retain the accepted winner node"
    );
    assert!(
        cached.ptr_eq(&state),
        "winner and push state indexed the same payload independently"
    );
}

fn publish_incremental(app: &AppState, owner: &str) -> Arc<ReadView> {
    app.publish_owner_snapshot_projection(owner);
    app.read_view.load()
}

fn change_topic(app: &mut AppState, owner: &str, topic: &str) {
    let id = agent(app, owner);
    app.runs
        .get_mut(owner)
        .unwrap()
        .agents
        .by_id_mut(&id)
        .unwrap()
        .topic = Some(topic.into());
}

fn assert_literal(view: &ReadView, owner: &str, topic: &str) {
    let row = state_node(view, owner).materialize();
    assert_eq!(row["run_id"], owner);
    assert_eq!(row["agents"][0]["topic"], topic);
    assert!(row["agents"][0]["thread_id"].is_string());
    let agent = row["agents"][0]["id"].as_str().unwrap();
    let history = view.history(agent).unwrap().render(&json!({}));
    assert_eq!(history["items"][0]["data"]["sequence"], 1);
    assert_eq!(
        row["title"],
        format!(
            "committed goal {}",
            owner.trim_start_matches("run-").parse::<usize>().unwrap()
        )
    );
}

#[test]
fn incremental_named_branch_indexes_each_32_and_200_owner_payload_only_once() {
    let mut measurements = Vec::new();
    for count in [32, 200] {
        let (_tmp, mut app) = fixture_with_count(count);
        let requests = requests(&app);
        let old = app.read_view.load();
        let before = image(&old, &requests);
        change_topic(&mut app, "run-00", "one freshly indexed branch");
        assert_fast_eligible(&app, "run-00");
        let ((next, reference), counts) = work::measure(|| {
            let next = publish_incremental(&app, "run-00");
            let _source = work::source("run-00", work::Source::Reference);
            let reference = SharedJson::value(crate::branch::named_project_row(
                app.branch_candidate_from_run("run-00", &stat(&app, "run-00"))
                    .row,
            ));
            (next, reference)
        });
        assert_literal(&next, "run-00", "one freshly indexed branch");
        assert_eq!(reference, state_node(&next, "run-00"));
        assert_full_reference(&app, &next, &requests);
        assert_eq!(before, image(&old, &requests));
        let visits = counts.get("run-00").copied().unwrap();
        assert!(
            visits.reference > 20,
            "positive actual recursive reference: {counts:?}"
        );
        assert!(
            visits.winner > 0,
            "actual incremental winner construction escaped attribution: {counts:?}"
        );
        eprintln!("named branch recursive work ({count} owners): {visits:?}");
        measurements.push((count, visits));
    }
    assert!(
        measurements
            .iter()
            .all(|(_, visits)| visits.winner + visits.state <= visits.reference),
        "same named payload was recursively indexed more than once: {measurements:?}"
    );
}

#[test]
fn accepted_incremental_board_cache_and_push_state_share_the_same_named_node() {
    let (_tmp, mut app) = fixture();
    let requests = requests(&app);
    let old = app.read_view.load();
    let before = image(&old, &requests);
    change_topic(&mut app, "run-00", "shared accepted branch");
    let next = publish_incremental(&app, "run-00");
    assert_literal(&next, "run-00", "shared accepted branch");
    assert_full_reference(&app, &next, &requests);
    assert_eq!(before, image(&old, &requests));
    assert_shared(&next, "run-00");
}

fn bind_alias(app: &mut AppState) -> (String, String) {
    let canonical = agent(app, "run-00");
    let alias = agent(app, "run-01");
    let mut candidate = app.take_run("run-01").unwrap();
    candidate
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&canonical);
    app.finish_run_mutation("run-01".into(), candidate).unwrap();
    (canonical, alias)
}

#[test]
fn folded_aliases_keep_the_losing_owners_own_state_and_share_only_the_winner() {
    let (_tmp, mut app) = fixture();
    let (canonical, alias) = bind_alias(&mut app);
    let root = app.runs["run-00"].worktree.path.clone();
    for owner in ["run-00", "run-01"] {
        let active = app.runs.get_mut(owner).unwrap();
        active.worktree.path = root.clone();
        active.worktree.recorded_branch = "one-folded-branch".into();
    }
    // Only the folded branch paths change; review observations remain accepted.
    let _ = app.refresh_snapshot_observations();
    app.publish_all_snapshot_projection();
    let expected_winner = app
        .runs
        .keys()
        .find(|id| ["run-00", "run-01"].contains(&id.as_str()))
        .unwrap()
        .clone();
    let loser = if expected_winner == "run-00" {
        "run-01"
    } else {
        "run-00"
    };
    let old = app.read_view.load();
    let requests = requests(&app);
    let before = image(&old, &requests);
    change_topic(&mut app, "run-00", "folded canonical update");
    let next = publish_incremental(&app, "run-00");
    let board = next.get_owned("board.list", "").unwrap();
    let folded = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| ["run-00", "run-01"].contains(&row["run_id"].as_str().unwrap_or("")))
        .collect::<Vec<_>>();
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0]["run_id"], expected_winner);
    let losing_state = state_node(&next, loser).materialize();
    assert_eq!(losing_state["run_id"], loser);
    assert_eq!(
        losing_state["agents"][0]["id"],
        if loser == "run-01" {
            alias.as_str()
        } else {
            canonical.as_str()
        }
    );
    assert_eq!(losing_state["agents"][0]["conversation_id"], canonical);
    assert!(!state_node(&next, loser).ptr_eq(&state_node(&next, &expected_winner)));
    assert_eq!(
        losing_state,
        crate::branch::named_project_row(
            app.branch_candidate_from_run(loser, &stat(&app, loser)).row
        )
    );
    assert_full_reference(&app, &next, &requests);
    assert_eq!(before, image(&old, &requests));
    assert_shared(&next, &expected_winner);
}
