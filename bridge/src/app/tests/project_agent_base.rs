//! The project agent stands in its project's base (#187): it reads the code
//! there and changes nothing, while everything Build keeps for it — the
//! owner's identity, the agent's `.build/` scaffold, its session lineage and
//! the conversation's attachments — stays in the scratch root the owner was
//! minted over.

use super::project_agent::{added_project, context, project_agent, rooted, workspace};
use super::*;

/// Put the project's agents on a harness that prints where it was started and
/// stays up, and keep every [`SpawnOptions`] a spawn was built from.
fn record_spawns(state: &Arc<Mutex<AppState>>) -> Arc<Mutex<Vec<SpawnOptions>>> {
    let spawns: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawns);
    let mut s = state.lock().unwrap();
    let repo = s.project_at(0).repo_path.clone();
    let worktrees = s.worktrees_root.clone();
    let agent = Agent::WarmBuilder(Arc::new(
        move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
            recorded.lock().unwrap().push(options.clone());
            Ok(HarnessSpec::new("bash")
                .arg("-c")
                .arg("printf 'started in %s\\n' \"$PWD\"; exec sleep 60"))
        },
    ));
    s.project_at_mut(0).orch = Orchestrator::new(
        repo,
        worktrees,
        agent,
        Templates::default(),
        test_bridge_exe(),
    );
    s.resume_id_probe = Arc::new(|_, _, _| false);
    spawns
}

fn only_spawn(spawns: &Arc<Mutex<Vec<SpawnOptions>>>) -> SpawnOptions {
    let spawns = spawns.lock().unwrap();
    assert_eq!(spawns.len(), 1, "{spawns:?}");
    spawns[0].clone()
}

/// A project's conversation owner, its agent, the scratch root the owner was
/// minted over, and the project's base.
struct ProjectAgent {
    owner: String,
    agent_id: String,
    scratch: PathBuf,
    base: PathBuf,
}

fn a_project_agent(state: &mut AppState, repo: &Path) -> ProjectAgent {
    let project_id = added_project(state, repo);
    let (owner, agent_id) = project_agent(state, &project_id);
    ProjectAgent {
        scratch: state.runs[&owner].worktree.path.clone(),
        base: state.project_base(&project_id).expect("the project's base"),
        owner,
        agent_id,
    }
}

fn git_status(repo: &Path) -> String {
    let out = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

// ---- where the agent stands ------------------------------------------------

/// The project agent's process starts in the project's base, and nothing of
/// Build's is written there: the scaffold it reads its MCP config from stays
/// in the scratch root, and the harness is handed that config by an absolute
/// path because a relative one would be read from the base.
#[tokio::test]
async fn a_project_agent_starts_in_its_projects_base_and_writes_nothing_there() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    assert_eq!(project.base, repo, "the base is the project's own path");
    let clean = git_status(&repo);
    let state = state.shared();
    let spawns = record_spawns(&state);

    let (_, spawned) = ensure_agent_tab(
        &state,
        &project.scratch,
        &project.owner,
        &project.agent_id,
        &ModelChoice::default(),
        "start",
    )
    .expect("the project agent starts");
    assert_eq!(spawned, Spawned::Fresh);

    let options = only_spawn(&spawns);
    assert_eq!(options.cwd, repo, "the agent is started in the base");
    assert_eq!(options.scaffold.as_deref(), Some(project.scratch.as_path()));
    let config = project
        .scratch
        .join(crate::orchestrator::mcp_config_path(&project.agent_id));
    assert!(config.is_file(), "{}", config.display());
    assert_eq!(options.mcp_config(), config.display().to_string());

    // The child really stands there, and its tab is still Build's by the
    // scratch root: that is the key every sweep, attach and lineage reads.
    let screen = wait_for_agent_screen(&state, &project.scratch, "started in").await;
    assert!(
        screen.contains(&format!("started in {}", repo.display())),
        "{screen}"
    );
    assert!(!repo.join(".build").exists(), "no scaffold in the base");
    assert_eq!(git_status(&repo), clean, "nothing written into the base");
}

/// Only a project's own conversation stands elsewhere. An agent on a workspace
/// is started in the workspace it works in, with its scaffold beside it.
#[tokio::test]
async fn a_workspace_agent_still_starts_in_its_own_root() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "beside-the-base");
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent_id = state.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    let root = AppState::canonical_root(&state.runs[&owner].worktree.path);
    let state = state.shared();
    let spawns = record_spawns(&state);

    ensure_agent_tab(
        &state,
        &root,
        &owner,
        &agent_id,
        &ModelChoice::default(),
        "start",
    )
    .expect("the workspace agent starts");

    let options = only_spawn(&spawns);
    assert_eq!(options.cwd, root);
    assert_eq!(options.scaffold, None);
    assert_eq!(
        options.mcp_config(),
        crate::orchestrator::mcp_config_path(&agent_id),
        "a workspace agent's config is read from where it stands"
    );
}

/// A project whose folder has gone still gets its agent: it is started in its
/// scratch root rather than refused a start over a directory it only reads.
#[test]
fn a_project_whose_base_is_gone_starts_its_agent_in_scratch() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);

    assert_eq!(
        state.agent_process_cwd(&project.owner, &project.scratch),
        repo
    );
    std::fs::rename(&repo, repo.with_extension("moved")).unwrap();
    assert_eq!(
        state.agent_process_cwd(&project.owner, &project.scratch),
        project.scratch
    );
}

// ---- what it is told -------------------------------------------------------

/// Its standing instructions say where it stands, that it may read anything
/// there, that it changes nothing there, and that every change goes through a
/// workspace. A project with several sources names the others.
#[test]
fn the_project_agents_prompt_names_its_base_and_forbids_changing_it() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let other_home = tempfile::tempdir().unwrap();
    let other = init_repo_named(other_home.path(), "docs");
    let other = std::fs::canonicalize(&other).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);

    let prompt = state.project_agent_prompt(&project.owner);
    let said = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    for rule in [
        format!("You stand in the project's base, {}:", repo.display()),
        "Read anything in it".to_string(),
        "Never change it: do not edit, check out, build or commit there".to_string(),
        "Every change goes through a workspace: `create_workspace` and an agent on it.".to_string(),
    ] {
        assert!(said.contains(&rule), "{rule:?} missing from {said}");
    }
    assert!(!said.contains("this directory is scratch"), "{said}");
    assert!(!said.contains("{project_"), "a placeholder left: {said}");
    assert!(!said.contains("other sources"), "one source: {said}");

    let project_id = state.project_of(&project.owner).unwrap();
    let added = state.handle(req(
        "project.add_source",
        json!({ "project_id": project_id, "path": other }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let said = state
        .project_agent_prompt(&project.owner)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        said.contains(&format!(
            "Its other sources are part of the base too: `docs` at {}.",
            other.display()
        )),
        "{said}"
    );
}

// ---- an agent from before the move ------------------------------------------

/// A project conversation minted before the agent moved is the same one after
/// a restart: found again by its scratch root, never minted twice, and its
/// agent resumes the exact session it recorded there — a claude transcript
/// filed under the scratch directory's name — from the base it now starts in.
#[tokio::test]
async fn a_conversation_from_before_the_move_is_found_and_resumed_from_the_base() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let config = state_root.join("config.json");
    let store = state_root.join("store");
    let boot = || {
        AppState::new_unrooted_configured(
            state_root.join("worktrees"),
            "main",
            true,
            context(&state_root),
        )
        .with_config(&config)
        .unwrap()
        .with_task_store(&store)
        .unwrap()
    };
    let choice = ModelChoice::default();

    let before = {
        let mut state = boot();
        let project = a_project_agent(&mut state, &repo);
        let root = AppState::canonical_root(&project.scratch);
        let instance = state
            .record_agent_session_start(&project.owner, &project.agent_id, &root, &choice, "start")
            .expect("the session the agent had before the move");
        state.note_self_report(
            &project.owner,
            &project.agent_id,
            &instance,
            SelfReport {
                named: Some("sess-before".to_string()),
                model: None,
                effort: None,
            },
        );
        project
    };

    // Where claude filed that conversation: under the cwd it was started in.
    let home = tempfile::tempdir().unwrap();
    let transcripts =
        home.path()
            .join(".claude/projects")
            .join(crate::harness::claude::encode_project_dir(
                &AppState::canonical_root(&before.scratch),
            ));
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::write(transcripts.join("sess-before.jsonl"), "{}\n").unwrap();

    let mut state = boot();
    let project_id = state.default_project().unwrap();
    let ensured = state.handle(req(
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    ));
    assert_eq!(
        ensured["result"]["run_id"],
        json!(before.owner),
        "{ensured:?}"
    );
    assert_eq!(state.runs.len(), 1, "no second owner");
    assert_eq!(
        state.project_conversation_run(&project_id).as_deref(),
        Some(before.owner.as_str())
    );
    let agents = state.entity_agents(&before.owner).unwrap();
    assert!(
        agents.by_id(&before.agent_id).is_some(),
        "the agent is kept"
    );

    let state = state.shared();
    let spawns = record_spawns(&state);
    let home_dir = home.path().to_path_buf();
    state.lock().unwrap().resume_id_probe = Arc::new(move |dir: &Path, provider, id: &str| {
        crate::harness::harness_for(provider).holds_conversation(&home_dir, dir, id)
    });
    ensure_agent_tab(
        &state,
        &before.scratch,
        &before.owner,
        &before.agent_id,
        &choice,
        "resume",
    )
    .expect("the agent starts again");

    let options = only_spawn(&spawns);
    assert_eq!(options.cwd, repo, "it now starts in the base");
    assert_eq!(
        options.resume_session_id.as_deref(),
        Some("sess-before"),
        "and keeps the conversation it had in scratch"
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.recorded_resume_id(&before.owner, &before.agent_id)
            .as_deref(),
        Some("sess-before"),
        "the recorded name is not forgotten"
    );
}

// ---- attachments -----------------------------------------------------------

/// A file attached to a project's conversation is stored in the scratch root,
/// never in the base, and the agent standing in the base is handed a path that
/// opens from there. One sent before the move — a scratch-relative path — is
/// named from the scratch root in the packet a restarted agent reads.
#[test]
fn a_project_conversations_attachments_open_from_the_base() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    let clean = git_status(&repo);

    // Sent before the move: stored in scratch, named relative to it.
    let home = project.scratch.join(".build/attachments");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("0123456789ab-before.png"), b"before").unwrap();
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": project.owner,
            "agent_id": project.agent_id,
            "body": "the old one",
            "attachments": [{
                "name": "before.png",
                "path": ".build/attachments/0123456789ab-before.png",
            }],
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    // Sent after it: an absolute path, into the same scratch folder.
    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": project.owner,
            "filename": "after.png",
            "content_b64": b64encode(b"after"),
        }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");
    let after = PathBuf::from(attached["result"]["path"].as_str().unwrap());
    assert!(after.is_absolute(), "{}", after.display());
    assert!(after.starts_with(&home), "{}", after.display());
    assert_eq!(std::fs::read(&after).unwrap(), b"after");
    let read_back = state.handle(req(
        "thread.attachment",
        json!({ "entity_id": project.owner, "path": after }),
    ));
    assert_eq!(read_back["ok"], true, "{read_back:?}");

    let packet = state.cold_prompt_with_catch_up(&project.owner, &project.agent_id, "");
    let before = home.join("0123456789ab-before.png");
    assert!(
        packet.contains(&format!("open them: {}]", before.display())),
        "{packet}"
    );
    assert!(!packet.contains("open them: .build/"), "{packet}");
    // Resolved the way the agent resolves it: from where it stands.
    assert!(repo.join(&before).is_file());

    assert!(!repo.join(".build").exists(), "no attachments in the base");
    assert_eq!(git_status(&repo), clean);
}

/// Every other conversation's agent stands where its attachments were
/// written, so their paths stay worktree-relative, in the packet too.
#[test]
fn a_workspace_conversations_attachment_paths_stay_relative() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "relative");
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent_id = state.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": owner,
            "filename": "shot.png",
            "content_b64": b64encode(b"shot"),
        }),
    ));
    let path = attached["result"]["path"].as_str().unwrap().to_string();
    assert!(path.starts_with(".build/attachments/"), "{path}");
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": owner,
            "agent_id": agent_id,
            "body": "look",
            "attachments": [attached["result"]],
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let packet = state.cold_prompt_with_catch_up(&owner, &agent_id, "");
    assert!(packet.contains(&format!("open them: {path}]")), "{packet}");
}

// ---- the base is never a workspace -------------------------------------------

/// Starting the project agent in the base does not make the base a workspace:
/// no list names it and no scan adopts it, and the scratch root is still the
/// only directory the owner is bound to.
#[tokio::test]
async fn the_base_a_project_agent_stands_in_is_never_a_workspace() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    let project_id = state.project_of(&project.owner).unwrap();
    let state = state.shared();
    record_spawns(&state);
    ensure_agent_tab(
        &state,
        &project.scratch,
        &project.owner,
        &project.agent_id,
        &ModelChoice::default(),
        "start",
    )
    .expect("the project agent starts");

    let mut s = state.lock().unwrap();
    let listed = s.handle(req("workspace.list", json!({ "project_id": project_id })));
    let workspaces = listed["result"]["workspaces"].as_array().unwrap().clone();
    for workspace in &workspaces {
        let root = PathBuf::from(workspace["root"].as_str().unwrap_or_default());
        assert_ne!(root, repo, "the base is not a workspace: {workspace:?}");
        assert_ne!(
            workspace["workspace_id"],
            json!(project.owner),
            "{workspace:?}"
        );
    }
    assert!(
        !s.bound_worktree_paths().contains(&repo),
        "nothing binds the base as a run's worktree"
    );
    assert_eq!(s.runs.len(), 1, "the base was not adopted as a run");
    assert_eq!(s.runs[&project.owner].worktree.path, project.scratch);
}

// ---- after a restart ---------------------------------------------------------

/// A daemon over the config and store under `state_root`: booting it twice is
/// a bridge restart.
fn configured(state_root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        state_root.join("worktrees"),
        "main",
        true,
        context(state_root),
    )
    .with_config(state_root.join("config.json"))
    .unwrap()
    .with_task_store(state_root.join("store"))
    .unwrap()
}

/// Every turn the queue holds, drained the way the delivery loop drains them
/// and composed the way it hands them over.
fn drained(state: &mut AppState) -> Vec<PendingAgentTurn> {
    let mut turns = state.take_pending_turns();
    let mut taken = Vec::new();
    while let Some((turn, mark)) = turns.next_turn() {
        mark.settle(state);
        taken.push(turn);
    }
    taken
}

fn the_turn_of(turns: &[PendingAgentTurn], operation_id: &str) -> TurnText {
    turns
        .iter()
        .find(|turn| turn.operation_id.as_deref() == Some(operation_id))
        .unwrap_or_else(|| panic!("no turn for {operation_id}"))
        .said()
        .clone()
}

fn post_native(state: &mut AppState, project: &ProjectAgent, operation_id: &str, body: &str) {
    post_native_with(state, project, operation_id, body, json!([]));
}

fn post_native_with(
    state: &mut AppState,
    project: &ProjectAgent,
    operation_id: &str,
    body: &str,
    attachments: Value,
) {
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": project.owner,
            "agent_id": project.agent_id,
            "operation_id": operation_id,
            "body": body,
            "attachments": attachments,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An agent idle when the bridge restarts is not in the restart roster, so
/// the first thing that starts it again is the user's next message — a native
/// operation. That turn opens the process in the base, and it is the one
/// chance to tell the agent where it stands: its prompt rides the cold half,
/// after the user's words and the operation's exact messages, which it leaves
/// as they were. A running agent already knows, so the warm half is the
/// operation alone.
#[test]
fn an_idle_project_agent_hears_its_prompt_on_a_native_post_after_a_restart() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let project = a_project_agent(&mut configured(&state_root), &repo);

    let mut state = configured(&state_root);
    post_native(
        &mut state,
        &project,
        "op-after-restart",
        "what changed overnight?",
    );
    let said = the_turn_of(&drained(&mut state), "op-after-restart");

    let cold = one_line(&said.cold);
    assert!(
        cold.starts_with("what changed overnight?"),
        "the user's words come first: {cold}"
    );
    assert!(
        cold.contains("Process only reviewer operation `op-after-restart`"),
        "{cold}"
    );
    assert!(
        cold.contains("\"body\": \"what changed overnight?\""),
        "{cold}"
    );
    for rule in [
        format!("You stand in the project's base, {}:", repo.display()),
        "Never change it: do not edit, check out, build or commit there".to_string(),
        "Every change goes through a workspace".to_string(),
    ] {
        assert!(cold.contains(&rule), "{rule:?} missing from {cold}");
    }
    assert!(
        cold.find("\"body\": \"what changed overnight?\"")
            < cold.find("You stand in the project's base"),
        "the prompt follows the operation: {cold}"
    );
    assert!(
        !said.warm.contains("You stand in the project's base"),
        "{}",
        said.warm
    );

    // A command the provider owns still goes exactly as it was typed.
    post_native(&mut state, &project, "op-compact", "/compact");
    let said = the_turn_of(&drained(&mut state), "op-compact");
    assert_eq!(said.cold, "/compact");
}

/// Only the project agent is told this: a coding agent's native turn goes as
/// it was queued.
#[test]
fn a_workspace_agents_native_post_is_not_given_the_project_prompt() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let workspace_id = workspace(&mut state, &project_id, "coding");
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agent_id = state.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": owner,
            "agent_id": agent_id,
            "operation_id": "op-coding",
            "body": "fix the parser",
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let said = the_turn_of(&drained(&mut state), "op-coding");
    assert!(
        !said.cold.contains("You stand in the project's base"),
        "{}",
        said.cold
    );
    assert!(
        !said.cold.contains("agent for the project"),
        "{}",
        said.cold
    );
}

// ---- what a harness leaves in the base ---------------------------------------

const HARNESS_FILES: [&str; 3] = [
    ".claude/scheduled_tasks.lock",
    ".claude/scheduled_tasks.json",
    ".claude/settings.local.json",
];

fn untracked(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn write_harness_files(dir: &Path) {
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    for file in HARNESS_FILES {
        std::fs::write(dir.join(file), "{}").unwrap();
    }
}

fn exclude_of(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default()
}

/// Claude writes its scheduler lock, its durable tasks and the permissions it
/// remembers into the directory it stands in. Started in the user's checkout,
/// those would show in their `git status`; the rules that keep them out go in
/// the repository's own `.git/info/exclude`, never a tracked file, and only
/// for those files — anything else under `.claude/` is still the user's.
#[tokio::test]
async fn what_the_harness_writes_in_the_base_stays_out_of_its_status() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    let state = state.shared();
    record_spawns(&state);
    ensure_agent_tab(
        &state,
        &project.scratch,
        &project.owner,
        &project.agent_id,
        &ModelChoice::default(),
        "start",
    )
    .expect("the project agent starts");

    write_harness_files(&repo);
    assert_eq!(
        untracked(&repo),
        "",
        "the harness's files are not the user's"
    );
    std::fs::write(repo.join(".claude/settings.json"), "{}").unwrap();
    assert_eq!(untracked(&repo), "?? .claude/settings.json\n");
    let tracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", ".gitignore"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(tracked.stdout.is_empty(), "no .gitignore was written");
    assert!(!repo.join(".gitignore").exists());
}

/// Each rule is written once however many times the agent starts, below the
/// user's own rules and never over them; it is anchored to the directory the
/// agent stands in, so a base inside a larger repository excludes its own
/// files and nobody else's; and a base that is not a repository is left
/// alone.
#[test]
fn the_harness_rules_are_written_once_where_the_agent_stands_and_only_in_git() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    std::fs::write(repo.join(".git/info/exclude"), "notes/\n").unwrap();
    for _ in 0..3 {
        crate::orchestrator::exclude_harness_files(&repo).unwrap();
    }
    let exclude = exclude_of(&repo);
    assert!(exclude.starts_with("notes/\n"), "{exclude}");
    for file in HARNESS_FILES {
        let rule = format!("/{file}");
        assert_eq!(
            exclude.lines().filter(|line| *line == rule).count(),
            1,
            "{rule} once: {exclude}"
        );
    }
    assert_eq!(
        exclude.lines().filter(|line| line.starts_with('#')).count(),
        1,
        "{exclude}"
    );

    let nested = repo.join("packages/app");
    std::fs::create_dir_all(&nested).unwrap();
    crate::orchestrator::exclude_harness_files(&nested).unwrap();
    write_harness_files(&nested);
    write_harness_files(&repo.join("packages/other"));
    assert!(exclude_of(&repo).contains("/packages/app/.claude/scheduled_tasks.lock\n"));
    let status = untracked(&repo);
    assert!(!status.contains("packages/app/"), "{status}");
    assert!(
        status.contains("?? packages/other/.claude/settings.local.json"),
        "{status}"
    );

    let plain = tempfile::tempdir().unwrap();
    crate::orchestrator::exclude_harness_files(plain.path()).unwrap();
    assert!(!plain.path().join(".git").exists(), "not made a repository");
}

// ---- attachments sent before the move, delivered after it -------------------

/// A file stored in the scratch root before the move, and the path the
/// conversation recorded for it then: relative to scratch.
fn an_old_attachment(project: &ProjectAgent) -> (PathBuf, Value) {
    let home = project.scratch.join(".build/attachments");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("0123456789ab-before.png"), b"before").unwrap();
    (
        home.join("0123456789ab-before.png"),
        json!([{
            "name": "before.png",
            "path": ".build/attachments/0123456789ab-before.png",
        }]),
    )
}

/// A native operation hands a project agent every attachment path from the
/// scratch root: one queued before the restart and replayed from the store,
/// one sent after it naming an earlier upload, and the ones the operation's
/// context mentions. The record keeps what was sent.
#[test]
fn a_native_post_names_a_project_agents_old_attachments_from_scratch() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let project = {
        let mut state = configured(&state_root);
        let project = a_project_agent(&mut state, &repo);
        let (_, attachments) = an_old_attachment(&project);
        let posted = state.handle(req(
            "thread.post",
            json!({
                "entity_id": project.owner,
                "agent_id": project.agent_id,
                "body": "said before the move",
                "attachments": attachments,
            }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        drained(&mut state);
        post_native_with(
            &mut state,
            &project,
            "op-queued",
            "still queued",
            attachments,
        );
        project
    };
    let (stored, attachments) = an_old_attachment(&project);
    let absolute = stored.display().to_string();

    let mut state = configured(&state_root);
    post_native_with(
        &mut state,
        &project,
        "op-resent",
        "this one again",
        attachments,
    );
    let turns = drained(&mut state);
    for operation_id in ["op-queued", "op-resent"] {
        let said = the_turn_of(&turns, operation_id);
        for (half, text) in [("cold", &said.cold), ("warm", &said.warm)] {
            assert!(
                text.contains(&format!("\"path\": \"{absolute}\"")),
                "{operation_id} {half}: {text}"
            );
            assert!(
                !text.contains("\"path\": \".build/attachments/"),
                "{operation_id} {half}: {text}"
            );
        }
        assert!(
            said.cold.contains(&format!("open them: {absolute}]")),
            "the context names it from scratch too: {}",
            said.cold
        );
        assert!(!said.cold.contains("open them: .build/"), "{}", said.cold);
    }
    let recorded = state
        .agent_conversation(&project.owner, Some(&project.agent_id))
        .unwrap()
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message) => message.attachments.first(),
            _ => None,
        })
        .map(|attachment| attachment.path.clone())
        .collect::<Vec<_>>();
    assert!(
        recorded
            .iter()
            .all(|path| path == ".build/attachments/0123456789ab-before.png"),
        "{recorded:?}"
    );
}

/// A legacy post is delivered as a payload frozen from the unread messages
/// when the turn goes, and a project agent reads its attachment paths from
/// the scratch root just the same.
#[test]
fn a_legacy_post_names_a_project_agents_old_attachments_from_scratch() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    let (stored, attachments) = an_old_attachment(&project);
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": project.owner,
            "agent_id": project.agent_id,
            "body": "the old one",
            "attachments": attachments,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let payload = state
        .legacy_delivery_payload(&project.owner, &project.agent_id)
        .unwrap()
        .expect("the message waits for delivery");
    let prompt = payload.legacy_delivery_prompt(false, crate::models::AgentProvider::Claude);
    assert!(
        prompt.contains(&format!("\"path\": \"{}\"", stored.display())),
        "{prompt}"
    );
    assert!(
        !prompt.contains("\"path\": \".build/attachments/"),
        "{prompt}"
    );
}

// ---- two copies of one conversation --------------------------------------------

/// Claude has a transcript of the agent's conversation under the scratch
/// directory's name, where Build filed it, and a shorter one under the base's.
/// Resumed from the base, claude would read the shorter one; it is set aside,
/// kept, and the whole conversation is the one resumed.
#[tokio::test]
async fn a_stale_copy_under_the_base_is_set_aside_for_the_conversation_build_filed() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project = a_project_agent(&mut state, &repo);
    let choice = ModelChoice::default();
    let root = AppState::canonical_root(&project.scratch);
    let instance = state
        .record_agent_session_start(&project.owner, &project.agent_id, &root, &choice, "start")
        .expect("the session the agent had");
    state.note_self_report(
        &project.owner,
        &project.agent_id,
        &instance,
        SelfReport {
            named: Some("sess-both".to_string()),
            model: None,
            effort: None,
        },
    );

    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join(".claude/projects");
    let filed = projects
        .join(crate::harness::claude::encode_project_dir(&root))
        .join("sess-both.jsonl");
    let shadow = projects
        .join(crate::harness::claude::encode_project_dir(&repo))
        .join("sess-both.jsonl");
    for (at, lines) in [(&filed, "{}\n{}\n{}\n"), (&shadow, "{}\n")] {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, lines).unwrap();
    }

    let state = state.shared();
    let spawns = record_spawns(&state);
    {
        let mut s = state.lock().unwrap();
        let home_dir = home.path().to_path_buf();
        s.resume_id_probe = Arc::new(move |dir: &Path, provider, id: &str| {
            crate::harness::harness_for(provider).holds_conversation(&home_dir, dir, id)
        });
        let home_dir = home.path().to_path_buf();
        s.shadow_probe = Arc::new(move |cwd: &Path, root: &Path, _, id: &str| {
            crate::harness::harness_for(crate::models::AgentProvider::Claude)
                .set_aside_shadowing_copy(&home_dir, cwd, root, id)
        });
    }
    ensure_agent_tab(
        &state,
        &project.scratch,
        &project.owner,
        &project.agent_id,
        &choice,
        "resume",
    )
    .expect("the agent starts again");

    let options = only_spawn(&spawns);
    assert_eq!(options.cwd, repo);
    assert_eq!(options.resume_session_id.as_deref(), Some("sess-both"));
    assert!(!shadow.exists(), "claude no longer finds the short copy");
    let aside: Vec<PathBuf> = std::fs::read_dir(shadow.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "{}\n", "kept");
    assert_eq!(std::fs::read_to_string(&filed).unwrap(), "{}\n{}\n{}\n");
}
