use super::super::workspace::NEW_THREAD_MESSAGES_PROMPT;
use super::reporting::done;
use super::runs::{
    comment_by_id, comment_on, dispatch_turn_halves, last_commit_subject, manifest_entry,
    orchestrator, posted_turn_halves, run_past_first_stage, split_store,
};
use super::workspace::{registered_checkouts, user_worktree, worktree_head};
use crate::git_fixture::init_repo;
use crate::isolation::Isolation;
use crate::mcp::{DoneReport, DoneStatus};
use crate::orchestrator::{
    gate_plan_message, gate_plan_stage_notes, mcp_config_path, ActivePlan, ActiveRun,
    AdoptableCheckout, AgentTurn, ImplementableTask, Orchestrator, OrchestratorError, RunSource,
};
use crate::plan::{
    plan_transition, PlanEvent, PlanId, PlanState, StageDocState, StageManifestEntry,
};
use crate::run::{RunId, RunState, StageProgressState};
use crate::store::{PersistedPlan, Store};
use crate::templates;
use std::path::PathBuf;
use std::process::Command;

/// `.git/info/exclude` is the human's own file, and every planning
/// workspace of the same project appends Build's two rules to it with the
/// app mutex released — so two Tasks planned at once are two writers.
/// A reader must see the file whole at every instant, and the human's own
/// rules must be there, once, when the writers are done.
#[test]
fn concurrent_planning_workspaces_never_shorten_the_humans_exclude_file() {
    let (dir, repo) = init_repo();
    let orch = std::sync::Arc::new(orchestrator(&dir, &repo));
    let info = repo.join(".git").join("info");
    std::fs::create_dir_all(&info).unwrap();
    let exclude = info.join("exclude");
    let human_rules: String = (0..4096)
        .map(|i| format!("scratch/notes-{i:04}.md\n"))
        .collect();
    std::fs::write(&exclude, &human_rules).unwrap();
    let expected = format!(
            "{human_rules}# Build's machine-local agent plumbing\n.build/mcp*.json\n.build/attachments/\n"
        );

    let writers: Vec<_> = (0..8)
        .map(|i| {
            let orch = std::sync::Arc::clone(&orch);
            let repo = repo.clone();
            std::thread::spawn(move || {
                orch.launch
                    .write_build_dir(&repo, &format!("plan-{i}"))
                    .unwrap();
            })
        })
        .collect();
    let torn = {
        let exclude = exclude.clone();
        let human_rules = human_rules.clone();
        let expected = expected.clone();
        std::thread::spawn(move || {
            let mut torn = Vec::new();
            for _ in 0..2000 {
                let seen = std::fs::read_to_string(&exclude).unwrap();
                if seen != human_rules && seen != expected {
                    torn.push(seen.len());
                }
            }
            torn
        })
    };
    for writer in writers {
        writer.join().unwrap();
    }
    let torn = torn.join().unwrap();

    assert!(
        torn.is_empty(),
        "a reader saw the exclude file part-written, at these lengths: {torn:?}"
    );
    assert_eq!(std::fs::read_to_string(&exclude).unwrap(), expected);
    let leftovers: Vec<_> = std::fs::read_dir(&info)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name != "exclude")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left beside exclude: {leftovers:?}"
    );
}
/// Play the plan agent's last move: write the stage manifest into the
/// plan's docs dir, where Build reads it on the Complete report.
pub(super) fn write_stage_manifest(plan: &ActivePlan, entries: &[StageManifestEntry]) {
    let plan_dir = plan_docs_dir(plan).join(".build/plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("stages.json"),
        serde_json::to_string(entries).unwrap(),
    )
    .unwrap();
}
/// The plan's live scratch docs dir (panics once the workspace is gone).
pub(super) fn plan_docs_dir(plan: &ActivePlan) -> PathBuf {
    plan.workspace
        .as_ref()
        .expect("plan has a live planning workspace")
        .docs_dir
        .clone()
}
pub(super) fn drafting_plan(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    goal: &str,
) -> ActivePlan {
    drafting_plan_and_turn(orch, store, id, goal).0
}
/// A door to a Task's planning agent, driven the way the app drives it:
/// gate it, prepare the workspace (the app does that with its mutex
/// released), open the session. One helper per door, so a test says which
/// door it is knocking on and nothing else has to know the order.
pub(super) fn start_plan_drafting(
    orch: &Orchestrator,
    active: &mut ActivePlan,
    store: &Store,
) -> Result<AgentTurn, OrchestratorError> {
    let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
    orch.open_plan_drafting(active, workspace)
}
pub(super) fn send_plan_notes(
    orch: &Orchestrator,
    active: &mut ActivePlan,
    store: &Store,
    notes: &str,
) -> Result<AgentTurn, OrchestratorError> {
    plan_transition(&active.plan.state, PlanEvent::SendNotes)?;
    let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
    orch.open_plan_notes(active, workspace, notes)
}
pub(super) fn send_plan_stage_notes(
    orch: &Orchestrator,
    active: &mut ActivePlan,
    store: &Store,
    stage_id: &str,
) -> Result<AgentTurn, OrchestratorError> {
    gate_plan_stage_notes(active, stage_id)?;
    let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
    orch.open_plan_stage_notes(active, workspace, stage_id)
}
pub(super) fn message_plan(
    orch: &Orchestrator,
    active: &mut ActivePlan,
    store: &Store,
    message: &str,
) -> Result<AgentTurn, OrchestratorError> {
    gate_plan_message(active, message)?;
    let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
    orch.open_plan_message(active, workspace, message)
}
pub(super) fn resume_plan(
    orch: &Orchestrator,
    active: &mut ActivePlan,
    store: &Store,
) -> Result<AgentTurn, OrchestratorError> {
    plan_transition(&active.plan.state, PlanEvent::Reply)?;
    let workspace = orch.prepare_plan_workspace(&active.plan.id.0, store)?;
    orch.open_plan_resume(active, workspace)
}
/// A dispatched plan plus the turn the dispatch wants said to its agent —
/// the orchestrator's whole output now that it owns no process.
pub(super) fn drafting_plan_and_turn(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    goal: &str,
) -> (ActivePlan, AgentTurn) {
    let mut active = orch.create_plan(PlanId::new(id), goal, "main", Default::default());
    let turn = start_plan_drafting(orch, &mut active, store).unwrap();
    (active, turn)
}
/// Play the plan agent: write a single plan doc and report done, landing
/// the plan at PlanReview with its docs ingested into the store.
pub(super) fn plan_in_review(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
    plan_in_review_with_goal(orch, store, id, "Add a greeting")
}
pub(super) fn plan_in_review_with_goal(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    goal: &str,
) -> ActivePlan {
    let mut plan = drafting_plan(orch, store, id, goal);
    let worktree_path = plan_docs_dir(&plan);
    std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v1\n").unwrap();
    orch.on_plan_done(&mut plan, store, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(plan.plan.state, PlanState::PlanReview);
    plan
}
pub(super) fn approved_plan(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
    approved_plan_with_goal(orch, store, id, "Add a greeting")
}
pub(super) fn approved_plan_with_goal(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    goal: &str,
) -> ActivePlan {
    let mut plan = plan_in_review_with_goal(orch, store, id, goal);
    orch.approve_plan(&mut plan).unwrap();
    plan
}
/// A plan whose agent produced `stage_count` stage docs plus the manifest,
/// driven to PlanReview (the docs are ingested into the store).
pub(super) fn multi_stage_plan_in_review(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    stage_count: usize,
) -> ActivePlan {
    let titles = ["First", "Second", "Third"];
    let mut plan = drafting_plan(orch, store, id, "Add greetings");
    let plan_dir = plan_docs_dir(&plan).join(".build/plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    let mut entries = Vec::new();
    for (position, title) in titles.iter().take(stage_count).enumerate() {
        let stage_id = title.to_lowercase();
        std::fs::write(
            plan_dir.join(format!("{:02}-{stage_id}.md", position + 1)),
            format!("# Stage: {title}\n"),
        )
        .unwrap();
        entries.push(manifest_entry(&stage_id, title, position + 1));
    }
    write_stage_manifest(&plan, &entries);
    orch.on_plan_done(&mut plan, store, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(plan.plan.state, PlanState::PlanReview);
    plan
}
pub(super) fn approved_multi_stage_plan(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    stage_count: usize,
) -> ActivePlan {
    let mut plan = multi_stage_plan_in_review(orch, store, id, stage_count);
    let stage_ids: Vec<String> = plan.stages.iter().map(|s| s.id.clone()).collect();
    for stage_id in stage_ids {
        orch.approve_plan_stage(&mut plan, &stage_id).unwrap();
    }
    orch.approve_plan(&mut plan).unwrap();
    plan
}
pub(super) fn dispatch_planned_run(
    orch: &Orchestrator,
    store: &Store,
    plan: &ActivePlan,
    id: &str,
) -> ActiveRun {
    dispatch_planned_run_and_turn(orch, store, plan, id).0
}
/// A dispatched run plus the turn the dispatch wants said to its agent.
pub(super) fn dispatch_planned_run_and_turn(
    orch: &Orchestrator,
    store: &Store,
    plan: &ActivePlan,
    id: &str,
) -> (ActiveRun, AgentTurn) {
    let task = ImplementableTask::judge(RunSource {
        plan,
        has_active_run: false,
    })
    .unwrap();
    let prepared = orch
        .prepare_run_checkout(&task, "main", id, Isolation::Worktree, store)
        .unwrap();
    orch.open_prepared_run(RunId::new(id), plan, prepared, Default::default())
        .unwrap()
}
#[tokio::test]
async fn a_drafting_plan_runs_on_the_primary_checkout_and_cuts_no_worktree() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");
    assert_eq!(plan.plan.state, PlanState::Drafting);
    let workspace = plan.workspace.as_ref().expect("a planning workspace");
    assert_eq!(
        workspace.checkout, repo,
        "a task's planning agent works in the project's primary checkout"
    );
    assert_eq!(
        registered_checkouts(&repo).len(),
        1,
        "planning cuts no worktree: {:?}",
        registered_checkouts(&repo)
    );
    assert!(
        !workspace.docs_dir.starts_with(&repo),
        "the scratch docs dir lives outside the repo: {}",
        workspace.docs_dir.display()
    );
    assert!(
        workspace.docs_dir.is_dir(),
        "the agent has a docs dir to write into: {}",
        workspace.docs_dir.display()
    );

    // The scaffolded MCP config routes `done` reports back to THIS plan,
    // and it lands in the checkout the agent actually runs in.
    let mcp = std::fs::read_to_string(repo.join(mcp_config_path("plan-1"))).unwrap();
    assert!(mcp.contains("plan-1"), "{mcp}");
}
/// The scratch docs dir keeps the name older builds gave it (#190), so a plan
/// being drafted when the bridge was upgraded resumes on the revision it had in
/// flight rather than on a fresh copy from the store.
#[tokio::test]
async fn a_plan_drafted_before_the_rename_resumes_in_its_own_scratch_docs() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let in_flight = dir.path().join("worktrees/.issue-docs/plan-1");
    std::fs::create_dir_all(&in_flight).unwrap();
    std::fs::write(in_flight.join("plan.md"), "# half a revision").unwrap();

    let plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

    let workspace = plan.workspace.as_ref().expect("a planning workspace");
    assert_eq!(workspace.docs_dir, in_flight);
    assert_eq!(
        std::fs::read_to_string(workspace.docs_dir.join("plan.md")).unwrap(),
        "# half a revision"
    );
}
/// Planning runs in the human's own checkout, so it must leave no trace
/// there: nothing to commit, and — critically — no untracked
/// `.build/.gitignore`, which would refuse to be overwritten by the merge
/// of any branch that carries one. The rules live in the repo-local
/// exclude file instead.
#[tokio::test]
async fn planning_leaves_the_primary_checkout_clean() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let _first = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

    assert!(
        !repo.join(".build/.gitignore").exists(),
        "an ignore file here would block every later merge"
    );
    let exclude_path = repo.join(".git/info/exclude");
    let exclude = std::fs::read_to_string(&exclude_path).unwrap();
    assert!(exclude.contains(".build/mcp*.json"), "{exclude}");
    assert!(exclude.contains(".build/attachments/"), "{exclude}");
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "",
        "planning dirties nothing in the primary checkout"
    );

    // A second task writes its own config and repeats no rule.
    let _second = drafting_plan(&orch, &store, "plan-2", "Add a farewell");
    let exclude = std::fs::read_to_string(&exclude_path).unwrap();
    assert_eq!(
        exclude
            .lines()
            .filter(|line| line.trim() == ".build/mcp*.json")
            .count(),
        1,
        "the exclude rules are written once: {exclude}"
    );
    assert!(repo.join(mcp_config_path("plan-2")).exists());
}
#[tokio::test]
async fn plan_done_ingests_docs_into_the_store_before_plan_review() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let plan = plan_in_review(&orch, &store, "plan-1");
    assert_eq!(plan.plan_path, ".build/plan.md");
    assert_eq!(plan.last_summary.as_deref(), Some("summary"));
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
        Some("# Plan v1\n"),
        "the store copy is canonical the moment the gate opens"
    );
}
#[tokio::test]
async fn plan_done_ingest_failure_keeps_the_plan_drafting_with_the_error_surfaced() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

    // The agent reports done but wrote NO docs: the ingest is transactional,
    // so the done errors and the plan never advances with unpersisted docs.
    let err = orch
        .on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .expect_err("ingest failure fails the done");
    assert!(matches!(err, OrchestratorError::Store(_)), "{err}");
    assert_eq!(plan.plan.state, PlanState::Drafting, "no state advance");
    assert!(
        plan.last_error
            .as_deref()
            .is_some_and(|e| e.contains("not persisted")),
        "{:?}",
        plan.last_error
    );
    assert_eq!(store.read_plan_doc("plan-1", ".build/plan.md"), None);
}
#[tokio::test]
async fn plan_blocked_and_failed_reports_park_the_plan() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let mut blocked = drafting_plan(&orch, &store, "plan-b", "goal b");
    orch.on_plan_done(&mut blocked, &store, done(DoneStatus::Blocked))
        .unwrap();
    assert_eq!(blocked.plan.state, PlanState::Blocked);
    assert_eq!(blocked.last_summary.as_deref(), Some("summary"));

    let mut failed = drafting_plan(&orch, &store, "plan-f", "goal f");
    orch.on_plan_done(&mut failed, &store, done(DoneStatus::Failed))
        .unwrap();
    assert_eq!(failed.plan.state, PlanState::Failed);
}
#[tokio::test]
async fn plan_idle_then_late_done_is_still_honored() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");

    orch.on_plan_idle(&mut plan).unwrap();
    assert_eq!(plan.plan.state, PlanState::IdleUnreported);

    // Quiescence never decided anything: the late report still lands.
    std::fs::write(plan_docs_dir(&plan).join(".build/plan.md"), "# Late plan\n").unwrap();
    orch.on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(plan.plan.state, PlanState::PlanReview);
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
        Some("# Late plan\n")
    );
}
#[tokio::test]
async fn plan_done_merges_the_manifest_into_stage_docs() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
    assert_eq!(plan.plan_path, templates::STAGES_MANIFEST_PATH);
    let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["first", "second"]);
    assert!(plan
        .stages
        .iter()
        .all(|s| s.state == StageDocState::Planned));
    assert_eq!(
        store
            .read_plan_doc("plan-1", ".build/plan/01-first.md")
            .as_deref(),
        Some("# Stage: First\n")
    );

    // A re-plan retitles "second" (doc state kept), drops "first", and
    // appends "third" — the plan side only carries doc review, so a
    // dropped id simply disappears (run progress is never deleted).
    plan.stages[1].state = StageDocState::Approved;
    send_plan_notes(&orch, &mut plan, &store, "restructure").unwrap();
    std::fs::write(
        plan_docs_dir(&plan).join(".build/plan/03-third.md"),
        "# Stage: Third\n",
    )
    .unwrap();
    write_stage_manifest(
        &plan,
        &[
            manifest_entry("second", "Second v2", 2),
            manifest_entry("third", "Third", 3),
        ],
    );
    orch.on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .unwrap();
    let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["second", "third"]);
    assert_eq!(plan.stages[0].title, "Second v2");
    assert_eq!(
        plan.stages[0].state,
        StageDocState::Approved,
        "an existing id keeps its review sub-state across a re-plan"
    );
    assert_eq!(plan.stages[1].state, StageDocState::Planned);
}
/// The manifest is read from disk on the Complete report, so a broken one
/// is Build's to name: the report is refused, the plan keeps drafting, and
/// the card says why — nothing is ingested and no stage is invented.
#[tokio::test]
async fn a_malformed_stage_manifest_refuses_the_plan_with_the_reason_on_the_card() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = drafting_plan(&orch, &store, "plan-1", "Add greetings");
    let plan_dir = plan_docs_dir(&plan).join(".build/plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(plan_dir.join("01-first.md"), "# Stage: First\n").unwrap();
    std::fs::write(plan_dir.join("stages.json"), "{\"not\": \"a list\"}").unwrap();

    let err = orch
        .on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .expect_err("a manifest Build cannot read refuses the plan");
    assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
    assert_eq!(plan.plan.state, PlanState::Drafting, "no state advance");
    assert!(plan.stages.is_empty(), "no stage is invented");
    let reason = plan.last_error.as_deref().unwrap_or_default();
    assert!(reason.contains("stages.json"), "{reason}");
    assert_eq!(
        plan.last_summary, None,
        "a refused report leaves no summary"
    );
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan/01-first.md"),
        None,
        "nothing is ingested past a refused manifest"
    );
}
/// With no stages.json on disk the plan is one document, whatever else the
/// docs dir holds.
#[tokio::test]
async fn a_plan_without_a_stage_manifest_stays_a_single_document() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = plan_in_review(&orch, &store, "plan-1");
    assert!(!plan.is_multi_stage());
    assert!(plan.stages.is_empty());
    assert_eq!(plan.plan_path, ".build/plan.md");
}
#[tokio::test]
async fn send_plan_notes_revises_in_the_warm_worktree() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");
    let worktree_path = plan_docs_dir(&plan);

    let turn = send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    assert_eq!(
        plan_docs_dir(&plan),
        worktree_path,
        "the worktree stays warm through the notes loop"
    );
    // The notes are a turn for the agent already drafting in that worktree,
    // not a prompt for a replacement.
    let cold = posted_turn_halves(&turn, "revise", "tighten step 2");
    assert!(
        cold.contains(".build/plan.md"),
        "a cold agent is pointed at the doc it must revise: {cold}"
    );

    // The revised doc lands in the store on the next done.
    std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v2\n").unwrap();
    orch.on_plan_done(&mut plan, &store, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(plan.plan.state, PlanState::PlanReview);
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
        Some("# Plan v2\n")
    );
}
#[tokio::test]
async fn send_plan_notes_refills_a_vanished_docs_dir_from_the_store() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");

    // The scratch docs dir was deleted out from under the plan. The docs
    // are canonical in the store, so a revision just refills it.
    let docs_dir = plan_docs_dir(&plan);
    std::fs::remove_dir_all(&docs_dir).unwrap();
    send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    assert_eq!(plan_docs_dir(&plan), docs_dir, "the same docs dir");
    assert_eq!(
        std::fs::read_to_string(docs_dir.join(".build/plan.md")).unwrap(),
        "# Plan v1\n",
        "docs re-materialized from the store before the turn is delivered"
    );
}
#[tokio::test]
async fn send_plan_notes_remakes_a_dropped_workspace_from_the_store() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");

    // Simulate the interrupted arm where the workspace is gone entirely
    // (a plan reattached after a restart holds none).
    let docs_dir = plan_docs_dir(&plan);
    std::fs::remove_dir_all(&docs_dir).unwrap();
    plan.workspace = None;
    send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    let workspace = plan.workspace.as_ref().expect("a workspace was remade");
    assert_eq!(workspace.checkout, repo, "still the primary checkout");
    assert_eq!(
        std::fs::read_to_string(workspace.docs_dir.join(".build/plan.md")).unwrap(),
        "# Plan v1\n"
    );
    // The checkout is fully scaffolded (done reports must route).
    assert!(repo.join(mcp_config_path("plan-1")).exists());
}
/// A revision in flight is never overwritten: the docs dir the agent is
/// working in is left exactly as the agent left it.
#[tokio::test]
async fn send_plan_notes_leaves_a_live_docs_dir_alone() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");

    let docs_dir = plan_docs_dir(&plan);
    std::fs::write(docs_dir.join(".build/plan.md"), "# Plan being revised\n").unwrap();
    send_plan_notes(&orch, &mut plan, &store, "tighten step 2").unwrap();

    assert_eq!(
        std::fs::read_to_string(docs_dir.join(".build/plan.md")).unwrap(),
        "# Plan being revised\n",
        "the store copy must not clobber the draft in flight"
    );
}
#[tokio::test]
async fn approve_plan_drops_the_scratch_docs_dir() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");
    let docs_dir = plan_docs_dir(&plan);

    orch.approve_plan(&mut plan).unwrap();
    assert_eq!(plan.plan.state, PlanState::Approved);
    assert_eq!(plan.workspace, None, "the workspace is gone");
    assert!(!docs_dir.exists(), "the scratch docs are gone with it");
    assert_eq!(
        registered_checkouts(&repo).len(),
        1,
        "planning never had a worktree to tear down"
    );
    // The canonical docs survive.
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
        Some("# Plan v1\n")
    );
}
/// The reviewer-facing bug this guards: a planning checkout cleaned up
/// outside Build made "Mark task ready" fail, because removing something
/// already absent was read as a failure. Approve only wants the scratch
/// docs gone — and they are.
#[tokio::test]
async fn approve_plan_succeeds_when_the_docs_dir_already_vanished() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");

    // Cleaned up behind the plan's back.
    std::fs::remove_dir_all(plan_docs_dir(&plan)).unwrap();

    orch.approve_plan(&mut plan)
        .expect("an already-gone docs dir is the goal, not a failure");
    assert_eq!(plan.plan.state, PlanState::Approved);
    assert!(plan.workspace.is_none(), "the plan lets the carcass go");
}
#[tokio::test]
async fn an_implementable_task_rejects_one_whose_first_stage_doc_is_unapproved() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    // A migrated plan can rest at Approved while a stage doc is Planned
    // (legacy records never re-gate); dispatch must still hold the line.
    let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    plan.stages[0].state = StageDocState::Planned;

    let Err(error) = ImplementableTask::judge(RunSource {
        plan: &plan,
        has_active_run: false,
    }) else {
        panic!("stage 0 must be approved before its build session spawns");
    };
    assert!(
        error.to_string().contains("first"),
        "the gate names the unapproved stage: {error}"
    );
}
#[tokio::test]
async fn plan_stage_revision_done_ingests_and_resets_approval() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
    plan.stages[0].state = StageDocState::Approved;
    let first_comment = comment_on(&mut plan, "first");
    let second_comment = comment_on(&mut plan, "first");

    // A stage-revision session is in flight for "first" (the dispatching
    // verb lands with the periphery; the state is arranged directly to
    // isolate the done handling).
    plan.plan.apply(crate::plan::PlanEvent::SendNotes).unwrap();
    plan.revising_stage_id = Some("first".into());
    std::fs::write(
        plan_docs_dir(&plan).join(".build/plan/01-first.md"),
        "# Stage: First (revised)\n",
    )
    .unwrap();

    orch.on_plan_done(
        &mut plan,
        &store,
        DoneReport::new(DoneStatus::Completed, "revised"),
    )
    .unwrap();

    assert_eq!(plan.plan.state, PlanState::PlanReview);
    assert_eq!(
        plan.stages[0].state,
        StageDocState::Planned,
        "a revised doc resets the stale approval"
    );
    assert_eq!(plan.revising_stage_id, None);
    for comment in [&first_comment, &second_comment] {
        assert_eq!(
            comment_by_id(&plan, comment).state,
            crate::thread::DocCommentState::Open,
            "a report resolves no comment; the reviewer does"
        );
    }
    assert_eq!(
        store
            .read_plan_doc("plan-1", ".build/plan/01-first.md")
            .as_deref(),
        Some("# Stage: First (revised)\n"),
        "the revision is ingested into the canonical store copy"
    );
}
#[test]
fn reattach_plan_mirrors_the_store_record() {
    let record = PersistedPlan {
        id: "plan-1".into(),
        goal: "add a greeting".into(),
        project_path: "/home/u/code/proj".into(),
        base_branch: "main".into(),
        state: crate::plan::PlanState::Interrupted,
        archived_at: Some("2026-07-01T10:06:00Z".into()),
        implementation_intent: crate::plan::ImplementationIntent::All,
        implementation_activity: crate::plan::ImplementationActivity::WaitingApproval(
            "second".into(),
        ),
        plan_path: ".build/plan.md".into(),
        stages: vec![],
        provider: crate::models::AgentProvider::Claude,
        model: Some("claude-opus-4-8".into()),
        effort: Some("xhigh".into()),
        agents: crate::agent::stored_agents("plan-1"),
        legacy_thread: crate::thread::Thread::default(),
        last_summary: Some("planned it".into()),
        last_error: Some("boom".into()),
        created_at: "2026-07-01T10:00:00Z".into(),
        updated_at: "2026-07-01T10:05:00Z".into(),
        state_changed_at: None,
    };
    let active = ActivePlan::reattach(&record);
    assert_eq!(active.plan.id.0, "plan-1");
    assert_eq!(active.plan.state, PlanState::Interrupted);
    assert_eq!(active.plan.archived_at, record.archived_at);
    assert_eq!(
        active.workspace, None,
        "a reattached plan holds no workspace: the next dispatch re-derives it"
    );
    assert_eq!(active.base_branch, "main");
    assert_eq!(
        active.model_choice.model.as_deref(),
        Some("claude-opus-4-8")
    );
    assert_eq!(active.last_error.as_deref(), Some("boom"));
}
#[tokio::test]
async fn dispatch_planned_run_materializes_commits_and_baselines_the_diff() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_plan(&orch, &store, "plan-1");

    let (mut run, turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
    assert_eq!(run.run.state, RunState::Building);
    assert_eq!(
        run.run.plan_id.as_ref().map(|p| p.0.as_str()),
        Some("plan-1")
    );
    assert_eq!(
        run.run.goal, "Add a greeting",
        "the run inherits the plan's goal"
    );

    // Dispatch order: materialize → commit ("plan: <goal>") → base_sha.
    assert_eq!(
        std::fs::read_to_string(run.worktree.path.join(".build/plan.md")).unwrap(),
        "# Plan v1\n"
    );
    assert_eq!(
        last_commit_subject(&run.worktree.path),
        "plan: Add a greeting"
    );
    let head = worktree_head(&run.worktree.path);
    assert_eq!(run.base_sha.as_deref(), Some(head.as_str()));

    // The materialized docs are the diff baseline — zero review noise…
    assert!(orch.run_diff(&run).unwrap().files().is_empty());
    // …while build-agent work (and any doc edits) still surface.
    std::fs::write(run.worktree.path.join("greeting.txt"), "hello\n").unwrap();
    let diff = orch.run_diff(&run).unwrap();
    let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["greeting.txt"]);

    // The build prompt points at the plan's doc, warm or cold.
    let prompt = dispatch_turn_halves(&turn, "build");
    assert!(prompt.contains(".build/plan.md"), "{prompt}");
    assert!(prompt.contains("Add a greeting"), "{prompt}");

    orch.on_run_done(&mut run, &[], done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::Review);
}
#[tokio::test]
async fn an_implementable_task_requires_an_approved_plan() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = plan_in_review(&orch, &store, "plan-1");

    let err = match ImplementableTask::judge(RunSource {
        plan: &plan,
        has_active_run: false,
    }) {
        Ok(_) => panic!("only an approved plan can be implemented"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("approved"), "{err}");
}
#[tokio::test]
async fn a_mid_plan_stage_complete_parks_at_the_gate_with_run_all_armed() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    run.auto_advance = true;
    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .unwrap();

    assert_eq!(run.run.state, RunState::StageGate);
    assert_eq!(run.stages[0].state, StageProgressState::Completed);
    assert!(
        run.auto_advance,
        "run-all stays armed after a completed stage"
    );
    assert_eq!(run.stages[0].completion_sha, run.stages[0].built_sha);
}
#[tokio::test]
async fn approve_plan_stage_approves_a_doc_and_rejects_on_terminal() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

    // An approved plan keeps taking per-stage approvals (that is how a run's
    // later stages get gated while an earlier one builds).
    orch.approve_plan(&mut plan).unwrap();
    orch.approve_plan_stage(&mut plan, "second").unwrap();
    assert_eq!(plan.stages[1].state, StageDocState::Approved);
    assert_eq!(plan.stages[0].state, StageDocState::Planned);

    // Double-approve is rejected by the pure doc-state machine.
    let err = orch
        .approve_plan_stage(&mut plan, "second")
        .expect_err("double approve is illegal");
    assert!(matches!(err, OrchestratorError::StageDoc(_)), "{err}");

    // Unknown stage id → a gate error, not a panic.
    assert!(matches!(
        orch.approve_plan_stage(&mut plan, "ghost"),
        Err(OrchestratorError::Gate(_))
    ));

    // A terminal plan takes no approvals.
    orch.abandon_plan(&mut plan).unwrap();
    let err = orch
        .approve_plan_stage(&mut plan, "first")
        .expect_err("no approvals on a terminal plan");
    assert!(err.to_string().contains("terminal"), "{err}");
}
#[tokio::test]
async fn send_plan_stage_notes_revises_a_stage_and_round_trips_through_done() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
    plan.stages[0].state = StageDocState::Approved;
    let first_comment = comment_on(&mut plan, "first");
    comment_on(&mut plan, "second");

    let turn = send_plan_stage_notes(&orch, &mut plan, &store, "first").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    assert_eq!(plan.revising_stage_id.as_deref(), Some("first"));
    let prompt = posted_turn_halves(&turn, "revise", NEW_THREAD_MESSAGES_PROMPT);
    assert!(!prompt.contains("read_unread_messages"), "{prompt}");
    assert_eq!(
        turn.warm, NEW_THREAD_MESSAGES_PROMPT,
        "delivery expands the notification with the pending native messages"
    );
    assert!(
        !prompt.contains("Comment:"),
        "no server-rendered comment block: {prompt}"
    );
    assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");

    // The agent revises the doc and reports done → back to PlanReview, the
    // stage approval reset, the store copy updated. The comment stays open:
    // resolving it is the reviewer's call.
    std::fs::write(
        plan_docs_dir(&plan).join(".build/plan/01-first.md"),
        "# Stage: First (revised)\n",
    )
    .unwrap();
    orch.on_plan_done(
        &mut plan,
        &store,
        DoneReport::new(DoneStatus::Completed, "revised"),
    )
    .unwrap();
    assert_eq!(plan.plan.state, PlanState::PlanReview);
    assert_eq!(plan.stages[0].state, StageDocState::Planned);
    assert_eq!(plan.revising_stage_id, None);
    assert_eq!(
        comment_by_id(&plan, &first_comment).state,
        crate::thread::DocCommentState::Open
    );
    assert_eq!(
        store
            .read_plan_doc("plan-1", ".build/plan/01-first.md")
            .as_deref(),
        Some("# Stage: First (revised)\n")
    );
}
#[tokio::test]
async fn send_plan_stage_notes_gates_on_plan_state_and_open_comments() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

    // No open comments on the stage.
    let err =
        send_plan_stage_notes(&orch, &mut plan, &store, "first").expect_err("no open comments");
    assert!(err.to_string().contains("no open comments"), "{err}");
    assert_eq!(plan.plan.state, PlanState::PlanReview);

    // Not at the review gate (approved) → the transition is rejected.
    orch.approve_plan(&mut plan).unwrap();
    comment_on(&mut plan, "first");
    let err = send_plan_stage_notes(&orch, &mut plan, &store, "first")
        .expect_err("an approved plan is past the review gate");
    assert!(err.to_string().contains("cannot send stage notes"), "{err}");
}
#[tokio::test]
async fn message_plan_redirects_drafting_resumes_parked_and_refuses_gates() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    // Empty message is refused before any state is touched.
    let mut plan = drafting_plan(&orch, &store, "plan-1", "Add a greeting");
    assert!(message_plan(&orch, &mut plan, &store, "   ")
        .unwrap_err()
        .to_string()
        .contains("empty"));

    // Drafting → a live redirect (no state change): the message is a turn
    // for the agent already drafting, never a replacement session.
    let turn = message_plan(&orch, &mut plan, &store, "focus on error paths").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    posted_turn_halves(&turn, "message", "focus on error paths");

    // A blocked plan resumes drafting on reply.
    orch.on_plan_done(&mut plan, &store, done(DoneStatus::Blocked))
        .unwrap();
    assert_eq!(plan.plan.state, PlanState::Blocked);
    let turn = message_plan(&orch, &mut plan, &store, "here is the missing detail").unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    posted_turn_halves(&turn, "message", "here is the missing detail");

    // The review gate refuses a side-channel message.
    let mut in_review = plan_in_review(&orch, &store, "plan-2");
    let err = message_plan(&orch, &mut in_review, &store, "sneak past the gate")
        .expect_err("review gate has send-notes");
    assert!(err.to_string().contains("review gate"), "{err}");
}
#[tokio::test]
async fn resume_plan_redispatches_an_interrupted_plan_remaking_its_workspace() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    // A plan interrupted mid-draft: its store docs survive, its scratch
    // docs dir is gone (what a restart leaves behind).
    let mut plan = plan_in_review(&orch, &store, "plan-1");
    // Move it back to a working phase then interrupt it.
    send_plan_notes(&orch, &mut plan, &store, "revise").unwrap();
    plan.plan.apply(crate::plan::PlanEvent::Interrupt).unwrap();
    let stale = plan.workspace.take().unwrap();
    std::fs::remove_dir_all(&stale.docs_dir).unwrap();

    let turn = resume_plan(&orch, &mut plan, &store).unwrap();
    assert_eq!(plan.plan.state, PlanState::Drafting);
    let workspace = plan
        .workspace
        .as_ref()
        .expect("resume remade the workspace");
    assert_eq!(workspace.checkout, repo, "still the primary checkout");
    assert!(
        workspace.docs_dir.join(".build/plan.md").exists(),
        "docs materialized"
    );
    // The re-plan instruction travels whether the agent is the one that was
    // interrupted or a fresh replacement.
    let prompt = dispatch_turn_halves(&turn, "revise");
    assert!(
        prompt.contains("Add a greeting"),
        "resume re-plans the same goal: {prompt}"
    );
}
#[tokio::test]
async fn abandon_plan_drops_the_docs_dir_and_keeps_the_store_docs() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = plan_in_review(&orch, &store, "plan-1");
    let docs_dir = plan_docs_dir(&plan);

    orch.abandon_plan(&mut plan).unwrap();
    assert_eq!(plan.plan.state, PlanState::Abandoned);
    assert_eq!(plan.workspace, None);
    assert!(!docs_dir.exists(), "the scratch docs are gone");
    assert_eq!(
        store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
        Some("# Plan v1\n"),
        "canonical docs survive an abandon"
    );
}
#[tokio::test]
async fn spawned_plan_and_run_prompts_put_the_ambiguity_rule_before_silent_directives() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let (_, plan_turn) = drafting_plan_and_turn(&orch, &store, "plan-1", "Add a greeting");
    dispatch_turn_halves(&plan_turn, "plan");
    let plan_prompt = plan_turn.cold;
    let plan = approved_plan(&orch, &store, "plan-of-run-1");
    let (_, run_turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
    dispatch_turn_halves(&run_turn, "build");
    let run_prompt = run_turn.cold;

    for (path, prompt) in [("plan", plan_prompt), ("run", run_prompt)] {
        let ambiguity_rule = prompt
            .find("either a question or an ambiguous directive")
            .unwrap_or_else(|| panic!("{path} spawn prompt lacks the ambiguity rule: {prompt}"));
        let silent_directive_allowance =
            prompt
                .find("directive without replying")
                .unwrap_or_else(|| {
                    panic!("{path} spawn prompt lacks the silent-directive allowance: {prompt}")
                });
        assert!(
            ambiguity_rule < silent_directive_allowance,
            "{path}: the ambiguity rule must precede the silent-directive allowance so an \
                 in-order reader hits the carve-out before committing to silence: {prompt}"
        );
    }
}
#[tokio::test]
async fn adopt_run_lands_in_review_as_a_plan_less_run() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let external = user_worktree(&dir, &repo, "wt-user", "user/thing");
    std::fs::write(external.path.join("notes.txt"), "pre-Build work\n").unwrap();

    let adoptable = AdoptableCheckout::judge(&external, "main").unwrap();
    orch.prepare_adoption(&adoptable, "main", "run-ad").unwrap();
    let run = orch
        .adopt_run(RunId::new("run-ad"), &adoptable, "main", Default::default())
        .unwrap();
    assert_eq!(run.run.state, RunState::Review);
    assert_eq!(run.run.plan_id, None, "an adopted run has no plan");
    assert_eq!(run.run.goal, "user/thing");
    assert_eq!(
        run.base_sha, None,
        "adopted runs baseline on the merge-base"
    );
    assert!(run.adopted);
    assert_eq!(
        last_commit_subject(&external.path),
        "Checkpoint: adopted by Build"
    );
}
#[tokio::test]
async fn mid_run_stage_revision_writes_back_to_the_plan_store() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    // The upcoming stage's doc was approved; a reviewer left a comment.
    plan.stages[1].state = StageDocState::Approved;
    let comment_id = comment_on(&mut plan, "second");
    let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");

    // The revision runs in the RUN's worktree; the run's coarse state is
    // untouched (it merely lends its worktree).
    let turn = orch
        .send_run_stage_notes(&mut run, &plan, "second")
        .unwrap();
    assert_eq!(run.run.state, RunState::StageGate);
    assert_eq!(run.revising_stage_id.as_deref(), Some("second"));
    // The revision is a turn for the run worktree's agent — the comments
    // themselves travel through MCP, so the turn only points at them.
    let cold = posted_turn_halves(&turn, "revise", NEW_THREAD_MESSAGES_PROMPT);
    assert!(
        cold.contains(".build/plan/02-second.md"),
        "a cold agent is pointed at the stage doc: {cold}"
    );

    // While a revision is in flight, a revise report must NOT go through
    // on_run_done — it is a store write-back, not a build report.
    let revise = DoneReport::new(DoneStatus::Completed, "revised");
    let guard = orch
        .on_run_done(&mut run, &plan.stages, revise.clone())
        .expect_err("on_run_done rejects a revision in flight");
    assert!(
        guard.to_string().contains("consume_run_stage_revision"),
        "{guard}"
    );

    // The agent revised the doc in the run's worktree; consuming ingests it
    // back to the plan store and resets the stale approval; the comment stays
    // open for the reviewer to resolve.
    std::fs::write(
        run.worktree.path.join(".build/plan/02-second.md"),
        "# Stage: Second (reworked)\n",
    )
    .unwrap();
    orch.consume_run_stage_revision(&mut run, &mut plan, &store, &revise)
        .unwrap();
    assert_eq!(run.revising_stage_id, None);
    assert_eq!(
        run.run.state,
        RunState::StageGate,
        "the build did not advance"
    );
    assert_eq!(
        plan.stages[1].state,
        StageDocState::Planned,
        "a revised doc resets its stale approval"
    );
    assert_eq!(
        comment_by_id(&plan, &comment_id).state,
        crate::thread::DocCommentState::Open
    );
    assert_eq!(
        store
            .read_plan_doc("plan-1", ".build/plan/02-second.md")
            .as_deref(),
        Some("# Stage: Second (reworked)\n"),
        "the revision reached the canonical store copy"
    );
}
#[tokio::test]
async fn send_run_stage_notes_is_only_legal_at_the_stage_gate() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    comment_on(&mut plan, "first");
    // A run still building its first stage is not at the gate.
    let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
    let err = orch
        .send_run_stage_notes(&mut run, &plan, "first")
        .expect_err("not at the stage gate");
    assert!(err.to_string().contains("stage gate"), "{err}");
}
