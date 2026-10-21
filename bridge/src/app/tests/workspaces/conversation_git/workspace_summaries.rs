use super::*;
use crate::app::snapshot_projection::summary_counts;

#[test]
fn workspace_summaries_observe_agent_statuses_once_for_all_roots() {
    for size in [8, 32] {
        let (_tmp, mut state, _) = ownership_fixture(size, false);
        for number in 0..size {
            let owner = format!("run-{number:02}");
            let agent = format!("agent-{number:02}");
            let root = state.runs[&owner].worktree.path.clone();
            let key = crate::app::TabKey::agent(&root, &agent);
            let status = if number.is_multiple_of(2) {
                crate::harness::AgentStatus::Working
            } else {
                crate::harness::AgentStatus::Waiting
            };
            state.session_registry.test_insert_tab(
                key,
                dictated_agent_tab(&root, &owner, &agent, DictatedSession::reporting(status)),
            );
        }
        state.initialize_snapshot_projection();
        let (board, count) = summary_counts::measure(|| state.board_snapshot());
        for summary in board["workspace_summaries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["workspace_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("workspace-"))
            })
        {
            let id = summary["workspace_id"].as_str().unwrap();
            let number: usize = id.strip_prefix("workspace-").unwrap().parse().unwrap();
            let blockers = summary["finish_blockers"].as_array().unwrap();
            assert_eq!(
                blockers.contains(&json!(crate::workspace::FINISH_BLOCKER_AGENT_WORKING)),
                number.is_multiple_of(2),
                "{summary}"
            );
        }
        eprintln!("{size} workspaces / {size} sessions: {count:?}");
        assert!(
            count.statuses <= 3 * size,
            "repeated status observations: {count:?}"
        );
    }
}

#[test]
fn workspace_summaries_keep_working_aliases_delivery_and_fresh_statuses() {
    let (tmp, mut state, _) = ownership_fixture(4, false);
    let root = state.runs["run-00"].worktree.path.clone();
    #[cfg(unix)]
    let alias = {
        let alias = tmp.path().join("root-alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        alias
    };
    #[cfg(not(unix))]
    let alias = root.clone();
    for (number, path, status, live) in [
        (0, root.clone(), crate::harness::AgentStatus::Waiting, true),
        (1, alias, crate::harness::AgentStatus::Working, true),
        (
            2,
            state.runs["run-02"].worktree.path.clone(),
            crate::harness::AgentStatus::Working,
            false,
        ),
    ] {
        let agent = format!("agent-{number}");
        let mut tab =
            dictated_agent_tab(&path, "run-00", &agent, DictatedSession::reporting(status));
        tab.live = live;
        state
            .session_registry
            .test_insert_tab(crate::app::TabKey::agent(&path, &agent), tab);
    }
    let turn = crate::app::PendingAgentTurn {
        #[cfg(test)]
        logical_obligations: Vec::new(),
        operation_id: None,
        root: state.runs["run-01"].worktree.path.clone(),
        owner: "run-01".into(),
        agent_id: "delivery-agent".into(),
        conversation_id: "delivery-agent".into(),
        model_choice: Default::default(),
        choice_revision: 0,
        interrupt: false,
        say: Some(crate::app::TurnText {
            cold: "hello".into(),
            warm: "hello".into(),
        }),
        phase: "task_notice",
        wants_catch_up: false,
        survives_refusal: false,
    };
    let ticket = state.delivery_queue.start(&turn);
    state.initialize_snapshot_projection();
    assert_working_blockers(&state, &[true, true, false, false]);
    state.delivery_queue.settle(ticket);
    let alias_key = state
        .session_registry
        .agent_working_roots()
        .into_iter()
        .find(|(_, working)| *working)
        .unwrap()
        .0;
    state
        .session_registry
        .test_tab_mut(&crate::app::TabKey::agent(&alias_key, "agent-1"))
        .unwrap()
        .live = false;
    assert_working_blockers(&state, &[false, false, false, false]);
}

fn assert_working_blockers(state: &AppState, expected: &[bool]) {
    let board = state.board_snapshot();
    let summaries = board["workspace_summaries"].as_array().unwrap();
    assert_eq!(
        summaries
            .iter()
            .filter(|row| row["workspace_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("workspace-")))
            .count(),
        expected.len()
    );
    for (number, working) in expected.iter().enumerate() {
        let id = format!("workspace-{number:02}");
        let row = summaries
            .iter()
            .find(|row| row["workspace_id"] == id)
            .unwrap();
        assert_eq!(
            row["finish_blockers"]
                .as_array()
                .unwrap()
                .contains(&json!(crate::workspace::FINISH_BLOCKER_AGENT_WORKING)),
            *working,
            "{row}"
        );
    }
}
