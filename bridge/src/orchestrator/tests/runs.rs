use super::super::reporting::as_merge_failure;
use super::plans::{
    approved_multi_stage_plan, approved_plan, approved_plan_with_goal, dispatch_planned_run,
    dispatch_planned_run_and_turn,
};
use super::reporting::done;
use super::workspace::worktree_head;
use crate::git_fixture::init_repo;
use crate::git_process::run_git;
use crate::harness::HarnessError;
use crate::mcp::DoneStatus;
use crate::models::ModelChoice;
use crate::orchestrator::{
    conversation_prompt, mcp_config_path, ActivePlan, ActiveRun, Agent, AgentTurn,
    ImplementableTask, Orchestrator, OrchestratorError, RunSource,
};
use crate::plan::{StageDocState, StageManifestEntry};
use crate::pty::HarnessSpec;
use crate::run::{RunState, StageProgressState};
use crate::store::{PersistedRun, Store};
use crate::templates::Templates;
use crate::worktree::WorktreeError;
use std::path::Path;
use std::process::Command;

/// A warm "harness" that stays alive and drains stdin (it discards the
/// prompt), like a real interactive CLI. Draining matters: a child that never
/// reads lets the PTY's canonical-mode input queue fill, so writing a
/// full-size rendered prompt would block and then fail with EIO. The startup
/// byte matters too: like a real TUI painting its screen, it satisfies the
/// spawn's readiness wait so dispatches don't idle out the grace. The test
/// plays the agent: it writes files and forwards `done` reports.
pub(super) fn warm_harness() -> HarnessSpec {
    HarnessSpec::new("sh")
        .arg("-c")
        .arg("printf '\\033[?2004h'; cat >/dev/null")
}
pub(super) fn orchestrator(dir: &tempfile::TempDir, repo: &Path) -> Orchestrator {
    Orchestrator::new(
        repo.to_path_buf(),
        dir.path().join("worktrees"),
        Agent::Warm(warm_harness()),
        Templates::default(),
        std::fs::canonicalize(repo.join("README.md")).unwrap(),
    )
}
#[test]
fn prepared_agent_launch_preserves_setup_errors() {
    let (dir, repo) = init_repo();
    let worktree = dir.path().join("failed-worktree");
    std::fs::create_dir(&worktree).unwrap();
    let agent = Agent::WarmBuilder(std::sync::Arc::new(|_, _, _| {
        Err(HarnessError::Setup("injected Pi setup failure".to_string()))
    }));
    let orchestrator = Orchestrator::new(
        repo.clone(),
        dir.path().join("worktrees"),
        agent,
        Templates::default(),
        std::fs::canonicalize(repo.join("README.md")).unwrap(),
    );
    let error = orchestrator
        .agent_launch()
        .prepare(
            "agent-fail",
            crate::orchestrator::LaunchDirs::at(&worktree),
            &ModelChoice::default(),
            false,
            None,
            "token",
        )
        .unwrap_err();
    assert!(matches!(
        error,
        OrchestratorError::Harness(HarnessError::Setup(message))
            if message == "injected Pi setup failure"
    ));
}
pub(super) fn manifest_entry(id: &str, title: &str, position: usize) -> StageManifestEntry {
    StageManifestEntry {
        id: id.into(),
        title: title.into(),
        path: format!(".build/plan/{position:02}-{id}.md"),
        summary: format!("{title}."),
    }
}
pub(super) fn last_commit_subject(worktree: &Path) -> String {
    let out = Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(worktree)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
pub(super) fn split_store(dir: &tempfile::TempDir) -> Store {
    Store::new(dir.path().join("store")).expect("store opens")
}
/// Leave a reviewer comment on one stage. A comment is a post on the
/// Task's conversation and nowhere else, so this is how a test makes one.
/// Returns the comment's id.
pub(super) fn comment_on(plan: &mut ActivePlan, stage_id: &str) -> String {
    let path = plan
        .stages
        .iter()
        .find(|stage| stage.id == stage_id)
        .map(|stage| stage.path.clone())
        .unwrap_or_default();
    let task_id = plan.plan.id.0.clone();
    plan.agents.primary_mut().unwrap().thread.post_doc_comment(
        &task_id,
        stage_id,
        &path,
        None,
        format!("comment on {stage_id}"),
        "2026-08-13T09:00:00Z",
    )
}
/// One comment as the conversation now holds it.
pub(super) fn comment_by_id(plan: &ActivePlan, comment_id: &str) -> crate::thread::DocComment {
    plan.agents
        .sole_thread()
        .doc_comments()
        .into_iter()
        .find(|comment| comment.id == comment_id)
        .unwrap_or_else(|| panic!("no comment {comment_id}"))
}
/// Assert both halves of the cold/warm rule on a DISPATCHED turn (one whose
/// whole content is the rendered prompt: a dispatch, a resume), and hand back the warm half to assert content on.
///
/// Both halves matter equally: the caller cannot know which one will travel
/// — that depends on whether it had to spawn a harness — so a turn that
/// renders only the half a test happens to look at reaches the other kind of
/// agent with nothing.
pub(super) fn dispatch_turn_halves(turn: &AgentTurn, phase: &str) -> String {
    assert_eq!(turn.phase, phase, "turn phase: {turn:?}");
    assert!(
        !turn.warm.is_empty(),
        "a turn with nothing to say: {turn:?}"
    );
    assert!(
        !turn.warm.contains("Build conversation protocol"),
        "an agent already in the conversation is not re-taught the protocol: {}",
        turn.warm
    );
    assert!(
        turn.cold.starts_with(&turn.warm),
        "cold is the warm instruction plus the conversation it missed — cold {:?}, warm {:?}",
        turn.cold,
        turn.warm
    );
    assert!(
        turn.cold.contains("Build conversation protocol"),
        "a spawned agent gets the protocol: {}",
        turn.cold
    );
    turn.warm.clone()
}
/// Assert both halves of the cold/warm rule on a POSTED turn (a change
/// request, a message, a batch of notes): the payload is already durable on
/// the thread, so a warm agent hears only `nudge` while a cold one gets the
/// same instruction wrapped in the run/plan context it cannot reconstruct.
pub(super) fn posted_turn_halves(turn: &AgentTurn, phase: &str, nudge: &str) -> String {
    assert_eq!(turn.phase, phase, "turn phase: {turn:?}");
    assert_eq!(
        turn.warm, nudge,
        "an agent already in the conversation hears the instruction alone"
    );
    assert!(
        turn.cold.contains(nudge),
        "the instruction travels cold too: {}",
        turn.cold
    );
    assert!(
        turn.cold.contains("Build conversation protocol"),
        "a spawned agent gets the protocol: {}",
        turn.cold
    );
    turn.cold.clone()
}
/// A run implementing a single-doc plan: one build session, no stage
/// pipeline. This is what the retired goal-only dispatch used to stand in
/// for, so it is the fixture for every run-side test that only needs "a run
/// the agent is building in". The plan id derives from the run id, so
/// repeated calls inside one test never collide.
pub(super) fn dispatch_single_stage_run(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
    goal: &str,
) -> ActiveRun {
    let plan = approved_plan_with_goal(orch, store, &format!("plan-of-{id}"), goal);
    dispatch_planned_run(orch, store, &plan, id)
}
#[test]
fn reattach_run_mirrors_the_store_record() {
    let record = PersistedRun {
        id: "run-1".into(),
        plan_id: Some("plan-1".into()),
        goal: "add a greeting".into(),
        project_path: "/home/u/code/proj".into(),
        base_branch: "main".into(),
        state: crate::run::RunState::Interrupted,
        branch: "build/add-a-greeting".into(),
        worktree_name: "add-a-greeting".into(),
        worktree_path: "/tmp/wt/add-a-greeting".into(),
        base_sha: Some("deadbeef".into()),
        stages: vec![crate::run::StageProgress::dispatched("first")],
        current_stage_id: Some("first".into()),
        revising_stage_id: None,
        auto_advance: true,
        adopted: true,
        publication_attempt: None,
        provider: crate::models::AgentProvider::Claude,
        model: None,
        effort: Some("high".into()),
        agents: crate::agent::stored_agents("run-1"),
        legacy_thread: crate::thread::Thread::default(),
        last_summary: Some("built it".into()),
        last_error: None,
        created_at: "2026-07-01T10:00:00Z".into(),
        updated_at: "2026-07-01T10:05:00Z".into(),
        state_changed_at: None,
    };
    let active = ActiveRun::reattach(&record, ".build/plan.md".into());
    assert_eq!(active.run.id.0, "run-1");
    assert_eq!(
        active.run.plan_id.as_ref().map(|p| p.0.as_str()),
        Some("plan-1")
    );
    assert_eq!(active.run.state, RunState::Interrupted);
    assert_eq!(active.worktree.branch(), "build/add-a-greeting");
    assert_eq!(active.base_sha.as_deref(), Some("deadbeef"));
    assert_eq!(active.plan_path, ".build/plan.md");
    assert_eq!(active.stages.len(), 1);
    assert_eq!(active.current_stage_id.as_deref(), Some("first"));
    assert!(active.auto_advance);
    assert!(active.adopted);
    assert_eq!(active.model_choice.effort.as_deref(), Some("high"));
}
#[tokio::test]
async fn dispatch_single_stage_run_goes_straight_to_building() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
    assert_eq!(run.run.state, RunState::Building);
    assert_eq!(
        run.run.plan_id.as_ref().map(|id| id.0.as_str()),
        Some("plan-of-run-1"),
        "every run implements a plan"
    );
    assert!(
        run.base_sha.is_some(),
        "the materialized plan doc baselines the review diff"
    );
    assert!(run.worktree.branch().starts_with("build/"));
    assert!(run.worktree.path.join(mcp_config_path("run-1")).exists());

    std::fs::write(run.worktree.path.join("fix.txt"), "fixed\n").unwrap();
    orch.on_run_done(&mut run, &[], done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::Review);
    let diff = orch.run_diff(&run).unwrap();
    assert!(diff.files().iter().any(|f| f.path == "fix.txt"));
}
#[tokio::test]
async fn an_implementable_task_enforces_the_single_active_writer_rule() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_plan(&orch, &store, "plan-1");

    let err = match ImplementableTask::judge(RunSource {
        plan: &plan,
        has_active_run: true,
    }) {
        Ok(_) => panic!("a second concurrent run of the same plan must be rejected"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("active run"), "{err}");
}
#[tokio::test]
async fn dispatch_multi_stage_run_starts_the_first_stage() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);

    let (run, turn) = dispatch_planned_run_and_turn(&orch, &store, &plan, "run-1");
    assert_eq!(run.run.state, RunState::Building);
    assert_eq!(run.current_stage_id.as_deref(), Some("first"));
    assert_eq!(run.stages.len(), 1);
    assert_eq!(run.stages[0].state, StageProgressState::Building);
    assert_eq!(
        run.stages[0].start_sha, run.base_sha,
        "the first stage's diff starts at the materialization commit"
    );
    assert!(run.worktree.path.join(".build/plan/01-first.md").exists());
    let prompt = dispatch_turn_halves(&turn, "build");
    assert!(prompt.contains("Execute ONE stage"), "{prompt}");
    assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");
    assert!(
        prompt.contains("Ordered Task stage-plan catalog"),
        "{prompt}"
    );
    let first = prompt
        .find("first — First")
        .expect("first stage in catalog");
    let second = prompt
        .find("second — Second")
        .expect("second stage in catalog");
    assert!(first < second, "catalog preserves manifest order: {prompt}");
    assert!(
        turn.cold.contains(
            "Act on the current instruction and exact accepted messages in the native payload"
        ),
        "cold Task agents learn how native reviewer messages are delivered: {}",
        turn.cold
    );
    assert!(!turn.cold.contains("read_unread_messages"), "{}", turn.cold);
}
/// Drive a two-stage planned run through its first stage, leaving it parked
/// at the between-stages gate with stage one `Completed`.
pub(super) fn run_past_first_stage(
    orch: &Orchestrator,
    store: &Store,
    plan: &ActivePlan,
    id: &str,
) -> ActiveRun {
    let mut run = dispatch_planned_run(orch, store, plan, id);
    std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
    orch.on_run_done(&mut run, &plan.stages, done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(run.run.state, RunState::StageGate);
    run
}
#[tokio::test]
async fn dispatch_run_stage_enforces_the_sequential_gate_and_pins_start_sha() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");
    // Un-approve the second doc (a mid-run revision resets approval the
    // same way) so the gate has something to refuse.
    plan.stages[1].state = StageDocState::Planned;

    // The next stage's doc is not approved yet → refused.
    let err = orch
        .dispatch_run_stage(&mut run, &plan.stages, "second", None)
        .expect_err("an unapproved stage cannot dispatch");
    assert!(err.to_string().contains("not approved"), "{err}");
    assert_eq!(
        run.run.state,
        RunState::StageGate,
        "no state change on refusal"
    );

    // Approve it → the sequential gate opens (stage one completed).
    orch.approve_plan_stage(&mut plan, "second").unwrap();
    let turn = orch
        .dispatch_run_stage(&mut run, &plan.stages, "second", None)
        .unwrap();
    assert_eq!(run.run.state, RunState::Building);
    assert_eq!(run.current_stage_id.as_deref(), Some("second"));
    let second = run.stage_progress("second").unwrap();
    assert_eq!(second.state, StageProgressState::Building);
    assert_eq!(
        second.start_sha.as_deref(),
        Some(worktree_head(&run.worktree.path).as_str()),
        "the stage diff pins to HEAD at dispatch"
    );
    let prompt = dispatch_turn_halves(&turn, "build");
    assert!(prompt.contains(".build/plan/02-second.md"), "{prompt}");
}
/// The sequential gate reads the run's own progress: a stage whose
/// predecessor never completed on this run cannot be dispatched, even from
/// the stage gate.
#[tokio::test]
async fn dispatch_run_stage_rejects_a_stage_whose_predecessor_has_not_completed() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);
    let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
    let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");
    // A record whose first stage never finished (an older store shape, a
    // hand-edited record) parked at the gate.
    run.stages[0].state = StageProgressState::Building;

    let err = orch
        .dispatch_run_stage(&mut run, &plan.stages, "second", None)
        .expect_err("stage one has not completed");
    assert!(err.to_string().contains("has not completed"), "{err}");
    assert_eq!(run.run.state, RunState::StageGate, "no state change");
}
/// Requesting changes hands the caller a turn to deliver; it never ends the
/// worktree's agent nor spawns a replacement. The turn carries both halves
/// of the cold/warm rule: the full run-context prompt for an agent that had
/// to be spawned, and the caller's bare instruction for one already in the
/// conversation.
#[tokio::test]
async fn run_request_changes_returns_a_revise_turn_and_never_respawns() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
    orch.on_run_done(&mut single, &[], done(DoneStatus::Completed))
        .unwrap();
    assert_eq!(single.run.state, RunState::Review);

    let turn = orch
        .run_request_changes(&mut single, &[], "tweak it", None)
        .unwrap();
    assert_eq!(single.run.state, RunState::Building);
    let cold = posted_turn_halves(&turn, "revise", "tweak it");
    assert!(
        cold.contains("The reviewer requested changes on your diff"),
        "a cold agent gets the whole revise prompt: {cold}"
    );
}
#[tokio::test]
async fn message_run_redirects_building_continues_and_refuses_gates() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
    assert!(orch
        .message_run(&mut run, &[], "  ")
        .unwrap_err()
        .to_string()
        .contains("empty"));

    // Building → the run keeps working and the message becomes a turn.
    let turn = orch
        .message_run(&mut run, &[], "also handle the empty case")
        .unwrap();
    assert_eq!(run.run.state, RunState::Building);
    posted_turn_halves(&turn, "message", "also handle the empty case");

    // The review gate refuses a message (request-changes is the verb there).
    orch.on_run_done(&mut run, &[], done(DoneStatus::Completed))
        .unwrap();
    let err = orch
        .message_run(&mut run, &[], "sneak past")
        .expect_err("review gate refuses messages");
    assert!(err.to_string().contains("review gate"), "{err}");
}
/// Every cold prompt, on every carrier, tells the agent to name its
/// conversation first: the header wears that name in place of the harness
/// name, and says "Starting" until it arrives.
#[test]
fn conversation_prompt_tells_the_agent_to_set_the_topic_first() {
    let prompt = conversation_prompt("do the work");
    assert!(prompt.contains("`set_topic`"), "{prompt}");
    assert!(
        prompt.contains("2-4 words"),
        "the shape of a topic is stated where the tool is named: {prompt}"
    );
}

/// Every cold prompt teaches the reference shapes (#229): an agent started
/// in a workspace never saw them, since only the phase and project-agent
/// templates carried the note. The shapes are spa/src/core/markdownRefs.js's.
#[test]
fn conversation_prompt_teaches_every_reference_shape() {
    let prompt = conversation_prompt("do the work");
    let bullet = prompt
        .lines()
        .find(|line| line.starts_with("- Link to Build things by reference"))
        .unwrap_or_else(|| panic!("no reference bullet: {prompt}"));
    for shape in [
        "`#42`",
        "`#42/c/<comment-id>`",
        "`@agent:<agent-id>`",
        "`@workspace:<name or id>`",
        "`@project:<name or id>`",
        "`[[<workspace>:<path>#L10]]`",
        "`[[<workspace>:commit:<sha>]]`",
        "`[[<workspace>/<directory>:<path>]]`",
    ] {
        assert!(bullet.contains(shape), "{shape}: {bullet}");
    }
}

/// Every cold prompt says where the user is talked to (#232): the thread, and
/// the tasks they are on, in the one wording the tool descriptions use.
#[test]
fn conversation_prompt_says_where_the_user_is_talked_to() {
    use crate::test_support::user_involvement::*;
    let prompt = conversation_prompt("do the work");
    for rule in [
        ON_A_TASK,
        REPLY_THERE,
        ASKED_IN_THREAD,
        RESULT_ON_TASK,
        BUILD_ON,
    ] {
        assert!(says(&prompt, rule), "{rule:?} missing: {prompt}");
    }
    assert!(
        says(
            &prompt,
            "The user reads this thread and the Build tasks they are on."
        ),
        "{prompt}"
    );
    assert!(
        !prompt.contains("The user sees only messages sent through"),
        "the thread is not the only place the user reads: {prompt}"
    );
}

#[test]
fn conversation_prompt_instructs_clarifying_reply_for_ambiguous_comments() {
    let prompt = conversation_prompt("do the work");
    assert!(prompt.contains("Build conversation protocol"), "{prompt}");
    assert!(
        prompt.contains("either a question or an ambiguous directive"),
        "ambiguous reviewer messages must trigger a clarifying reply: {prompt}"
    );
    assert!(
        prompt.contains("one-line clarifying reply"),
        "the reply must be a one-liner, not a silent code change: {prompt}"
    );
}

#[test]
fn conversation_prompt_uses_available_native_progress_tools_for_long_running_work() {
    let prompt = conversation_prompt("do the work");
    assert!(
        prompt.contains("long-running work with several meaningful steps"),
        "multi-step work should be visible to the user: {prompt}"
    );
    assert!(
        prompt.contains("native checklist or plan tool available in this session"),
        "each provider should use its own supported progress surface: {prompt}"
    );
    // Build's tasks are cards on the board (#190). A progress rule that said
    // "task" would send an agent to TaskCreate thinking it filed one.
    let progress = prompt
        .lines()
        .find(|line| line.contains("long-running work with several meaningful steps"))
        .unwrap();
    assert!(
        !progress.to_lowercase().contains("task"),
        "the progress rule must not say task: {progress}"
    );
    assert!(
        prompt.contains("brief, one-step work"),
        "quick work should not create needless checklists: {prompt}"
    );
    assert!(
        prompt.contains("do not invent a tool that is absent from your tool list"),
        "the prompt must not ask a provider to call an unsupported tool: {prompt}"
    );
    assert!(
        prompt.contains("update it at each meaningful advance"),
        "progress must stay current throughout the work: {prompt}"
    );
    assert!(
        prompt.contains("every item complete only after it is complete"),
        "completion must be accurately reported: {prompt}"
    );
}

#[tokio::test]
async fn resume_run_redispatches_single_stage_and_multi_stage_builds() {
    let (dir, repo) = init_repo();
    let orch = orchestrator(&dir, &repo);
    let store = split_store(&dir);

    // A single-stage run interrupted mid-build → resumes the whole-run build.
    let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
    single.run.apply(crate::run::RunEvent::Interrupt).unwrap();
    let turn = orch.resume_run(&mut single, &[]).unwrap();
    assert_eq!(single.run.state, RunState::Building);
    let prompt = dispatch_turn_halves(&turn, "resume");
    assert!(prompt.contains("single stage work"), "{prompt}");

    // Multi-stage run interrupted mid stage-build → resumes THAT stage.
    let orch2 = Orchestrator::new(
        repo.to_path_buf(),
        dir.path().join("worktrees2"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        std::fs::canonicalize(repo.join("README.md")).unwrap(),
    );
    let plan = approved_multi_stage_plan(&orch2, &store, "plan-1", 2);
    let mut run = dispatch_planned_run(&orch2, &store, &plan, "run-1");
    run.run.apply(crate::run::RunEvent::Interrupt).unwrap();
    let turn = orch2.resume_run(&mut run, &plan.stages).unwrap();
    assert_eq!(run.run.state, RunState::Building);
    let prompt = dispatch_turn_halves(&turn, "resume");
    assert!(
        prompt.contains(".build/plan/01-first.md"),
        "resumes stage one: {prompt}"
    );
}
/// git's failures reach the orchestrator through one door, keeping the
/// message the RPC surfaces the same one the child gave.
#[test]
fn a_failed_git_child_arrives_as_a_git_failure() {
    let dir = tempfile::tempdir().unwrap();

    let failure: OrchestratorError = run_git(dir.path(), &["rev-parse", "--verify", "HEAD"])
        .unwrap_err()
        .into();

    assert!(matches!(failure, OrchestratorError::Git(_)), "{failure}");
    assert!(failure.to_string().contains("rev-parse"), "{failure}");
}
/// A refusal the manager composed itself is already the sentence to show:
/// wrapping it in a merge failure adds the prefix the web client keys on
/// and nothing else.
#[test]
fn a_refused_merge_reads_as_the_refusal_itself() {
    let refusal = OrchestratorError::Worktree(WorktreeError::Refused(
        "primary checkout is on \"elsewhere\"".to_string(),
    ));

    assert_eq!(
        as_merge_failure(refusal).to_string(),
        "merge_failed: primary checkout is on \"elsewhere\""
    );
}
