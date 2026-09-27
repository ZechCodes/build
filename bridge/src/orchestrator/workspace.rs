use portable_pty::PtySize;

use crate::agent::AgentRoster;
use crate::git_process::run_git;
use crate::harness::HarnessError;
use crate::isolation::Isolation;
use crate::models::{AgentProvider, ModelChoice};
use crate::plan::{plan_transition, PlanEvent, StageDoc};
use crate::pty::HarnessSpec;
use crate::run::{Run, RunEvent, RunId};
use crate::store::Store;
use crate::templates::{Templates, DEFAULT_PLAN_PATH};
use crate::worktree::{derive_adoption_goal, ExternalWorktree, Worktree, WorktreeManager};
use std::path::{Path, PathBuf};

/// A checkout that passed every refusal adoption makes. Construction IS the
/// validation, so nothing downstream can refuse a checkout it has already
/// written a checkpoint commit into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptableCheckout {
    /// Git's internal worktree name, so teardown understands the checkout.
    pub name: String,
    /// Canonical absolute path of the working directory.
    pub path: PathBuf,
    /// The branch it has checked out. Never detached, never the base branch of
    /// an external worktree, never option-shaped.
    pub branch: String,
    /// HEAD commit subject. UNTRUSTED display text, and what names the run when
    /// the branch name says nothing.
    pub head_subject: String,
}

impl AdoptableCheckout {
    /// The three refusals, all of them pure. A checkout that fails one is
    /// refused with nothing on disk touched and nothing persisted.
    pub fn judge(
        checkout: &ExternalWorktree,
        base_branch: &str,
    ) -> Result<AdoptableCheckout, OrchestratorError> {
        let Some(branch) = checkout.branch.clone() else {
            return Err(OrchestratorError::Gate(
                "cannot adopt a detached-HEAD worktree — check out a branch first".to_string(),
            ));
        };
        // A worktree sitting on the base branch is a mistake to adopt: the
        // base branch is what workspaces are cut from, not a place to work.
        if branch == base_branch {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree with the base branch {base_branch:?} checked out"
            )));
        }
        // The branch name is an EXTERNAL, untrusted string handed to `git merge`
        // / `git push` as a bare argv element later; a leading `-` would be read
        // as an option (arbitrary code execution). Native branches are always
        // `build/<slug>` and can never trip this.
        if branch.starts_with('-') {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree whose branch name {branch:?} looks like a command-line \
                 option — rename the branch first"
            )));
        }
        Ok(AdoptableCheckout {
            name: checkout.name.clone(),
            path: checkout.path.clone(),
            branch,
            head_subject: checkout.head_subject.clone(),
        })
    }

    /// The checkout as Build records it. One shape, read by the scaffold and by
    /// the run alike, so the two can never disagree about what was adopted.
    pub fn worktree(&self, base_branch: &str) -> Worktree {
        Worktree {
            name: self.name.clone(),
            path: self.path.clone(),
            recorded_branch: self.branch.clone(),
            base_branch: base_branch.to_string(),
        }
    }
}

/// Per-spawn context an interactive harness builder may honor.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Resume the harness's own most-recent conversation for this cwd (claude:
    /// `--continue`, codex: `resume --last`) — a GUESS, since what it reopens
    /// is the newest conversation in the checkout whoever was having it. Set
    /// for a respawn of an agent with recorded history, which is that agent
    /// continuing its own conversation, and never for a brand-new agent.
    pub continue_session: bool,
    /// Resume the conversation the agent's last session NAMED (claude:
    /// `--resume <id>`, codex: `resume <SESSION_ID>`), when one was recorded
    /// and the provider still holds it.
    ///
    /// An alternative to `continue_session`, never a companion: this names the
    /// exact conversation Build was speaking to, and `--continue` guesses the
    /// newest one in the cwd. Every carrier can carry a name: a protocol
    /// announces it, while a terminal either knows it at launch or locates it
    /// from durable harness records.
    pub resume_session_id: Option<String>,
    /// Entity whose per-session MCP server receives the terminal `done` report.
    pub owner_id: String,
    /// Unlogged capability for this exact harness process. The daemon rotates it
    /// whenever the worktree's agent tab is replaced, preventing another local
    /// process from forging reports with only a known entity id.
    pub mcp_session_token: String,
    /// The worktree the harness will run in. Providers gate an interactive
    /// session behind a workspace-trust dialog for a directory they have not
    /// seen before, and Build mints a fresh worktree per run — so the adapter
    /// needs the path to pre-trust it, or the dialog eats the injected prompt.
    pub cwd: PathBuf,
    /// The directory holding this agent's `.build/` scaffold, when it is not
    /// `cwd`. Only a project agent's is elsewhere: it stands in its project's
    /// base, which is the user's and gets nothing of Build's written into it,
    /// while its scaffold stays in the root Build keeps for it.
    pub scaffold: Option<PathBuf>,
}

impl SpawnOptions {
    /// The `--mcp-config` a harness is handed: worktree-relative where the
    /// scaffold is the cwd, which is every agent but a project agent, and
    /// absolute where it is not, since a relative one would be read from the
    /// wrong directory and the harness would die before its first turn.
    pub fn mcp_config(&self) -> String {
        let relative = mcp_config_path(&self.owner_id);
        match &self.scaffold {
            Some(dir) => dir.join(relative).display().to_string(),
            None => relative,
        }
    }
}

/// Where one agent launch writes its scaffold and where its child stands.
/// The same directory for every agent but a project agent.
#[derive(Clone, Copy)]
pub(crate) struct LaunchDirs<'a> {
    pub(crate) scaffold: &'a Path,
    pub(crate) cwd: &'a Path,
}

impl<'a> LaunchDirs<'a> {
    /// One directory for both.
    #[cfg(test)]
    pub(crate) fn at(dir: &'a Path) -> Self {
        LaunchDirs {
            scaffold: dir,
            cwd: dir,
        }
    }
}

/// Builds an interactive harness command for a rendered prompt + model + context.
///
/// The prompt is supplied so test and custom adapters can inspect the turn being
/// dispatched, but it is always submitted through the tab's PTY, never baked
/// into argv.
pub type WarmBuilder = std::sync::Arc<
    dyn Fn(&str, &ModelChoice, &SpawnOptions) -> Result<HarnessSpec, HarnessError> + Send + Sync,
>;

/// Whether the harness has an existing conversation transcript for a worktree
/// cwd — the one question `--continue` turns on. Its whole job is picking a
/// conversation back up that Build did not start in this process: a tab
/// respawned after a daemon restart or a crash, or an agent the user ran in
/// the worktree by hand before Build ever looked at it. Injectable so tests
/// never touch the real home directory.
pub type TranscriptProbe = std::sync::Arc<dyn Fn(&Path, AgentProvider) -> bool + Send + Sync>;

/// Builds the watcher that will name the conversation a session about to open
/// in a worktree cwd is having. Called at the spawn reservation, BEFORE the
/// child exists, so what the harness already wrote there can be told from what
/// the child writes. `None` for a provider whose session names its own
/// conversation. Injectable for the same reason the probe is: tests never read
/// the developer's real transcript tree.
pub type SessionLocatorFactory = std::sync::Arc<
    dyn Fn(&Path, AgentProvider) -> Option<Box<dyn crate::harness::SessionLocator>> + Send + Sync,
>;

/// Whether the conversation a recorded id names is still in the provider's
/// tree — asked before the id is spent, so a dead one costs zero restarts
/// rather than one. The sibling of [`TranscriptProbe`], injectable for the same
/// reason.
pub type ResumeIdProbe = std::sync::Arc<dyn Fn(&Path, AgentProvider, &str) -> bool + Send + Sync>;

/// Clear a recorded id's way from the directory a child stands in to the one
/// Build filed it under — [`crate::harness::Harness::set_aside_shadowing_copy`],
/// with the provider's home read by the caller. Asked as `(cwd, root, provider,
/// id)`, and only where the two differ.
pub type ShadowProbe = std::sync::Arc<
    dyn Fn(&Path, &Path, AgentProvider, &str) -> std::io::Result<Option<PathBuf>> + Send + Sync,
>;

/// The one sentence Build says when a reviewer has written: every carrier,
/// every phase, every path. The plan-side revise turns used to carry a
/// shorter cousin of this line, so one agent heard two wordings of the
/// same instruction depending on which door the message came through.
pub const NEW_THREAD_MESSAGES_PROMPT: &str =
    "Act on every reviewer message delivered in this prompt. The user can only see messages sent with `post_thread_message`; use its status to say whether you are Working, Waiting, Blocked, or Complete.";

/// How long a failed prompt write waits for the harness's exit status to
/// become reapable before the failure is treated as fatal. Long enough to
/// cover the kernel's close-fds-then-reap lag for a harness that exited
/// under the write; short enough that a genuinely wedged PTY still surfaces
/// its write error promptly.
pub(crate) const PROMPT_WRITE_EXIT_GRACE: std::time::Duration =
    std::time::Duration::from_millis(250);

/// How long a fresh spawn waits for the harness's first output before writing
/// the prompt into its PTY. Real harnesses are interactive TUIs: injecting the
/// prompt before the TUI has started servicing the PTY risks it landing on a
/// startup screen. First output is the readiness signal; when the grace
/// expires the prompt is written anyway — a spawn that silently never delivers
/// its prompt is worse than one that races the startup screen.
/// Upper bound on waiting for a harness to become ready. Must comfortably
/// exceed a real TUI's full startup or the wait expires and the prompt is
/// written into a still-painting screen, which is the failure it exists to
/// prevent. claude 2.1.219 settled at ~1.8s; 2.1.223 (statusline hooks, MCP
/// config load) does not enable bracketed paste until ~4s on an idle machine,
/// so the old 6s bound left no margin at all under load — and an expired wait
/// writes into the startup screen, where the alternate-screen clear eats it.
pub(crate) const HARNESS_READY_GRACE: std::time::Duration = std::time::Duration::from_millis(20000);

/// How long a checkout's removal waits for the agents that were writing into it
/// to die.
///
/// `remove_dir_all` walking a directory a child is still creating files in
/// fails the walk, so kill, reap, THEN remove is the order that makes the
/// removal reliable. A SIGKILLed harness reaps in milliseconds; this is the
/// bound on one wedged in uninterruptible I/O, after which the removal is
/// attempted anyway — best-effort, as it has always been.
pub(crate) const CHECKOUT_REAP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How many messages a resumed agent's catch-up packet carries.
///
/// The limit counts messages, never items (§6.1), so a session that emitted
/// hundreds of tool calls still hands its replacement what the human said. The
/// 12 KB byte bound below is the real cap on how much that is.
pub const CATCH_UP_MESSAGES: usize = 40;

/// The cold prompt: the rendered instruction and the conversation protocol
/// every new agent process needs before it touches anything.
///
/// What it deliberately does NOT carry is the durable conversation — the
/// catch-up packet and the previous completion report. Those are composed onto
/// this at delivery ([`append_durable_conversation`]), because only the daemon
/// draining the queue can read the conversation's history out of the store,
/// and because a packet baked when the turn was queued misses whatever was
/// said while it waited for the lock.
pub(crate) fn conversation_prompt(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len() + 2048);
    out.push_str(prompt);
    // This block is the canonical reply policy. The `post_thread_message` tool
    // description in mcp.rs and NEW_THREAD_MESSAGES_PROMPT above defer to
    // it by reference — never restate these bullets elsewhere, restated copies
    // drift. The ambiguity rule stays above the silent-directive allowance so
    // an in-order reader hits the carve-out before committing to silence.
    out.push_str(
        "\n\nBuild conversation protocol:\n\
         - First, call `set_topic` with the objective of this conversation in 2-4 words (e.g. \"Unify prompt delivery\"). The conversation header shows it and says \"Starting\" until you do. Call it again if the objective changes.\n\
         - Act on the current instruction and exact accepted messages in the native payload. Use conversation context and catch-up packets only as background.\n\
         - The user sees only messages sent through `post_thread_message`. Terminal output and ordinary assistant responses are not visible in Build.\n\
         - For long-running work with several meaningful steps, use the native task, checklist, or plan tool available in this session so the user can track progress in Build. Do not create one for brief, one-step work, and do not invent a tool that is absent from your tool list. Keep it accurate: mark work in progress when it begins, update it at each meaningful advance, and mark every item complete only after it is complete; mark blocked work as blocked when the tool supports that state.\n\
         - Every `post_thread_message` needs a status: `Working` for a progress update while continuing, `Waiting` when the next step needs a user response, `Blocked` when work cannot proceed, and `Complete` when the objective is finished. `Complete` and `Blocked` also end the turn: they are the completion report, so the body carries the whole of it.\n\
         - If a reviewer message reads as either a question or an ambiguous directive, post a one-line clarifying reply via `post_thread_message` instead of silently changing code.\n\
         - You may implement an unambiguous directive without replying; the next revision is its acknowledgment.\n\
         - Call `post_thread_message` for progress the user needs, questions or clarification, and always once with `Complete` or `Blocked` to report the final outcome.\n\
         - Do not post acknowledgments or diff recaps.\n\
         - When the reply you need is a choice you can enumerate, send `options` with the message: each is a chip the reviewer presses, and what comes back is an ordinary reviewer message. Write each option's `message` as the full instruction it stands for, not a repeat of its label — that text is what a later session sees. Anything said afterwards closes the offer.\n\
         - A message may carry files (`attachments`, each with a `path`). Open every one before acting on that message: the reviewer attached it because the words alone do not carry what they mean.\n",
    );
    out
}

/// Close a cold prompt with the durable conversation: the catch-up packet the
/// caller assembled. The last session's report is its outcome message, which
/// is conversation and so already inside the packet.
///
/// Kept newest-first inside the byte bound — a packet clipped from the front
/// loses the oldest lines rather than the ones that just happened.
pub(crate) fn append_durable_conversation(mut out: String, catch_up: &str) -> String {
    if !catch_up.is_empty() {
        out.push_str("\nCatch-up packet from the durable conversation (oldest to newest):\n");
        if catch_up.len() <= 12_000 {
            out.push_str(catch_up);
        } else {
            let mut boundary = catch_up.len() - 12_000;
            while !catch_up.is_char_boundary(boundary) {
                boundary += 1;
            }
            out.push_str(&catch_up[boundary..]);
        }
        out.push('\n');
    }
    // The previous session's report is its outcome message, and that is
    // conversation: the packet above already carries it.
    out
}

pub(super) fn append_stage_catalog(
    mut prompt: String,
    stages: &[StageDoc],
    status_for: impl Fn(&str) -> String,
) -> String {
    prompt.push_str("\n\nOrdered Issue stage-plan catalog (authoritative order):\n");
    if stages.is_empty() {
        prompt.push_str("- No stage plans exist yet.\n");
    } else {
        for stage in stages {
            prompt.push_str(&format!(
                "- {} — {} — {} — approval: {:?}; predecessor/execution status: {}\n",
                stage.id,
                stage.title,
                stage.path,
                stage.state,
                status_for(&stage.id)
            ));
        }
    }
    prompt
}

/// The scratch docs dir a plan's prompts point its agent at. Empty when the
/// plan has no workspace — no session is being rendered for it either.
pub(super) fn plan_docs_dir_display(active: &ActivePlan) -> String {
    active
        .workspace
        .as_ref()
        .map(|workspace| workspace.docs_dir.display().to_string())
        .unwrap_or_default()
}

/// What a stage revision needs to be legal, and the doc it is against.
/// Pure, and checked before any disk work: nothing is scaffolded for a
/// revise that will be refused.
pub fn gate_plan_stage_notes(
    active: &ActivePlan,
    stage_id: &str,
) -> Result<usize, OrchestratorError> {
    let index = active
        .stage_doc_index(stage_id)
        .map_err(OrchestratorError::Gate)?;
    plan_transition(&active.plan.state, PlanEvent::SendNotes)
        .map_err(|e| OrchestratorError::Gate(format!("cannot send stage notes: {e}")))?;
    if active.open_comments_for(stage_id).is_empty() {
        return Err(OrchestratorError::Gate(format!(
            "no open comments on stage {stage_id}"
        )));
    }
    Ok(index)
}

/// What a freeform message costs the plan machine: nothing while it is
/// drafting, a `Reply` out of a parked state. Pure, and the refusals are
/// made here — before any disk work, and before the caller has anything to
/// persist but the message itself.
pub fn gate_plan_message(
    active: &ActivePlan,
    message: &str,
) -> Result<Option<PlanEvent>, OrchestratorError> {
    if message.trim().is_empty() {
        return Err(OrchestratorError::Gate("message must not be empty".into()));
    }
    use crate::plan::PlanState as S;
    let event = match active.plan.state {
        S::Drafting => None,
        S::Blocked | S::Failed | S::IdleUnreported | S::Interrupted => Some(PlanEvent::Reply),
        S::PlanReview => {
            return Err(OrchestratorError::Gate(
                "the plan is at the review gate — use send notes there".into(),
            ))
        }
        S::Created | S::Approved | S::Abandoned => {
            return Err(OrchestratorError::Gate(
                "no plan agent session to message".into(),
            ))
        }
    };
    if let Some(event) = event {
        plan_transition(&active.plan.state, event)?;
    }
    Ok(event)
}

/// Whether a directory holds at least one file, at any depth. A scratch docs
/// dir that holds nothing is one the canonical docs must be restored into.
pub(super) fn dir_holds_a_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| match entry.file_type() {
        Ok(kind) if kind.is_dir() => dir_holds_a_file(&entry.path()),
        Ok(kind) => kind.is_file(),
        Err(_) => false,
    })
}

/// The worktree-relative name of one owner's MCP config. Every agent gets its
/// own, because the file names the owner the harness reports `done` for.
pub fn mcp_config_path(owner_id: &str) -> String {
    format!(".build/{}", mcp_config_name(owner_id))
}

fn mcp_config_name(owner_id: &str) -> String {
    // Agent ids are Crockford base32 with a fixed prefix, so this is always a
    // plain file name; anything else (a legacy entity id) is sanitized to one.
    let safe: String = owner_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("mcp-{safe}.json")
}

/// How the orchestrator launches an agent for a phase.
#[derive(Clone)]
pub enum Agent {
    /// A fixed warm interactive session: spawn the binary, then write the prompt
    /// to its PTY.
    Warm(HarnessSpec),
    /// A provider/model-aware warm interactive session. The builder supplies
    /// argv and environment; Build still injects the prompt through the PTY.
    WarmBuilder(WarmBuilder),
}

/// Owned inputs for constructing one agent process. Cloning this under the app
/// lock lets every filesystem and provider setup step run after that lock is
/// released.
#[derive(Clone)]
pub(crate) struct AgentLaunch {
    pub(super) repo_path: PathBuf,
    pub(super) bridge_exe: PathBuf,
    pub(super) agent: Agent,
    pub(super) pty_size: PtySize,
}

#[derive(Debug)]
pub(crate) struct PreparedAgentLaunch {
    pub(crate) spec: HarnessSpec,
    pub(crate) pty_size: PtySize,
}

impl AgentLaunch {
    /// Make the worktree launch-ready and return everything the app needs to
    /// spawn it. The ordered scaffold/spec boundary stays inside this operation.
    pub(crate) fn prepare(
        &self,
        owner_id: &str,
        dirs: LaunchDirs<'_>,
        model_choice: &ModelChoice,
        continue_session: bool,
        resume_session_id: Option<String>,
        mcp_session_token: &str,
    ) -> Result<PreparedAgentLaunch, OrchestratorError> {
        self.scaffold_agent_worktree(dirs.scaffold, owner_id)?;
        if dirs.scaffold != dirs.cwd {
            if let Err(error) = exclude_harness_files(dirs.cwd) {
                eprintln!("keep harness files out of {}: {error}", dirs.cwd.display());
            }
        }
        let options = SpawnOptions {
            continue_session,
            resume_session_id,
            owner_id: owner_id.to_string(),
            mcp_session_token: mcp_session_token.to_string(),
            cwd: dirs.cwd.to_path_buf(),
            scaffold: (dirs.scaffold != dirs.cwd).then(|| dirs.scaffold.to_path_buf()),
        };
        let spec = match &self.agent {
            Agent::Warm(spec) => Ok(spec.clone()),
            Agent::WarmBuilder(build) => build("", model_choice, &options),
        }?;
        Ok(PreparedAgentLaunch {
            spec,
            pty_size: self.pty_size,
        })
    }

    pub(super) fn scaffold_agent_worktree(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.write_build_dir(worktree_path, owner_id)
    }

    fn is_primary_checkout(&self, path: &Path) -> bool {
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical(path) == canonical(&self.repo_path)
    }

    fn exclude_build_machinery_repo_locally(&self) -> Result<(), OrchestratorError> {
        let git_dir = run_git(&self.repo_path, &["rev-parse", "--git-common-dir"])?
            .trim()
            .to_string();
        append_exclude_rules(
            &self.repo_path.join(git_dir),
            "# Build's machine-local agent plumbing",
            &[
                ".build/mcp*.json".to_string(),
                ".build/attachments/".to_string(),
            ],
        )
    }

    pub(super) fn write_build_dir(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        if !worktree_path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no checkout at {}", worktree_path.display()),
            )
            .into());
        }
        let build_dir = worktree_path.join(".build");
        std::fs::create_dir_all(&build_dir)?;
        if self.is_primary_checkout(worktree_path) {
            self.exclude_build_machinery_repo_locally()?;
        } else {
            std::fs::write(build_dir.join(".gitignore"), "mcp*.json\nattachments/\n")?;
        }
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": self.bridge_exe.to_string_lossy(),
                    "args": ["mcp", "--task", owner_id]
                }
            }
        });
        std::fs::write(
            build_dir.join(mcp_config_name(owner_id)),
            serde_json::to_string_pretty(&mcp)?,
        )?;
        Ok(())
    }
}

/// What a harness writes into the directory it is started in, whatever it
/// was asked to do there: Claude Code's scheduler lock and durable task list,
/// and the permissions it remembers for that directory.
const HARNESS_FILES_WHERE_IT_STANDS: [&str; 3] = [
    ".claude/scheduled_tasks.lock",
    ".claude/scheduled_tasks.json",
    ".claude/settings.local.json",
];

/// Keep what a harness writes where it stands out of `git status` in a
/// directory Build starts an agent in but does not own — a project's base.
///
/// The rules go in the repository's own `.git/info/exclude`, which is this
/// machine's and never committed, rather than a `.gitignore` that would be a
/// change to the user's code. They are anchored to the directory the agent
/// stands in, so a `.claude/` anywhere else in the repository is untouched,
/// and each is appended once. A directory that is not in a git repository has
/// no status to keep clean, and is left alone.
pub(crate) fn exclude_harness_files(cwd: &Path) -> Result<(), OrchestratorError> {
    let Ok(located) = run_git(cwd, &["rev-parse", "--git-common-dir", "--show-prefix"]) else {
        return Ok(());
    };
    let mut located = located.lines();
    let git_dir = cwd.join(located.next().unwrap_or_default().trim());
    let prefix = glob_escaped(located.next().unwrap_or_default().trim());
    let rules: Vec<String> = HARNESS_FILES_WHERE_IT_STANDS
        .iter()
        .map(|file| format!("/{prefix}{file}"))
        .collect();
    append_exclude_rules(
        &git_dir,
        "# Build: what an agent's harness writes in the directory it stands in",
        &rules,
    )
}

/// A path as a gitignore pattern that matches only itself.
fn glob_escaped(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Append to `git_dir`'s `info/exclude` whichever of `rules` it does not
/// already hold, under `header`, written whole so a reader never sees half a
/// file. Nothing is written when every rule is there, and neither the header
/// nor a rule is ever written twice.
fn append_exclude_rules(
    git_dir: &Path,
    header: &str,
    rules: &[String],
) -> Result<(), OrchestratorError> {
    let exclude_path = git_dir.join("info").join("exclude");
    let existing = match std::fs::read_to_string(&exclude_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let held = |line: &str| existing.lines().any(|held| held.trim() == line);
    let missing: Vec<&String> = rules.iter().filter(|rule| !held(rule)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(git_dir.join("info"))?;
    let mut updated = existing.clone();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !held(header) {
        updated.push_str(header);
        updated.push('\n');
    }
    for rule in missing {
        updated.push_str(rule);
        updated.push('\n');
    }
    crate::store::write_file_atomically(&exclude_path, &updated)?;
    Ok(())
}

/// Owns project configuration and drives plans and runs through their
/// lifecycles.
///
/// Cloneable, and cheaply: a verb clones its project's orchestrator under the
/// app mutex and then runs the git with the mutex released.
#[derive(Clone)]
pub struct Orchestrator {
    pub(super) repo_path: PathBuf,
    /// Run (and legacy task) worktrees: `build/<slug>` branches.
    pub(super) worktrees: WorktreeManager,
    /// Where each issue's scratch plan docs are written, one directory per
    /// issue. Outside the repo: planning writes no files into the checkout it
    /// runs in.
    pub(super) plan_docs_root: PathBuf,
    pub(super) launch: AgentLaunch,
    pub(super) templates: Templates,
}

use super::{
    ActivePlan, ActiveRun, AgentTurn, ImplementableIssue, OrchestratorError, PreparedImplementation,
};

impl Orchestrator {
    /// The one seam every checkout operation goes through, for callers that
    /// hold the orchestrator rather than the manager.
    pub fn worktrees(&self) -> &WorktreeManager {
        &self.worktrees
    }
    /// Create a worktree with nothing attached to it — no `.build/` scaffold, no
    /// run record, no session. The human works in it by hand (a terminal or an
    /// agent tab); Build only owns the directory and the branch it cut. It sits
    /// on the same `build/<slug>` naming as run worktrees so teardown, adoption
    /// and the scan all treat it identically.
    pub fn create_bare_worktree(
        &self,
        slug: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self.worktrees.create(slug, base_branch, isolation)?)
    }
    /// The same bare checkout, on a branch that already exists — here or on a
    /// remote. A name no ref anywhere backs is refused, never cut.
    pub fn create_worktree_on_existing_branch(
        &self,
        branch: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self
            .worktrees
            .create_on_existing_branch(branch, base_branch, isolation)?)
    }
    /// The same bare checkout, on a branch the caller named in full and means
    /// to start: an existing branch is checked out rather than cut a second
    /// time over the work it holds, and a name nothing backs is cut from the
    /// base exactly as it was given.
    pub fn create_worktree_cutting_named_branch(
        &self,
        branch: &str,
        base_branch: &str,
        isolation: Isolation,
    ) -> Result<crate::worktree::NamedBranchCheckout, OrchestratorError> {
        Ok(self
            .worktrees
            .create_cutting_branch(branch, base_branch, isolation)?)
    }
    /// One checkout of this repository, described the way the board's scan
    /// describes it. A git walk of that one directory: off the app mutex.
    pub fn describe_checkout(
        &self,
        path: &Path,
        base_branch: &str,
    ) -> Result<ExternalWorktree, OrchestratorError> {
        crate::worktree::describe_checkout(path, base_branch, crate::worktree::unix_now())
            .ok_or_else(|| {
                OrchestratorError::Git(format!(
                    "{} is not a checkout this project can describe",
                    path.display()
                ))
            })
    }
    /// Every checkout of this repository no run owns, as they stand right now.
    /// The whole-repository walk a dispatch or an adoption resolves against,
    /// and seconds of git on a repository with many worktrees: off the app
    /// mutex, always.
    pub fn scan_checkouts(
        &self,
        base_branch: &str,
        excluded: &std::collections::HashSet<PathBuf>,
    ) -> Result<Vec<ExternalWorktree>, OrchestratorError> {
        Ok(self.worktrees.discover(base_branch, excluded)?)
    }
    /// Cut the checkout an Issue's implementation works in and make it ready
    /// to be worked in: `build/<slug>` off the base branch, `.build/`
    /// scaffolded (the MCP config carries the run id), the plan's canonical
    /// docs materialized out of the store and committed ("plan: <goal>" — the
    /// intent record the scope doc keeps through merge).
    ///
    /// That commit is the answer's `base_sha`, the baseline of the review diff,
    /// so the materialized docs never show up as review noise.
    ///
    /// Git and disk from end to end, seconds of it on a large repository: off
    /// the app mutex, always. A failure after the checkout exists removes it,
    /// so a preparation nobody can be handed leaves nothing behind.
    pub fn prepare_run_checkout(
        &self,
        issue: &ImplementableIssue,
        base_branch: &str,
        run_id: &str,
        isolation: Isolation,
        store: &Store,
    ) -> Result<PreparedImplementation, OrchestratorError> {
        let worktree = self
            .worktrees
            .create(issue.slug(), base_branch, isolation)?
            .worktree;
        let prepared = self.scaffold_build_dir(&worktree, run_id).and_then(|()| {
            self.materialize_and_commit_plan_docs(
                &issue.plan_id,
                &worktree.path,
                &issue.goal,
                store,
            )
        });
        match prepared {
            Ok(base_sha) => Ok(PreparedImplementation { worktree, base_sha }),
            Err(error) => {
                self.discard_checkout(&worktree, /* keep_branch */ false);
                Err(error)
            }
        }
    }
    /// The same preparation on a checkout that already exists — the branch's
    /// own uncommitted work is not part of what the implementation does, and it
    /// must not vanish under the baseline either, so it lands as its own commit
    /// below the docs commit.
    ///
    /// Two commits and a store read: off the app mutex, always.
    pub fn prepare_adopted_checkout(
        &self,
        issue: &ImplementableIssue,
        checkout: &Path,
        store: &Store,
    ) -> Result<String, OrchestratorError> {
        self.commit_all_with_message(
            checkout,
            "Checkpoint: before Build implements an Issue here",
        )?;
        self.materialize_and_commit_plan_docs(&issue.plan_id, checkout, &issue.goal, store)
    }
    /// Bind an Issue's implementation to a checkout that already exists,
    /// instead of cutting `build/<slug>` for it. The branch's run adopts the
    /// implementation: whatever the branch was carrying was checkpointed under
    /// its own message, the stage docs were committed on top, and THAT commit is
    /// the review baseline — so the diff the human reviews is exactly what the
    /// implementation adds to the branch.
    ///
    /// The work is handed to a FRESH agent (Decisions §Entity model: issue
    /// implementation stays a handoff), which is why the caller gets the new
    /// agent's id back: the turn is addressed to it, not to whatever agent was
    /// already talking on this branch.
    ///
    /// Pure bookkeeping: `base_sha` is what
    /// [`prepare_adopted_checkout`](Self::prepare_adopted_checkout) committed,
    /// and the refusals were made when the [`ImplementableIssue`] was judged.
    pub fn open_adopted_implementation(
        &self,
        active: &mut ActiveRun,
        plan_link: &ActivePlan,
        base_sha: String,
        model_choice: ModelChoice,
    ) -> Result<(AgentTurn, String), OrchestratorError> {
        let mut run = Run::new(
            active.run.id.clone(),
            Some(plan_link.plan.id.clone()),
            plan_link.plan.goal.clone(),
        );
        run.apply(RunEvent::Dispatch)?;
        active.run = run;
        active.base_sha = Some(base_sha);
        active.plan_path = plan_link.plan_path.clone();
        active.stages = Vec::new();
        active.current_stage_id = None;
        active.revising_stage_id = None;
        active.auto_advance = false;
        active.publication_attempt = None;
        active.model_choice = model_choice.clone();
        active.last_summary = None;
        active.last_error = None;

        let agent_id = active
            .agents
            .add(&active.run.id.0, model_choice, &crate::store::now_rfc3339())
            .id
            .clone();
        let turn = self.open_implementation(active, plan_link);
        Ok((turn, agent_id))
    }
    /// Write Build's ownership into a checkout it is about to adopt: the
    /// checkpoint commit that keeps pre-Build work its own legible commit, then
    /// the `.build/mcp.json` scaffold (left uncommitted). The disk half of an
    /// adoption, and the half that must run with the app mutex released.
    ///
    /// Ordered as the fused dispatch path is, and separated from
    /// [`adopt_run`](Self::adopt_run) so the verdict — which cannot fail once
    /// the checkout is an [`AdoptableCheckout`] — is written down under the
    /// same lock acquisition as everything else it settles.
    pub fn prepare_adoption(
        &self,
        checkout: &AdoptableCheckout,
        base_branch: &str,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.commit_all_with_message(&checkout.path, "Checkpoint: adopted by Build")?;
        self.scaffold_build_dir(&checkout.worktree(base_branch), owner_id)
    }
    /// Mint a plan-less run around a checkout [`prepare_adoption`] has already
    /// written to (the run-side `adopt`; `plan_id` is `None`). No agent session
    /// is spawned — the run lands in `Review` (there is work to review). Pure
    /// bookkeeping: every refusal was spent judging the checkout, and no disk
    /// is touched here.
    ///
    /// [`prepare_adoption`]: Self::prepare_adoption
    pub fn adopt_run(
        &self,
        id: RunId,
        checkout: &AdoptableCheckout,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActiveRun, OrchestratorError> {
        let branch = checkout.branch.clone();
        let worktree = checkout.worktree(base_branch);
        let goal = derive_adoption_goal(&branch, &checkout.head_subject);
        let mut run = Run::new(id, None, goal);
        run.apply(RunEvent::Dispatch)?;
        run.apply(RunEvent::BuildReady)?;

        // Adoption speaks to nobody: it is git and records. The branch starts
        // with no agents, its chat tab shows the new-agent view, and the first
        // thing said to it creates the agent that hears it — on `model_choice`,
        // which is what the adopting caller named.
        Ok(ActiveRun {
            run,
            worktree,
            // Adopted runs baseline their review diff on the merge-base — there
            // is no materialization commit to pin.
            base_sha: None,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: true,
            publication_attempt: None,
            model_choice,
            agents: AgentRoster::empty(),
            last_summary: None,
            last_error: None,
        })
    }
    /// Abandon without touching the checkout — the lifecycle verdict alone.
    /// A run adopted around the PRIMARY checkout ends this way: that directory
    /// is the repository, and [`WorktreeManager::remove`] opens with
    /// `remove_dir_all`.
    pub fn abandon_run_keeping_checkout(
        &self,
        active: &mut ActiveRun,
    ) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::Abandon)?;
        Ok(())
    }
    /// Recreate a missing native implementation checkout from its exact
    /// persisted branch, using the verified local ref first and origin second.
    pub fn restore_run_worktree(
        &self,
        worktree: &Worktree,
        when_unregistered: crate::worktree::UnregisteredRestore,
        isolation: Isolation,
    ) -> Result<Worktree, OrchestratorError> {
        Ok(self
            .worktrees
            .restore(worktree, when_unregistered, isolation)?)
    }
    /// Best-effort teardown of a leftover checkout: the directory always, and
    /// the branch under it unless the caller is handing that back. A failed
    /// cleanup is logged, never fatal — a stray worktree is only clutter, and
    /// what removes a card is the record, not the directory.
    ///
    /// `keep_branch` is the caller's own fact and never derivable here: a run's
    /// work outlives an abandon so it can be re-attempted, and a dispatch that
    /// checked out a branch somebody else made must hand that branch back
    /// whole.
    pub fn discard_checkout(&self, worktree: &Worktree, keep_branch: bool) {
        let removed = if keep_branch {
            self.worktrees.remove_keeping_branch(worktree)
        } else {
            self.worktrees.remove(worktree)
        };
        if let Err(e) = removed {
            eprintln!(
                "discard_checkout {} (keep_branch {keep_branch}): {e}",
                worktree.name
            );
        }
    }
    /// Write the per-entity MCP config under `.build/` so it never trips
    /// plan-scope enforcement, pointing the harness at the owning entity's
    /// `done` server. `owner_id` is a plan, run, or (legacy) task id — the MCP
    /// CLI stays `mcp --task <id>` (opaque); the daemon routes each report by
    /// owner lookup.
    pub(super) fn scaffold_build_dir(
        &self,
        worktree: &Worktree,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.launch
            .scaffold_agent_worktree(&worktree.path, owner_id)
    }
    pub(super) fn commit_all(
        &self,
        worktree_path: &Path,
        goal: &str,
    ) -> Result<(), OrchestratorError> {
        self.commit_all_with_message(worktree_path, &format!("Build: {goal}"))
    }
    /// Stage everything and commit with `message` verbatim; a clean tree is a
    /// no-op (nothing staged, nothing committed).
    pub(super) fn commit_all_with_message(
        &self,
        worktree_path: &Path,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        // The scaffolded MCP config is machine-local plumbing (absolute binary
        // path, per-task identity): committing it would merge it into the base
        // branch and add/add-conflict against every other branch's copy. It is
        // kept out of every commit by `.build/.gitignore` (written at scaffold
        // time), which `git add -A` honors silently — and which also guards the
        // agent's own commits. (A `:(exclude)` pathspec here would instead ERROR,
        // since it names an ignored path explicitly.)
        run_git(worktree_path, &["add", "-A", "--", "."])?;
        // Only commit if something is staged (the MCP config alone must not
        // produce a commit).
        let staged = run_git(worktree_path, &["diff", "--cached", "--name-only"])?;
        if !staged.trim().is_empty() {
            run_git(worktree_path, &["commit", "-m", message])?;
        }
        Ok(())
    }
}
