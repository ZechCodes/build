use super::*;
use crate::app::read_view::{ReadView, SnapshotDelta};
use std::collections::BTreeSet;

fn seed_clocks(state: &mut AppState) {
    for (id, active) in &state.runs {
        state.board.attention_mut().restore_entity_clocks(
            id.clone(),
            "2026-01-01T00:00:00Z".into(),
            "2026-01-01T00:00:00Z".into(),
            "2026-01-01T00:00:00Z".into(),
            crate::app::run_state_str(&active.run.state),
        );
    }
}

fn assert_full_reference(state: &AppState, incremental: &ReadView) {
    state.publish_snapshot_projection();
    let full = state.read_view.load();
    for key in full.keys().filter(|key| {
        matches!(
            key.method,
            "board.list" | "project.list" | "workspace.list" | "workspace.get"
        )
    }) {
        assert_eq!(
            incremental.get_owned(key.method, &key.owner),
            full.get_owned(key.method, &key.owner),
            "{}/{}",
            key.method,
            key.owner
        );
    }
}

fn fast_owner(state: &AppState, owner: &str) {
    let _context = state.begin_snapshot_ownership();
    let previous = state.read_view.load();
    let owners = BTreeSet::from([owner.to_owned()]);
    let delta = state
        .prepare_incremental_owner_snapshot(
            SnapshotDelta::new(state.read_view.next_version()),
            &previous,
            &owners,
        )
        .expect("stable owner layout must use the incremental producer");
    drop(delta);
    state.publish_owner_snapshot_projection(owner);
}

fn assert_fallback(state: &AppState) {
    let _context = state.begin_snapshot_ownership();
    assert!(state
        .prepare_incremental_owner_snapshot(
            SnapshotDelta::new(state.read_view.next_version()),
            &state.read_view.load(),
            &BTreeSet::from(["run-00".into()])
        )
        .is_none());
}

#[test]
fn incremental_rows_preserve_aliases_shared_outcomes_and_native_fold_ties() {
    let (_tmp, mut state, _) = ownership_fixture(8, true);
    add_tip_agents(&mut state, 1);
    let canonical = state.runs["run-00"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    for owner in ["run-01", "run-02"] {
        let root = state.runs["run-00"].worktree.path.clone();
        let active = state.runs.get_mut(owner).unwrap();
        active.worktree.path = root;
        active.worktree.recorded_branch = "same-branch".into();
        active
            .agents
            .iter_mut()
            .next()
            .unwrap()
            .bind_conversation(&canonical);
    }
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .worktree
        .recorded_branch = "same-branch".into();
    seed_clocks(&mut state);
    state.initialize_snapshot_projection();
    let old = state.read_view.load();
    let agent = state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&canonical)
        .unwrap();
    agent.topic = Some("canonical topic changed".into());
    agent
        .thread
        .post_agent("a canonical message", None, "2026-10-20T00:00:00Z");
    fast_owner(&state, "run-00");
    let next = state.read_view.load();
    assert_ne!(
        old.get_owned("board.list", ""),
        next.get_owned("board.list", "")
    );
    assert_full_reference(&state, &next);
    state.runs.get_mut("run-02").unwrap().run.state = crate::run::RunState::Merged;
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-02");
    assert_full_reference(&state, &state.read_view.load());
}

#[test]
fn incremental_rows_fall_back_for_reset_watch_roster_root_and_lifecycle_changes() {
    let (_tmp, mut state, _) = ownership_fixture(4, false);
    add_tip_agents(&mut state, 1);
    seed_clocks(&mut state);
    state.initialize_snapshot_projection();
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .thread
        .generation_revision += 1;
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-00");
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .watched = false;
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-00");
    state.runs.get_mut("run-00").unwrap().agents.add(
        "run-00",
        Default::default(),
        "2026-10-20T00:00:00Z",
    );
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-00");
    state.runs.get_mut("run-00").unwrap().worktree.path =
        state.runs["run-01"].worktree.path.clone();
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-00");
    state.workspace_lifecycle.insert(
        "workspace-03".into(),
        crate::reclaim::LifecycleRecord {
            idle: true,
            measured_at_ms: 1234,
            ..Default::default()
        },
    );
    assert_fallback(&state);
    state.publish_owner_snapshot_projection("run-00");
    let next = state.read_view.load();
    assert_eq!(
        next.get_owned("workspace.list", "").unwrap()["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["workspace_id"] == "workspace-03")
            .unwrap()["lifecycle"]["idle"],
        true
    );
    assert_full_reference(&state, &next);
}

#[test]
fn incremental_task_rows_keep_owner_payloads_and_unchanged_task_nodes() {
    let (_tmp, mut state, _) = ownership_fixture(16, false);
    add_tip_agents(&mut state, 1);
    seed_clocks(&mut state);
    state.initialize_snapshot_projection();
    let project = state.projects.project_id_of("run-00").unwrap().to_owned();
    for number in 0..700 {
        let id = format!("task-{number}");
        state.install_snapshot_watched_task(&project, &id, Some(json!({"kind":"tracker_task","task_id":id,"title":id,"anchor":"2026-01-01T00:00:00Z"})));
    }
    state.publish_snapshot_projection();
    let old = state.read_view.load();
    state.install_snapshot_watched_task(&project, "task-350", Some(json!({"kind":"tracker_task","task_id":"task-350","title":"Updated","anchor":"2026-01-01T00:00:00Z"})));
    let _context = state.begin_snapshot_ownership();
    let (delta, count) = work_counts::measure(|| {
        state
            .prepare_incremental_task_snapshot(
                SnapshotDelta::new(state.read_view.next_version()),
                &old,
            )
            .expect("stable task update must have an incremental projection")
    });
    state.publish_prepared_snapshot_projection(delta);
    drop(_context);
    assert_eq!(
        count.branches, 0,
        "task-only write rerendered unrelated owners: {count:?}"
    );
    let next = state.read_view.load();
    let before = old
        .get_shared("board.list", "")
        .unwrap()
        .field("runs")
        .unwrap();
    let after = next
        .get_shared("board.list", "")
        .unwrap()
        .field("runs")
        .unwrap();
    for (before, after) in before
        .elements()
        .unwrap()
        .iter()
        .zip(after.elements().unwrap())
    {
        assert!(before.ptr_eq(after));
    }
    let before = old.get_owned("board.list", "").unwrap();
    let after = next.get_owned("board.list", "").unwrap();
    assert_eq!(
        before["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["task_id"] == "task-350")
            .unwrap()["title"],
        "task-350"
    );
    assert_eq!(
        after["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["task_id"] == "task-350")
            .unwrap()["title"],
        "Updated"
    );
    assert_full_reference(&state, &next);
}

#[test]
fn incremental_rows_refresh_unrelated_clocks_registered_status_and_shared_finish_roots() {
    let (_tmp, mut state, _) = ownership_fixture(4, false);
    add_tip_agents(&mut state, 1);
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .iter_mut()
        .next()
        .unwrap()
        .working_since = Some("2999-01-01T00:00:00Z".into());
    let root = state.runs["run-02"].worktree.path.clone();
    let agent = state.runs["run-02"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    let key = crate::app::TabKey::agent(&root, &agent);
    state.session_registry.test_insert_tab(
        key.clone(),
        dictated_agent_tab(
            &root,
            "run-02",
            &agent,
            DictatedSession::reporting(crate::harness::AgentStatus::Working),
        ),
    );
    seed_clocks(&mut state);
    state.initialize_snapshot_projection();
    let old = state.read_view.load();
    state.session_registry.test_tab_mut(&key).unwrap().live = false;
    fast_owner(&state, "run-00");
    let next = state.read_view.load();
    let board = next.get_owned("board.list", "").unwrap();
    let row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == "run-01")
        .unwrap();
    assert_eq!(row["working_time"]["since"], "2999-01-01T00:00:00Z");
    assert_eq!(row["working_time"]["seconds"], 0);
    let blocked = |view: &ReadView| {
        view.get_owned("board.list", "").unwrap()["workspace_summaries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["workspace_id"] == "workspace-02")
            .unwrap()["finish_blockers"]
            .as_array()
            .unwrap()
            .contains(&json!(crate::workspace::FINISH_BLOCKER_AGENT_WORKING))
    };
    assert!(blocked(&old));
    assert!(!blocked(&next));
    assert_full_reference(&state, &next);
    state.session_registry.test_insert_tab(
        key,
        dictated_agent_tab(
            &root,
            "run-02",
            &agent,
            DictatedSession::reporting(crate::harness::AgentStatus::Waiting),
        ),
    );
    assert_fallback(&state);
}

#[cfg(unix)]
#[test]
fn incremental_layout_preserves_native_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;
    let (tmp, mut state, _) = ownership_fixture(2, false);
    let root = tmp
        .path()
        .join(std::ffi::OsString::from_vec(b"checkout-\xff".to_vec()));
    std::fs::create_dir(&root).unwrap();
    state.runs.get_mut("run-00").unwrap().worktree.path = root;
    seed_clocks(&mut state);
    state.initialize_snapshot_projection();
    fast_owner(&state, "run-00");
    assert_full_reference(&state, &state.read_view.load());
}

#[test]
fn incremental_rows_refresh_clockless_owner_fallbacks() {
    let (_tmp, mut state, _) = ownership_fixture(4, false);
    state.initialize_snapshot_projection();
    let old = state.read_view.load();
    fast_owner(&state, "run-00");
    let next = state.read_view.load();
    let old = old
        .get_shared("board.list", "")
        .unwrap()
        .field("runs")
        .unwrap();
    let next = next
        .get_shared("board.list", "")
        .unwrap()
        .field("runs")
        .unwrap();
    assert!(
        old.elements()
            .unwrap()
            .iter()
            .zip(next.elements().unwrap())
            .all(|(old, new)| !old.ptr_eq(new)),
        "clockless owner rows have dynamic anchor/resume fallbacks"
    );
}

#[test]
fn incremental_workspace_join_counts_the_actual_full_and_owner_producers() {
    use crate::app::board_index::DiffCacheEntry;
    use crate::app::snapshot_projection::summary_counts;
    let mut measurements = Vec::new();
    for size in [8, 32] {
        let (_tmp, mut state, _) = ownership_fixture(size, false);
        seed_clocks(&mut state);
        let subjects = state
            .workspaces
            .list(None)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let memberships = subjects
            .iter()
            .map(|workspace| {
                let repositories = workspace
                    .directories
                    .iter()
                    .filter(|directory| directory.is_git)
                    .map(|directory| directory.path.clone())
                    .collect::<Vec<_>>();
                (workspace.id.clone(), repositories)
            })
            .collect::<Vec<_>>();
        state.sync_workspace_summaries(&memberships);
        for (number, (workspace_id, repositories)) in memberships.into_iter().enumerate() {
            state.board.diff_mut().store_entry_for_adapter(DiffCacheEntry::WorkspaceSummary {
                workspace_id, repositories,
                summary: json!({"pushes":number,"dirty":false,"payload":format!("summary-{number}")}),
            }, std::time::Instant::now());
        }
        let projects = state
            .capture_project_snapshot_observations()
            .into_iter()
            .map(|row| (row.project_id.clone(), row.render()))
            .collect();
        state.install_project_snapshot_observations(projects);
        state.warm_snapshot_observations_for_test();
        let (_, full) = summary_counts::measure(|| state.publish_all_snapshot_projection());
        let (_, owner) =
            summary_counts::measure(|| state.publish_owner_snapshot_projection("run-00"));
        let view = state.read_view.load();
        let board = view.get_owned("board.list", "").unwrap();
        let workspaces = view.get_owned("workspace.list", "").unwrap();
        // Retain the old linear consumer oracle, independently of the indexed
        // producer: every field and every unmatched base field must survive.
        let expected = workspaces["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|workspace| {
                let mut row = workspace.clone();
                let summary = board["workspace_summaries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|summary| summary["workspace_id"] == row["workspace_id"])
                    .unwrap();
                assert!(
                    summary["work_summary"]["payload"].as_str().is_some(),
                    "the seeded summary must be observed"
                );
                for field in ["work_summary", "can_finish", "finish_blockers"] {
                    row[field] = summary[field].clone();
                }
                row
            })
            .collect::<Vec<_>>();
        assert_eq!(published_workspace_facts(&view), Value::Array(expected));
        measurements.push((size, full, owner));
    }
    for (size, full, owner) in measurements {
        eprintln!("{size} actual summary sources: full={full:?} owner={owner:?}");
        for count in [full, owner] {
            assert!(
                count.joins > 0,
                "the real producer's summary iteration was not observed"
            );
            assert!(
                count.joins <= size,
                "actual publication rescanned summaries: {count:?}"
            );
        }
    }
}

fn published_workspace_facts(view: &ReadView) -> Value {
    use crate::app::read_view::ReadViewSlot;
    let slot = ReadViewSlot::default();
    slot.publish_delta(
        SnapshotDelta::new(slot.next_version()).set_facts(std::sync::Arc::clone(&view.facts)),
    );
    let handle = crate::app::facts::FactsHandle::default();
    assert!(handle
        .set(crate::app::facts::FactsSource::new(
            slot,
            std::sync::Weak::new()
        ))
        .is_ok());
    let request = crate::changes::FactsRequest {
        entity_id: crate::changes::BOARD_ITEM_ID.into(),
        lists: crate::changes::BoardLists::WORKSPACES,
        ..Default::default()
    };
    let mut facts = crate::app::facts::entity_facts(&handle, &[request]);
    facts.pop().unwrap().state.unwrap()["workspaces"].take()
}

mod workspace_details;

mod compaction_policies;
