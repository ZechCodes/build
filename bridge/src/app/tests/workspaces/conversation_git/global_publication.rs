use super::*;
use crate::app::snapshot_projection::work_counts;

#[test]
fn global_publication_renders_each_branch_and_agent_scope_once() {
    let (_tmp, mut state, _) = ownership_fixture(16, false);
    add_tip_agents(&mut state, 4);
    state.initialize_snapshot_projection();
    let (_, counts) = work_counts::measure(|| state.publish_all_snapshot_projection());
    eprintln!("16 owners / 4 agents publication work: {counts:?}");
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
    eprintln!("32 indexed outcome lookups: {comparisons} path comparisons");
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
    let _context = state.begin_snapshot_ownership();
    let ((actual, states), counts) = work_counts::measure(|| {
        let rows = state.cached_external_worktrees_json();
        let states = checkouts
            .iter()
            .map(|w| state.external_worktree_row_of(&w.id).unwrap())
            .collect::<Vec<_>>();
        (rows, states)
    });
    eprintln!("32 external list rows plus 32 state lookups: {counts:?}");
    assert_eq!(actual.rows, expected);
    assert_eq!(states, expected);
    assert!(!actual.scanning);
    assert!(
        counts.external_candidates <= checkouts.len(),
        "repeated cached checkout search: {counts:?}"
    );
}

#[test]
fn global_publication_outcomes_preserve_direct_ids_project_scope_and_path_aliases() {
    let (tmp, mut state, _) = ownership_fixture(8, true);
    state.runs.get_mut("run-00").unwrap().run.state = crate::run::RunState::Merged;
    state.runs.get_mut("run-01").unwrap().run.state = crate::run::RunState::Abandoned;
    state.runs.get_mut("run-02").unwrap().run.state = crate::run::RunState::Archived;
    let other_root = tmp.path().join("other-project");
    std::fs::create_dir(&other_root).unwrap();
    let other = state.add_project(other_root, "main".into());
    state.projects.bind_entity("run-03".into(), other);
    #[cfg(unix)]
    {
        let target = state.runs["run-00"].worktree.path.clone();
        let alias = tmp.path().join("root-alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        state.runs.get_mut("run-01").unwrap().worktree.path = alias;
    }
    state.initialize_snapshot_projection();
    let mut queries = Vec::new();
    for project in state
        .projects
        .iter()
        .map(|project| project.id.as_str())
        .chain(["unknown-project"])
    {
        for active in state.runs.values() {
            for direct in [None, Some("run-00"), Some("run-01"), Some("missing-run")] {
                queries.push((
                    project.to_owned(),
                    active.worktree.path.clone(),
                    direct,
                    state.preferred_workspace_run_state(project, &active.worktree.path, direct),
                ));
            }
        }
    }
    let _context = state.begin_snapshot_ownership();
    for (project, root, direct, expected) in queries {
        assert_eq!(
            state.preferred_workspace_snapshot_run_state(&project, &root, direct),
            expected,
            "project={project} root={} direct={direct:?}",
            root.display()
        );
    }
}

#[test]
fn global_publication_index_preserves_agent_lookup_and_missing_aliases() {
    let (_tmp, mut state, _) = ownership_fixture(4, false);
    add_tip_agents(&mut state, 2);
    state.initialize_snapshot_projection();
    let ids = state
        .runs
        .values()
        .flat_map(|run| run.agents.iter().map(|agent| agent.id.clone()))
        .chain(["missing-agent".to_owned()])
        .collect::<Vec<_>>();
    let (expected, counts) = work_counts::measure(|| {
        ids.iter()
            .map(|id| state.entity_of_agent(id))
            .collect::<Vec<_>>()
    });
    assert!(
        counts.owner_candidates > 0,
        "the unindexed lookup must exercise the counter"
    );
    let _context = state.begin_snapshot_ownership();
    let (actual, counts) = work_counts::measure(|| {
        ids.iter()
            .map(|id| state.entity_of_agent(id))
            .collect::<Vec<_>>()
    });
    assert_eq!(actual, expected);
    assert_eq!(counts.owner_candidates, 0);
}

#[test]
fn global_publication_digest_memo_keeps_supplied_preview_metadata() {
    let (_tmp, mut state, _) = ownership_fixture(1, false);
    add_tip_agents(&mut state, 1);
    state.initialize_snapshot_projection();
    let agent = state.runs["run-00"].agents.iter().next().unwrap();
    let _context = state.begin_snapshot_ownership();
    let first = state.agent_digest("run-00", agent, None, crate::app::DigestScope::List);
    let mut preview = agent.clone();
    preview.topic = Some("preview metadata".into());
    let next = state.agent_digest("run-00", &preview, None, crate::app::DigestScope::List);
    assert_ne!(first["topic"], next["topic"]);
    assert_eq!(next["topic"], "preview metadata");
}

#[test]
fn global_publication_external_rows_keep_first_project_scope_for_duplicate_ids() {
    let (tmp, mut state, _) = ownership_fixture(0, false);
    let shared = checkout(tmp.path().join("shared-checkout"), 0);
    let other_root = tmp.path().join("other-project");
    std::fs::create_dir(&other_root).unwrap();
    state.add_project(other_root, "main".into());
    for index in 0..2 {
        let project = state.projects.at(index).id.clone();
        state.projects.at_mut(index).is_git = true;
        state.board.diff_mut().register_project(project.clone());
        let mut observed = shared.clone();
        observed.head_subject = format!("project {index} observation");
        state.board.diff_mut().store_entry_for_adapter(
            crate::app::board_index::DiffCacheEntry::ExternalScan {
                project_id: project,
                worktrees: vec![observed],
            },
            std::time::Instant::now(),
        );
    }
    let expected = state.external_worktree_row_of(&shared.id).unwrap();
    let rows = state.cached_external_worktrees_json();
    assert_eq!(rows.rows, vec![expected.clone(), expected]);
    assert!(!rows.scanning);
}

#[test]
fn global_publication_updates_each_canonical_history_once() {
    let (_tmp, mut state, _) = ownership_fixture(16, false);
    add_tip_agents(&mut state, 4);
    state.initialize_snapshot_projection();
    for owner in state.runs.keys() {
        state.mark_snapshot_owner_pending(owner);
    }
    let (_, counts) = work_counts::measure(|| state.publish_all_snapshot_projection());
    eprintln!(
        "64 canonical threads: {} history updates",
        counts.history_updates
    );
    assert_eq!(
        counts.history_updates,
        16 * 4,
        "repeated canonical history updates: {counts:?}"
    );
}

#[test]
fn global_publication_unread_memo_distinguishes_cursors_and_preview_threads() {
    let (_tmp, mut state, _) = ownership_fixture(1, false);
    add_tip_agents(&mut state, 1);
    let agent = state.runs["run-00"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&agent)
        .unwrap()
        .thread
        .post_agent("canonical message", None, "2026-10-19T00:00:00Z");
    state.initialize_snapshot_projection();
    let thread = state.agent_conversation("run-00", Some(&agent)).unwrap();
    let _context = state.begin_snapshot_ownership();
    let ((unread, read), counts) = work_counts::measure(|| {
        (
            state.unread_including_history(&agent, thread, 0),
            state.unread_including_history(&agent, thread, thread.last_sequence()),
        )
    });
    assert_eq!(unread.count, 1);
    assert_eq!(read.count, 0);
    assert_eq!(read.reason, None);
    assert_eq!(counts.history_updates, 1);
    let mut preview = thread.clone();
    preview.post_agent("preview message", None, "2026-10-19T00:01:00Z");
    assert_eq!(state.unread_including_history(&agent, &preview, 0).count, 2);
    assert_eq!(state.unread_including_history(&agent, thread, 0), unread);
}

#[test]
fn global_publication_unread_memo_expires_when_an_alias_rebinds() {
    let (_tmp, mut state, _) = ownership_fixture(2, false);
    add_tip_agents(&mut state, 1);
    let canonical = state.runs["run-00"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    let alias = state.runs["run-01"]
        .agents
        .iter()
        .next()
        .unwrap()
        .id
        .clone();
    for (owner, agent, count) in [("run-00", &canonical, 1), ("run-01", &alias, 2)] {
        for _ in 0..count {
            state
                .runs
                .get_mut(owner)
                .unwrap()
                .agents
                .by_id_mut(agent)
                .unwrap()
                .thread
                .post_agent("message", None, "2026-10-19T00:00:00Z");
        }
    }
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&canonical);
    state.initialize_snapshot_projection();
    let previous = state.read_view.load();
    assert_eq!(
        previous.get("agent.list", "run-01").unwrap()["agents"][0]["unread_count"],
        1
    );
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&alias);
    state.mark_snapshot_owner_pending("run-01");
    state.publish_owner_snapshot_projection("run-01");
    let current = state.read_view.load();
    let row = &current.get("agent.list", "run-01").unwrap()["agents"][0];
    assert_eq!(row["conversation_id"], alias);
    assert_eq!(row["unread_count"], 2);
    assert_eq!(
        previous.get("agent.list", "run-01").unwrap()["agents"][0]["unread_count"],
        1
    );
}
