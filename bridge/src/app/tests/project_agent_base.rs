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
