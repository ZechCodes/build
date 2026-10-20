use super::*;
use crate::workspace::read_work_counts;

#[test]
fn task_identity_capture_does_not_list_or_search_workspaces_per_agent() {
    let mut measurements = Vec::new();
    for size in [16, 64, 200] {
        let (_tmp, mut state, _) = ownership_fixture(size, false);
        add_tip_agents(&mut state, 4);
        state.initialize_snapshot_projection();
        let workspaces = state.workspaces.list(None).len();
        let _ownership = state.begin_snapshot_ownership();
        let (_, counts) = read_work_counts::measure(|| state.task_read_context());
        eprintln!("task identity {size} owners x4 agents: {counts:?}");
        measurements.push((workspaces, counts));
    }
    for (workspaces, counts) in measurements {
        assert!(
            counts.workspace_lists <= 1 && counts.workspace_rows <= workspaces,
            "task identity repeated workspace list/filter/sort per agent: {counts:?}"
        );
        assert!(
            counts.task_candidates <= workspaces,
            "task identity rescanned ordered workspace candidates: {counts:?}"
        );
    }
}

#[test]
fn ownership_capture_visits_and_normalizes_only_linear_inputs() {
    let mut measurements = Vec::new();
    for size in [16, 64, 200] {
        for mode in ["direct", "alias", "shared"] {
            let (_tmp, mut state, _) = ownership_fixture(size, mode == "shared");
            if mode == "direct" {
                for number in 0..size {
                    let owner = format!("run-{number:02}");
                    let root = state.runs[&owner].worktree.path.clone();
                    let project = state.projects.project_id_of(&owner).unwrap().to_owned();
                    state.workspaces.forget(&format!("workspace-{number:02}"));
                    state.workspaces.adopt_root(
                        &project,
                        owner.clone(),
                        owner,
                        root,
                        "source-1".into(),
                        false,
                    );
                }
            }
            state.initialize_snapshot_projection();
            let workspaces = state.workspaces.list(None).len();
            let inputs = state.runs.len() + workspaces + state.projects.iter().count();
            let (_, counts) = read_work_counts::measure(|| state.capture_snapshot_ownership());
            eprintln!("ownership capture {size} / {mode}: {counts:?}");
            measurements.push((size, mode, workspaces, inputs, counts));
        }
    }
    for (size, mode, workspaces, inputs, counts) in measurements {
        assert!(
            counts.owner_candidates + counts.scope_candidates <= 8 * inputs,
            "ownership capture repeatedly visits source candidates ({size}/{mode}): {counts:?}"
        );
        assert!(
            counts.observed_roots <= 8 * inputs,
            "ownership capture repeatedly normalizes source roots ({size}/{mode}): {counts:?}"
        );
        assert!(
            counts.workspace_lists <= 1 && counts.workspace_rows <= workspaces,
            "ownership capture repeated ordered registry allocation ({size}/{mode}): {counts:?}"
        );
        assert!(
            counts.observed_roots > 0,
            "input normalization must be measured"
        );
    }
}

#[test]
fn workspace_list_counter_includes_every_input_before_project_filtering() {
    let (_tmp, state, _) = ownership_fixture(16, false);
    let (_, counts) = read_work_counts::measure(|| state.workspaces.list(Some("missing-project")));
    assert_eq!(counts.workspace_lists, 1);
    assert_eq!(counts.workspace_rows, 16);
}

#[test]
fn task_identity_and_ownership_capture_have_separate_timed_phases() {
    let (_tmp, mut state, _) = ownership_fixture(2, false);
    add_tip_agents(&mut state, 2);
    state.initialize_snapshot_projection();
    let clock = FrameClock::new();
    let samples = clock.capture_phases();
    let state = Arc::new(Mutex::new(state));
    let timer = clock.frame("mcp.control");
    {
        let app = timer.lock(&state);
        app.task_read_context();
    }
    drop(timer);
    let measured = samples.snapshot();
    for phase in [
        "publication.ownership_capture",
        "read_context.task_identity",
    ] {
        assert_eq!(measured["mcp.control"][phase]["count"], 1, "{measured}");
    }
}
