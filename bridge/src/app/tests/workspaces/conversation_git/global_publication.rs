use super::*;
use crate::app::snapshot_projection::work_counts;

#[test]
fn global_publication_renders_each_branch_and_agent_scope_once() {
    let (_tmp, mut state, _) = ownership_fixture(16, false);
    add_tip_agents(&mut state, 4);
    state.initialize_snapshot_projection();
    let (_, counts) = work_counts::measure(|| state.publish_all_snapshot_projection());
    assert_eq!(counts.branches, state.runs.len(), "{counts:?}");
    assert_eq!(counts.agents, 2 * 4 * state.runs.len(), "{counts:?}");
    assert_eq!(
        counts.owner_candidates, 0,
        "publication rescanned rosters: {counts:?}"
    );
}

#[test]
fn global_publication_outcome_lookup_reuses_the_captured_root_index() {
    let (_tmp, mut state, _) = ownership_fixture(32, false);
    state.initialize_snapshot_projection();
    let _context = state.begin_snapshot_ownership();
    let (outcomes, comparisons) = crate::app::workspaces::count_path_comparisons(|| {
        state
            .runs
            .iter()
            .map(|(id, active)| {
                let project = state.projects.project_id_of(id).unwrap();
                state.preferred_workspace_snapshot_run_state(project, &active.worktree.path, None)
            })
            .collect::<Vec<_>>()
    });
    assert!(outcomes.iter().all(Option::is_some));
    assert!(
        comparisons <= state.runs.len(),
        "outcome lookups scanned every run: {comparisons}"
    );
}

#[test]
fn global_publication_captures_fresh_values_after_each_candidate() {
    let (_tmp, mut state, _) = ownership_fixture(2, false);
    add_tip_agents(&mut state, 1);
    state.initialize_snapshot_projection();
    let agent = state.runs["run-00"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    for topic in ["first candidate", "next candidate"] {
        state
            .runs
            .get_mut("run-00")
            .unwrap()
            .agents
            .by_id_mut(&agent)
            .unwrap()
            .topic = Some(topic.into());
        state.publish_all_snapshot_projection();
        let view = state.read_view.load();
        assert_eq!(
            view.get("agent.list", "run-00").unwrap()["agents"][0]["topic"],
            topic
        );
        let board = view.get("board.list", "").unwrap();
        let row = board["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["run_id"] == "run-00")
            .unwrap();
        assert_eq!(row["agents"][0]["topic"], topic);
        assert!(state.snapshot_ownership_context().is_none());
    }
}

fn checkout(root: PathBuf, number: usize) -> crate::worktree::ExternalWorktree {
    crate::worktree::ExternalWorktree {
        id: crate::worktree::external_worktree_id(&root),
        name: format!("checkout-{number}"),
        path: root,
        isolation: crate::isolation::Isolation::Worktree,
        branch: Some(format!("branch-{number}")),
        head_sha: "abc".into(),
        head_subject: "observed commit".into(),
        head_age_seconds: 5,
        head_committed_at: None,
        dirty_files: 0,
        unpushed: Some(0),
        upstream: None,
        comparison_ref: Some("main".into()),
        ahead: Some(0),
        behind: Some(0),
        diffstat: Default::default(),
        uncommitted: Default::default(),
    }
}

#[test]
fn global_publication_external_rows_visit_each_cached_checkout_once() {
    let (tmp, mut state, _) = ownership_fixture(0, false);
    let project = state.projects.at(0).id.clone();
    state.projects.at_mut(0).is_git = true;
    state.board.diff_mut().register_project(project.clone());
    let checkouts = (0..32)
        .map(|i| checkout(tmp.path().join(format!("checkout-{i}")), i))
        .collect::<Vec<_>>();
    state.board.diff_mut().store_entry_for_adapter(
        crate::app::board_index::DiffCacheEntry::ExternalScan {
            project_id: project,
            worktrees: checkouts.clone(),
        },
        std::time::Instant::now(),
    );
    let expected = checkouts
        .iter()
        .map(|w| state.external_worktree_row_of(&w.id).unwrap())
        .collect::<Vec<_>>();
    let (actual, counts) = work_counts::measure(|| state.cached_external_worktrees_json());
    assert_eq!(actual.rows, expected);
    assert!(!actual.scanning);
    assert!(
        counts.external_candidates <= checkouts.len(),
        "repeated cached checkout search: {counts:?}"
    );
}
