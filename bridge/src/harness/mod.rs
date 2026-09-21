//! The API in front of a coding-agent harness.
//!
//! Two traits, and everything a provider knows lives behind one of them:
//!
//! - [`Harness`] is the launch side — what to exec, which flags carry a model
//!   and a reasoning effort, how the provider is told about Build's MCP server,
//!   what has to be true of a worktree before a session opens in it, and
//!   whether the provider already has a conversation for that worktree to
//!   resume. One implementation per [`AgentProvider`], reached only through
//!   [`harness_for`].
//! - [`AgentSession`] is the running side — the calls the daemon makes on a
//!   session once it exists, with [`TerminalView`] as the capability a session
//!   offers only when the harness behind it is opaque enough to need an escape
//!   hatch. It is the whole vocabulary: nothing above [`crate::pty`] knows
//!   what a session is carried over.
//!
//! Provider launch dispatch is centralized in [`harness_for`]. Policies that
//! decide whether a provider is eligible for a role, such as router validation,
//! remain separate from launch construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use portable_pty::PtySize;

use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::{HarnessSpec, PtySession};

pub(crate) mod adk;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod codex_app_server;
pub(crate) mod pi;
mod session;
pub mod shell_tail;
#[cfg(test)]
pub(crate) mod stream_fixtures;
pub mod surfaces;
pub(crate) mod transcript_activity;

pub use session::{
    ActivityReport, AgentActivity, AgentSession, AgentStatus, FrozenTurnChoice, HarnessError,
    SessionOutput, SessionStatusSnapshot, TerminalView, ToolOutcome, Turn, TurnChoiceSupport,
    TurnReceiptSnapshot, TurnReceiptSupport,
};

/// How long a real harness TUI must stop painting before its input is live.
///
/// Startup is a burst with GAPS in it, so the window has to outlast the largest
/// gap inside the burst rather than merely the first pause. Measured against
/// claude 2.1.219: the largest intra-startup gap is ~400ms and the
/// alternate-screen clear lands after a 311ms lull, so a prompt written inside
/// that lull is wiped by a clear that has not happened yet.
pub const REAL_TUI_SETTLE: Duration = Duration::from_millis(750);

/// How long a real TUI's submit key must trail the pasted prompt.
///
/// Written together they arrive in one stdin read, and the harness's editor
/// handles the Enter before the paste has committed to its composer — the turn
/// sits pasted, never submitted. Measured against claude 2.1.223 (PTY probe,
/// idle machine): the composer echoes the paste ~640ms after the write, an
/// Enter at +200ms is swallowed, and an Enter at +1000ms submits reliably. The
/// margin over that covers a loaded machine.
pub const REAL_TUI_SUBMIT_DELAY: Duration = Duration::from_millis(1500);

/// Session markers a parent agent leaves in the environment.
///
/// A harness that finds its own markers treats itself as a nested child of that
/// session rather than its own — claude disables transcript saving, which
/// breaks the `--continue` adoption path Build depends on. Build spawns agents
/// from a process that may itself be an agent, so every harness clears these:
/// Build's agents are always their own sessions.
pub const INHERITED_AGENT_MARKERS: [&str; 6] = [
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_EFFORT",
];

/// What the daemon supplies to every harness at spawn time, whichever provider
/// is being launched.
#[derive(Debug, Clone)]
pub struct HarnessContext {
    /// The daemon's own executable. Providers that spawn Build's MCP server
    /// themselves re-exec this rather than a binary looked up on PATH.
    pub bridge_exe: PathBuf,
    /// The socket that session's MCP server is reachable on.
    pub mcp_socket: PathBuf,
    /// The private Build state associated with the configured store.
    pub state_root: PathBuf,
}

impl HarnessContext {
    pub fn resolved(mcp_socket: PathBuf, state_root: PathBuf) -> Result<Self, HarnessError> {
        std::fs::create_dir_all(&state_root).map_err(|error| {
            HarnessError::Setup(format!(
                "create harness state root {}: {error}",
                state_root.display()
            ))
        })?;
        let state_root = std::fs::canonicalize(&state_root).map_err(|error| {
            HarnessError::Setup(format!(
                "canonicalize harness state root {}: {error}",
                state_root.display()
            ))
        })?;
        let bridge_exe = std::env::current_exe()
            .map_err(|error| HarnessError::Setup(format!("resolve bridge executable: {error}")))?;
        let bridge_exe = std::fs::canonicalize(&bridge_exe).map_err(|error| {
            HarnessError::Setup(format!(
                "canonicalize bridge executable {}: {error}",
                bridge_exe.display()
            ))
        })?;
        if !bridge_exe.is_absolute() {
            return Err(HarnessError::Setup(format!(
                "bridge executable is not absolute: {}",
                bridge_exe.display()
            )));
        }
        Ok(HarnessContext {
            bridge_exe,
            mcp_socket,
            state_root,
        })
    }
}

/// Everything Build knows about one coding-agent provider.
pub trait Harness: Send + Sync {
    /// The provider this implementation is reached by.
    fn provider(&self) -> AgentProvider;

    /// What a human sees this provider called.
    fn label(&self) -> &'static str;

    /// The program this harness execs, as it is looked up on `PATH`.
    ///
    /// Stated rather than read back out of [`Harness::spec`]: whether a
    /// harness can run here is a question worth answering before anybody
    /// tries, and building a whole spec to find out would need a checkout, a
    /// model choice and a session that does not exist yet.
    fn binary(&self) -> &'static str;

    /// The curated model catalog, most capable first.
    fn models(&self) -> Vec<ModelOption>;

    /// Every reasoning effort this provider accepts, across all its models.
    /// A model may still accept only some of them ([`ModelOption::efforts`]).
    fn effort_levels(&self) -> &'static [&'static str];

    /// The argv fragment carrying a model selection. Providers disagree about
    /// reasoning effort in particular: a flag for one, a config override for
    /// another.
    fn model_args(&self, choice: &ModelChoice) -> Vec<String>;

    /// Whether `prompt` is a provider command that must be delivered without
    /// Build's native-delivery envelope or conversation instructions.
    fn requires_unadorned_command(&self, _prompt: &str) -> bool {
        false
    }

    /// Whether an exact terminal command starts a compaction cycle whose
    /// completion will later appear in the provider transcript.
    fn starts_compaction(&self, _prompt: &str) -> bool {
        false
    }

    /// The command that opens an interactive session for `options`.
    ///
    /// The prompt is never part of this: every turn travels through the session
    /// after it is running, never baked into argv.
    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, HarnessError>;

    /// Open this provider's running session and subscribe to its output before
    /// startup can emit anything.
    ///
    /// Opaque CLI providers share this PTY implementation. Protocol providers
    /// override it so their concrete process and session types remain private
    /// to the provider module.
    fn open_session(&self, request: SessionOpenRequest) -> Result<OpenedSession, HarnessError> {
        open_terminal_session(&request.spec, request.root, request.terminal)
    }

    /// Whether this provider can carry a router session.
    ///
    /// The router speaks Build's own MCP surface. A provider that reaches
    /// Build's tools some other way has no router surface to speak, and says
    /// so here, so the router's configuration check asks the provider rather
    /// than naming it.
    fn routes_captures(&self) -> bool {
        true
    }

    /// Whether a session opened for this provider offers a terminal.
    fn has_terminal(&self) -> bool {
        true
    }

    /// Make `cwd` fit for this provider to open a session in, before one is
    /// spawned there.
    ///
    /// Providers gate an interactive session behind a workspace-trust dialog
    /// for a directory they have not seen, and Build mints a fresh worktree per
    /// run — so a provider that keeps trust in shared state has to record it
    /// here or the dialog eats the injected prompt. A provider that takes the
    /// grant in its own argv needs nothing, which is why this defaults to doing
    /// nothing.
    fn prepare_workspace(&self, cwd: &Path) {
        let _ = cwd;
    }

    /// Whether this provider already holds a conversation for `cwd` under
    /// `home` — the one question a resume turns on.
    ///
    /// Heuristic by design: a false negative costs a fresh session, never a
    /// wrong one. `home` is a parameter rather than read here so tests never
    /// touch the developer's real home directory.
    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool;

    /// A locator that will name the conversation a session opened in `cwd` is
    /// having, watched off this provider's own transcript tree under `home`.
    ///
    /// The mirror of [`has_transcript`](Harness::has_transcript), with `home` a
    /// parameter for the same reason: tests never read the real `~/.claude` or
    /// `~/.codex`. Built BEFORE the child exists, so what is already in the
    /// tree can be told from what the child writes.
    ///
    /// `None` by default, which is the answer for a session protocol that announces its
    /// own id: a locator standing beside that announcement would be two records
    /// of one answer, free to disagree.
    fn session_locator(&self, home: &Path, cwd: &Path) -> Option<Box<dyn SessionLocator>> {
        let _ = (home, cwd);
        None
    }

    /// Whether the conversation `id` names is still in this provider's tree —
    /// asked before a recorded id is spent, so a dead one costs zero restarts
    /// instead of one.
    ///
    /// `true` by default: a provider that keeps no tree Build can read has no
    /// grounds to refuse an id, and refusing on a doubt would throw away a good
    /// conversation. This is also what makes a provider swap safe — an agent
    /// moved from one harness to another holds an id the new one does not
    /// recognize, and the check clears it rather than letting the resume choke.
    fn holds_conversation(&self, home: &Path, cwd: &Path, id: &str) -> bool {
        let _ = (home, cwd, id);
        true
    }
}

/// Locates a terminal session identity from durable provider state when launch
/// arguments do not already determine it.
pub trait SessionLocator: Send + Sync {
    /// The id, once exactly one transcript this session could be has appeared.
    /// `None` until then; cached once found, so a locator never changes its
    /// answer and the steady-state cost is a field read.
    fn session_id(&self) -> Option<String>;
    fn activity(
        &self,
        _known_session_id: Option<&str>,
        _terminal_alive: std::sync::Weak<
            std::sync::Mutex<Option<tokio::sync::broadcast::Sender<Vec<u8>>>>,
        >,
    ) -> Option<tokio::sync::broadcast::Receiver<ActivityReport>> {
        None
    }
}

pub enum SessionIdentitySource {
    /// The launch contract fixes the conversation identity before spawn.
    Known(String),
    /// Durable provider state identifies the conversation after spawn.
    Located(Box<dyn SessionLocator>),
}

/// Whether `id` is a name a transcript file can be looked up by.
///
/// A recorded id rides a JSON record on disk and is spent as a path component,
/// so the one that walks out of the tree it names is refused before any
/// filesystem call is made. Every real id from either provider is a uuid, so
/// this rejects nothing a harness actually writes.
pub(crate) fn is_a_filename(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Terminal mechanics supplied to a provider without deciding that provider's
/// carrier. Protocol harnesses ignore them and own their startup mechanics.
///
/// `identity` is how a terminal answers [`AgentSession::session_id`]: a
/// launch-known id or a provider watcher built before the child exists.
/// `None` is for the human's shell, which has no conversation to name.
pub struct TerminalOpenOptions {
    pub size: PtySize,
    pub turn_ready_grace: Option<Duration>,
    pub identity: Option<SessionIdentitySource>,
    pub activity_locator: Option<(Box<dyn SessionLocator>, Option<String>)>,
}

/// Everything a provider needs to construct one live session.
pub struct SessionOpenRequest {
    pub spec: HarnessSpec,
    pub root: PathBuf,
    pub choice: ModelChoice,
    pub terminal: TerminalOpenOptions,
    pub resume_session_id: Option<String>,
}

/// A live session and the output subscribed during its construction.
pub struct OpenedSession {
    pub session: Arc<dyn AgentSession>,
    pub output: SessionOutput,
}

/// Open a provider's live session through its harness implementation.
///
/// The session comes back behind an [`Arc`] because the daemon keeps it inside
/// the state it locks, and hands turns to it with that lock RELEASED — see
/// [`AgentSession::send_turn`]. A shared handle is what lets a caller take the
/// session out of the registry without holding the registry open across the
/// turn.
///
pub fn open_session(
    provider: AgentProvider,
    request: SessionOpenRequest,
) -> Result<OpenedSession, HarnessError> {
    open_session_with_harness(harness_for(provider), request)
}

fn open_session_with_harness(
    harness: &dyn Harness,
    request: SessionOpenRequest,
) -> Result<OpenedSession, HarnessError> {
    let opened = harness.open_session(request)?;
    refuse_a_session_nobody_can_watch(opened.session.as_ref())?;
    Ok(opened)
}

/// Open a PTY session and subscribe before waiting for its line editor.
///
/// Harnesses use this as their shared default. The app also uses it directly
/// for a human shell, which has no provider to dispatch through.
pub(crate) fn open_terminal_session(
    spec: &HarnessSpec,
    root: PathBuf,
    options: TerminalOpenOptions,
) -> Result<OpenedSession, HarnessError> {
    let session = PtySession::spawn(spec, Some(root), options.size)?
        .with_session_identity(options.identity)
        .with_activity_locator(options.activity_locator);
    let output = match session.terminal() {
        Some(terminal) => {
            SessionOutput::painting_with_activity(terminal.subscribe(), session.activity())
        }
        None => SessionOutput::silent(),
    };
    if let Some(grace) = options.turn_ready_grace {
        session.ready_within(grace);
    }
    Ok(OpenedSession {
        session: Arc::new(session),
        output,
    })
}

/// Refuse a session that offers neither a terminal nor an activity stream.
///
/// The two capabilities are alternatives, not extras, and a session with
/// neither is worse than one Build cannot see working: the death rites hang off
/// a stream CLOSING — the tab going not live, the conversation's session
/// lineage ending — so a session with no stream would leave a dead agent's tab
/// reading as live until the idle sweep explained the exit as silence. Better
/// to never open it.
fn refuse_a_session_nobody_can_watch(session: &dyn AgentSession) -> Result<(), HarnessError> {
    if session.terminal().is_some() || session.activity().is_some() {
        return Ok(());
    }
    session.end();
    Err(HarnessError::Session(
        "this harness offers neither a terminal nor an activity stream, so Build could not see it \
         working or learn that it had stopped"
            .to_string(),
    ))
}

/// The implementation for `provider`. The only way to reach one.
pub fn harness_for(provider: AgentProvider) -> &'static dyn Harness {
    match provider {
        AgentProvider::Claude => &claude::ClaudeHarness,
        AgentProvider::Codex => &codex::CodexHarness,
        AgentProvider::ClaudeAdk => &adk::AdkHarness,
        AgentProvider::CodexAppServer => &codex_app_server::CodexAppServerHarness,
        AgentProvider::Pi => &pi::PiHarness,
    }
}

#[cfg(test)]
mod tests {
    use syn::punctuated::Punctuated;
    use syn::visit::Visit;

    use super::*;

    /// Every provider is reachable and answers for itself — the invariant that
    /// makes `harness_for` the single registry rather than one of several.
    #[test]
    fn every_provider_has_a_harness_that_names_itself() {
        for provider in AgentProvider::ALL {
            let harness = harness_for(provider);
            assert_eq!(harness.provider(), provider);
            assert!(!harness.label().is_empty(), "{provider:?}");
            assert!(!harness.effort_levels().is_empty(), "{provider:?}");
        }
    }

    #[test]
    fn resolved_context_uses_the_canonical_bridge_and_configured_state_root() {
        let state = tempfile::tempdir().unwrap();
        let configured_state = state.path().join("private-state");
        std::fs::create_dir(&configured_state).unwrap();
        let context = HarnessContext::resolved(
            state.path().join("mcp.sock"),
            configured_state.join("..").join("private-state"),
        )
        .unwrap();
        assert!(context.bridge_exe.is_absolute());
        assert_eq!(
            context.bridge_exe,
            std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
        );
        assert_eq!(
            context.state_root,
            std::fs::canonicalize(configured_state).unwrap()
        );
        assert_eq!(context.mcp_socket, state.path().join("mcp.sock"));
        assert_ne!(context.bridge_exe, PathBuf::from("build-bridge"));
    }

    /// A catalog entry a harness advertises must be one the same harness will
    /// accept back, or the picker offers a selection that cannot be dispatched.
    #[test]
    fn every_catalogued_model_validates_against_its_own_provider() {
        for provider in AgentProvider::ALL {
            for model in harness_for(provider).models() {
                let bare = ModelChoice {
                    provider,
                    model: Some(model.id.to_string()),
                    effort: None,
                };
                assert!(bare.validate().is_ok(), "{provider:?} {}", model.id);
                for effort in model.efforts {
                    let with_effort = ModelChoice {
                        effort: Some((*effort).to_string()),
                        ..bare.clone()
                    };
                    assert!(
                        with_effort.validate().is_ok(),
                        "{provider:?} {} {effort}",
                        model.id
                    );
                }
            }
        }
    }

    /// No selection means the provider's own configuration decides, so a
    /// default choice contributes nothing to argv on any provider.
    #[test]
    fn a_default_choice_adds_no_model_args_on_any_provider() {
        for provider in AgentProvider::ALL {
            let choice = ModelChoice {
                provider,
                ..ModelChoice::default()
            };
            assert!(
                harness_for(provider).model_args(&choice).is_empty(),
                "{provider:?}"
            );
        }
    }

    /// The terminal is a capability, and exactly the opaque CLI wrappers have
    /// it: Build sees what it launched and what they reported, and nothing in
    /// between, so the human needs the escape hatch. A protocol carrier
    /// reports its own reasoning and tool calls, so it has nothing to escape
    /// to — and this is the answer the rail and the spawn BOTH read, which is
    /// what keeps the rail from offering a button the spawn would refuse.
    #[test]
    fn only_the_opaque_cli_wrappers_offer_a_terminal() {
        for provider in [
            AgentProvider::Claude,
            AgentProvider::Codex,
            AgentProvider::Pi,
        ] {
            assert!(harness_for(provider).has_terminal(), "{provider:?}");
        }
        for provider in [AgentProvider::ClaudeAdk, AgentProvider::CodexAppServer] {
            assert!(!harness_for(provider).has_terminal(), "{provider:?}");
        }
    }

    /// Transcript-backed terminals use locators. A protocol session announces
    /// its id, while Pi's terminal identity is fixed by its launch contract;
    /// neither needs a second transcript-derived answer.
    #[test]
    fn transcript_backed_terminals_have_locators_and_launch_named_sessions_do_not() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp worktree");
        for provider in [AgentProvider::Claude, AgentProvider::Codex] {
            assert!(
                harness_for(provider)
                    .session_locator(home.path(), cwd.path())
                    .is_some(),
                "{provider:?}"
            );
        }
        for provider in [
            AgentProvider::ClaudeAdk,
            AgentProvider::CodexAppServer,
            AgentProvider::Pi,
        ] {
            assert!(
                harness_for(provider)
                    .session_locator(home.path(), cwd.path())
                    .is_none(),
                "{provider:?}"
            );
        }
    }

    /// A recorded id is verified against the provider that would spend it, and
    /// an empty tree holds nobody's conversation — which is what clears the id
    /// an agent moved between providers still carries.
    #[test]
    fn an_id_no_provider_holds_is_not_spent_by_any_of_them() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp worktree");
        for provider in AgentProvider::ALL {
            assert!(
                !harness_for(provider).holds_conversation(home.path(), cwd.path(), "sess-1"),
                "{provider:?}"
            );
        }

        // Both codex carriers read the ONE rollout tree, so a thread id the
        // app-server recorded verifies under both — and a stale one starts
        // fresh instead of failing `thread/resume`.
        let id = "99999999-9999-9999-9999-999999999999";
        let dated = home.path().join(".codex/sessions/2026/09/13");
        std::fs::create_dir_all(&dated).expect("the rollout directory");
        std::fs::write(
            dated.join(format!("rollout-2026-09-13T10-00-00-{id}.jsonl")),
            "{}\n",
        )
        .expect("the rollout");
        for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
            assert!(
                harness_for(provider).holds_conversation(home.path(), cwd.path(), id),
                "{provider:?}"
            );
        }

        // Both claude providers write and read the ONE tree, so an id captured
        // under either verifies under both.
        let project = home
            .path()
            .join(".claude/projects")
            .join(claude::encode_project_dir(cwd.path()));
        std::fs::create_dir_all(&project).expect("the transcript directory");
        std::fs::write(project.join("sess-1.jsonl"), "{}\n").expect("the transcript");
        for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
            assert!(
                harness_for(provider).holds_conversation(home.path(), cwd.path(), "sess-1"),
                "{provider:?}"
            );
        }
        assert!(
            !harness_for(AgentProvider::Codex).holds_conversation(
                home.path(),
                cwd.path(),
                "sess-1"
            ),
            "codex keeps its own tree and does not recognize a claude uuid"
        );
    }

    /// A harness that announces its line editor only after a pause, the way a
    /// real TUI does.
    fn slow_to_open_spec() -> HarnessSpec {
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("sleep 0.3; printf '\\033[?2004h'; cat >/dev/null")
    }

    fn one_pty() -> PtySize {
        PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// A session Build will hand a turn to is opened READY.
    ///
    /// An interactive TUI paints its banner — or a modal workspace-trust dialog
    /// — long before its line editor will take a turn, so a prompt written on
    /// first byte lands in whatever owns the keyboard and the submit key
    /// answers it. Waiting that out is how a terminal opens, so it happens
    /// where the terminal is chosen rather than at the caller.
    #[test]
    fn a_session_that_will_be_handed_a_turn_opens_ready() {
        let root = tempfile::tempdir().expect("temp worktree");
        let started = std::time::Instant::now();
        let opened = open_terminal_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: Some(Duration::from_secs(5)),
                identity: None,
                activity_locator: None,
            },
        )
        .expect("the session opens");
        let waited = started.elapsed();
        opened.session.end();
        assert!(
            waited >= Duration::from_millis(300),
            "the open returned before the harness would take a turn, after {waited:?}"
        );
    }

    /// And a session no turn is coming for is NOT waited on.
    ///
    /// The human's own shell is opened this way: it may never announce a line
    /// editor at all, and `term.create` holds the app-wide state lock across
    /// the open — so a wait for a signal that never comes would stall every
    /// project for the whole grace.
    #[test]
    fn a_session_with_no_turn_coming_is_not_waited_on() {
        let root = tempfile::tempdir().expect("temp worktree");
        let started = std::time::Instant::now();
        let opened = open_terminal_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: None,
                identity: None,
                activity_locator: None,
            },
        )
        .expect("the session opens");
        let waited = started.elapsed();
        opened.session.end();
        assert!(
            waited < Duration::from_millis(200),
            "opening a session nobody will speak to waited {waited:?} for readiness"
        );
    }

    /// The locator travels with the terminal that has a use for it: a terminal
    /// hands its answers back as its own, and a session Build opens with none
    /// — the human's shell, and every session protocol that names its conversation
    /// itself — names nothing.
    #[tokio::test]
    async fn only_a_terminal_opened_with_a_locator_names_its_conversation() {
        struct Says(&'static str);
        impl SessionLocator for Says {
            fn session_id(&self) -> Option<String> {
                Some(self.0.to_string())
            }
        }

        let root = tempfile::tempdir().expect("temp worktree");
        let named = open_terminal_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: None,
                identity: Some(SessionIdentitySource::Located(Box::new(Says(
                    "sess-located",
                )))),
                activity_locator: None,
            },
        )
        .expect("the session opens");
        assert_eq!(named.session.session_id().as_deref(), Some("sess-located"));
        named.session.end();

        let unnamed = open_terminal_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: None,
                identity: None,
                activity_locator: None,
            },
        )
        .expect("the session opens");
        assert_eq!(unnamed.session.session_id(), None);
        unnamed.session.end();
    }

    #[test]
    fn a_terminal_with_a_launch_known_identity_names_itself_immediately() {
        let root = tempfile::tempdir().expect("temp worktree");
        let named = open_terminal_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: None,
                identity: Some(SessionIdentitySource::Known("agent-pi".to_string())),
                activity_locator: None,
            },
        )
        .expect("the session opens");
        assert_eq!(named.session.session_id().as_deref(), Some("agent-pi"));
        named.session.end();
    }

    /// A fake stream-json harness: it announces its session, then sits with its
    /// stdin open the way the real one does between turns.
    fn fake_protocol_spec() -> HarnessSpec {
        HarnessSpec::new("sh").arg("-c").arg(
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}'; \
             cat >/dev/null",
        )
    }

    fn protocol_open_request(provider: AgentProvider, root: &Path) -> SessionOpenRequest {
        SessionOpenRequest {
            spec: fake_protocol_spec(),
            root: root.to_path_buf(),
            choice: ModelChoice {
                provider,
                ..ModelChoice::default()
            },
            terminal: TerminalOpenOptions {
                size: one_pty(),
                turn_ready_grace: None,
                identity: None,
                activity_locator: None,
            },
            resume_session_id: None,
        }
    }

    #[test]
    fn public_open_session_matches_each_harness_declared_output_capability() {
        for provider in AgentProvider::ALL {
            let root = tempfile::tempdir().expect("temp worktree");
            let opened = open_session(provider, protocol_open_request(provider, root.path()))
                .expect("the provider opens its session");

            let terminal = harness_for(provider).has_terminal();
            assert_eq!(opened.session.terminal().is_some(), terminal);
            assert_eq!(opened.output.bytes.is_some(), terminal);
            assert_eq!(opened.output.activity.is_some(), !terminal);
            opened.session.end();
        }
    }

    #[test]
    fn public_construction_refuses_and_ends_an_invisible_session() {
        struct MuteSession {
            ended: Arc<std::sync::atomic::AtomicBool>,
        }

        impl AgentSession for MuteSession {
            fn send_turn(&self, _turn: &Turn) -> Result<(), HarnessError> {
                Ok(())
            }
            fn status(&self) -> AgentStatus {
                AgentStatus::Waiting
            }
            fn quiet_for(&self) -> Duration {
                Duration::ZERO
            }
            fn exited_within(&self, _timeout: Duration) -> bool {
                false
            }
            fn end(&self) {
                self.ended.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            fn backdate_last_output(&self, _ago: Duration) {}
        }

        struct MuteHarness {
            ended: Arc<std::sync::atomic::AtomicBool>,
        }

        impl Harness for MuteHarness {
            fn provider(&self) -> AgentProvider {
                AgentProvider::Claude
            }

            fn label(&self) -> &'static str {
                "mute"
            }

            fn binary(&self) -> &'static str {
                "sh"
            }

            fn models(&self) -> Vec<ModelOption> {
                claude::ClaudeHarness.models()
            }

            fn effort_levels(&self) -> &'static [&'static str] {
                claude::ClaudeHarness.effort_levels()
            }

            fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
                claude::ClaudeHarness.model_args(choice)
            }

            fn spec(
                &self,
                choice: &ModelChoice,
                options: &SpawnOptions,
                context: &HarnessContext,
            ) -> Result<HarnessSpec, HarnessError> {
                claude::ClaudeHarness.spec(choice, options, context)
            }

            fn open_session(
                &self,
                _request: SessionOpenRequest,
            ) -> Result<OpenedSession, HarnessError> {
                Ok(OpenedSession {
                    session: Arc::new(MuteSession {
                        ended: Arc::clone(&self.ended),
                    }),
                    output: SessionOutput::silent(),
                })
            }

            fn has_transcript(&self, _home: &Path, _cwd: &Path) -> bool {
                false
            }
        }

        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let harness = MuteHarness {
            ended: Arc::clone(&ended),
        };
        let root = tempfile::tempdir().expect("temp worktree");
        let refusal = match open_session_with_harness(
            &harness,
            protocol_open_request(AgentProvider::Claude, root.path()),
        ) {
            Err(refusal) => refusal,
            Ok(opened) => {
                opened.session.end();
                panic!("a session nobody can watch was opened")
            }
        };
        assert!(
            refusal
                .to_string()
                .contains("neither a terminal nor an activity stream"),
            "the refusal says what is missing: {refusal}"
        );
        assert!(ended.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// A worktree Build has never opened has no conversation to resume, on any
    /// provider — the false that keeps a fresh run from inheriting a stranger's
    /// context.
    #[test]
    fn an_unseen_worktree_has_no_transcript_on_any_provider() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp worktree");
        for provider in AgentProvider::ALL {
            assert!(
                !harness_for(provider).has_transcript(home.path(), cwd.path()),
                "{provider:?}"
            );
        }
    }

    /// Whether `attributes` gate their item behind `#[cfg(test)]`, so its
    /// whole body is test code the guards leave alone.
    fn is_test_gated(attributes: &[syn::Attribute]) -> bool {
        attributes.iter().any(|attribute| {
            attribute.path().is_ident("cfg")
                && attribute
                    .parse_args::<syn::Ident>()
                    .is_ok_and(|gate| gate == "test")
        })
    }

    /// `items` with every `#[cfg(test)]`-gated module dropped, at any depth,
    /// so scaffolding that drives one carrier on purpose is not read as a
    /// production dispatch.
    fn without_test_gated_modules(items: Vec<syn::Item>) -> Vec<syn::Item> {
        items
            .into_iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) if is_test_gated(&module.attrs) => None,
                syn::Item::Mod(mut module) => {
                    module.content = module
                        .content
                        .map(|(brace, nested)| (brace, without_test_gated_modules(nested)));
                    Some(syn::Item::Mod(module))
                }
                other => Some(other),
            })
            .collect()
    }

    /// The syntax tree of what ships from `source`: the module as the compiler
    /// reads it, minus its test-gated modules.
    fn shipped_syntax_of(source: &str) -> syn::File {
        let module = syn::parse_file(source).expect("a module the compiler accepts");
        syn::File {
            items: without_test_gated_modules(module.items),
            ..module
        }
    }

    /// The implementation types no module above `harness_for` may name: the
    /// harnesses `harness_for` constructs, and the sessions they open. Kept
    /// honest against its owner by
    /// [`the_ban_list_covers_every_harness_harness_for_constructs`].
    const CONCRETE_HARNESS_TYPES: [&str; 7] = [
        "AdkHarness",
        "AdkSession",
        "ClaudeHarness",
        "CodexHarness",
        "CodexAppServerHarness",
        "CodexAppServerSession",
        "PiHarness",
    ];

    /// The harness types `harness_for` names, read out of the function itself so
    /// the guard's ban list is checked against the registry that owns the fact
    /// rather than against a second hand-kept copy of it.
    fn harnesses_named_by_harness_for() -> Vec<String> {
        const REGISTRY: &str = include_str!("mod.rs");
        const SIGNATURE: &str = "pub fn harness_for";

        let body = REGISTRY
            .split_once(SIGNATURE)
            .expect("harness_for is defined in this module")
            .1
            .split_once("\n}\n")
            .expect("harness_for's body closes")
            .0;
        body.lines()
            .filter_map(|line| line.trim().strip_suffix(','))
            .filter(|arm| arm.contains("=> &"))
            .map(|arm| {
                arm.rsplit("::")
                    .next()
                    .expect("a dispatch arm names a type")
                    .to_string()
            })
            .collect()
    }

    /// Collects the types a module's shipped `impl AgentSession for` blocks
    /// name.
    #[derive(Default)]
    struct SessionImplementors {
        types: Vec<String>,
    }

    impl<'ast> Visit<'ast> for SessionImplementors {
        fn visit_item_impl(&mut self, block: &'ast syn::ItemImpl) {
            let implements_a_session = block.trait_.as_ref().is_some_and(|(_, implemented, _)| {
                implemented
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "AgentSession")
            });
            if let (true, syn::Type::Path(implementor)) = (implements_a_session, &*block.self_ty) {
                self.types.push(
                    implementor
                        .path
                        .segments
                        .last()
                        .expect("a type path has a segment")
                        .ident
                        .to_string(),
                );
            }
        }
    }

    /// The session types the harness modules open, read out of every shipped
    /// `impl AgentSession for` under `src/harness` so the guard's ban list is
    /// checked against the modules that own those types rather than against a
    /// second hand-kept copy. `PtySession` lives in `src/pty.rs`, outside the
    /// walk, and stays out of the list.
    fn sessions_opened_by_the_harness_modules() -> Vec<String> {
        let harness_modules = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("harness");
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        shipped_rust_sources_under(&source_root, &harness_modules)
            .iter()
            .flat_map(|path| {
                let source = std::fs::read_to_string(path).expect("a readable harness module");
                let mut implementors = SessionImplementors::default();
                implementors.visit_file(&shipped_syntax_of(&source));
                implementors.types
            })
            .collect()
    }

    fn entries(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(directory)
            .expect("a readable source directory")
            .map(|entry| entry.expect("a readable source entry").path())
            .collect()
    }

    fn is_rust_source(path: &std::path::Path) -> bool {
        path.extension().is_some_and(|extension| extension == "rs")
    }

    /// Every Rust source that ships from `directory` and the directories under
    /// it, leaving out the modules the directory's declaring file gates behind
    /// `#[cfg(test)]`, whose whole bodies are test code.
    fn shipped_rust_sources_under(
        source_root: &std::path::Path,
        directory: &std::path::Path,
    ) -> Vec<std::path::PathBuf> {
        let test_only_modules = test_only_modules_declared_for(source_root, directory);
        let (subdirectories, files): (Vec<_>, Vec<_>) = entries(directory)
            .into_iter()
            .partition(|path| path.is_dir());
        files
            .into_iter()
            .filter(|path| is_rust_source(path))
            .filter(|path| {
                let module = path
                    .file_stem()
                    .expect("a Rust source names its module")
                    .to_string_lossy();
                !test_only_modules
                    .iter()
                    .any(|test_only| *test_only == module)
            })
            .chain(
                subdirectories
                    .iter()
                    .filter(|path| {
                        let module = path
                            .file_name()
                            .expect("a source directory names its module")
                            .to_string_lossy();
                        !test_only_modules
                            .iter()
                            .any(|test_only| *test_only == module)
                    })
                    .flat_map(|subdirectory| shipped_rust_sources_under(source_root, subdirectory)),
            )
            .collect()
    }

    /// The modules every file declaring `directory`'s children places behind
    /// `#[cfg(test)]`. A crate source root may be shared by `lib.rs` and
    /// `main.rs`; a module shipped by either root remains in the walk.
    fn test_only_modules_declared_for(
        source_root: &std::path::Path,
        directory: &std::path::Path,
    ) -> Vec<String> {
        let declaring_files = if directory == source_root {
            vec![source_root.join("lib.rs"), source_root.join("main.rs")]
        } else {
            vec![directory.join("mod.rs"), directory.with_extension("rs")]
        };
        let declarations: Vec<_> = declaring_files
            .into_iter()
            .filter(|candidate| candidate.is_file())
            .flat_map(|declaring_file| {
                let source =
                    std::fs::read_to_string(declaring_file).expect("a readable module file");
                out_of_line_module_declarations_in(&source)
            })
            .collect();
        let mut modules: Vec<_> = declarations
            .iter()
            .filter(|(_, test_only)| *test_only)
            .filter(|(candidate, _)| {
                !declarations
                    .iter()
                    .any(|(module, test_only)| module == candidate && !test_only)
            })
            .map(|(module, _)| module.clone())
            .collect();
        modules.sort();
        modules.dedup();
        modules
    }

    /// The out-of-line modules (`mod name;`) that `source` declares behind
    /// `#[cfg(test)]`. An inline gated module carries its own body and
    /// declares no file.
    fn test_only_modules_in(source: &str) -> Vec<String> {
        out_of_line_module_declarations_in(source)
            .into_iter()
            .filter_map(|(module, test_only)| test_only.then_some(module))
            .collect()
    }

    fn out_of_line_module_declarations_in(source: &str) -> Vec<(String, bool)> {
        syn::parse_file(source)
            .expect("a module the compiler accepts")
            .items
            .into_iter()
            .filter_map(|item| match item {
                syn::Item::Mod(module) if module.content.is_none() => {
                    Some((module.ident.to_string(), is_test_gated(&module.attrs)))
                }
                _ => None,
            })
            .collect()
    }

    const PROVIDER_ENUM: &str = "AgentProvider";

    /// The variant `path` names when it reaches into `AgentProvider`, whether
    /// written bare or through the modules above it.
    fn provider_variant(path: &syn::Path) -> Option<String> {
        let segments: Vec<&syn::PathSegment> = path.segments.iter().collect();
        segments
            .windows(2)
            .find(|pair| pair[0].ident == PROVIDER_ENUM)
            .map(|pair| pair[1].ident.to_string())
    }

    /// The path a pattern matches by, when it matches by one: a bare path, or
    /// the struct or tuple variant it destructures.
    fn pattern_path(pattern: &syn::Pat) -> Option<&syn::Path> {
        match pattern {
            syn::Pat::Path(path) => Some(&path.path),
            syn::Pat::Struct(fields) => Some(&fields.path),
            syn::Pat::TupleStruct(elements) => Some(&elements.path),
            _ => None,
        }
    }

    /// The arguments of a `matches!` call: the scrutinee, the pattern it is
    /// tested against, and whatever guard and trailing comma follow.
    struct MatchesArguments {
        pattern: syn::Pat,
    }

    impl syn::parse::Parse for MatchesArguments {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let _scrutinee: syn::Expr = input.parse()?;
            let _separator: syn::Token![,] = input.parse()?;
            let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
            if input.peek(syn::Token![if]) {
                let _guard_keyword: syn::Token![if] = input.parse()?;
                let _guard: syn::Expr = input.parse()?;
            }
            let _trailing_comma: Option<syn::Token![,]> = input.parse()?;
            Ok(Self { pattern })
        }
    }

    /// The expressions a macro is called with, for the macros that take them —
    /// `assert!`, `format!` and their kin — so a dispatch written inside one is
    /// still walked. A macro fed something else (`thread_local!`,
    /// `macro_rules!`) has no expression to walk.
    fn expression_arguments(
        invocation: &syn::Macro,
    ) -> Option<Punctuated<syn::Expr, syn::Token![,]>> {
        invocation
            .parse_body_with(Punctuated::parse_terminated)
            .ok()
    }

    /// Walks a module's shipped items for a second provider dispatch: a pattern
    /// naming a provider variant wherever patterns go (match arms with or
    /// without guards, `if let`, `while let`, let-else, `matches!`), an
    /// equality test against one, or a mention of a concrete harness or
    /// session type. The first one found is kept.
    #[derive(Default)]
    struct ProviderDispatchFinder {
        offence: Option<String>,
    }

    impl ProviderDispatchFinder {
        fn record(&mut self, offence: String) {
            self.offence.get_or_insert(offence);
        }
    }

    impl<'ast> Visit<'ast> for ProviderDispatchFinder {
        fn visit_pat(&mut self, pattern: &'ast syn::Pat) {
            if let Some(variant) = pattern_path(pattern).and_then(provider_variant) {
                self.record(format!("matches on {PROVIDER_ENUM}::{variant}"));
            }
            syn::visit::visit_pat(self, pattern);
        }

        fn visit_expr_binary(&mut self, comparison: &'ast syn::ExprBinary) {
            let tests_equality = matches!(comparison.op, syn::BinOp::Eq(_) | syn::BinOp::Ne(_));
            let against_a_variant = [&*comparison.left, &*comparison.right]
                .into_iter()
                .find_map(|operand| match operand {
                    syn::Expr::Path(path) => provider_variant(&path.path),
                    _ => None,
                });
            if let (true, Some(variant)) = (tests_equality, against_a_variant) {
                self.record(format!("compares against {PROVIDER_ENUM}::{variant}"));
            }
            syn::visit::visit_expr_binary(self, comparison);
        }

        fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
            let mut inside_the_call = ProviderDispatchFinder::default();
            if invocation.path.is_ident("matches") {
                let tested: MatchesArguments = invocation
                    .parse_body()
                    .expect("matches! takes a scrutinee and a pattern");
                inside_the_call.visit_pat(&tested.pattern);
            } else if let Some(arguments) = expression_arguments(invocation) {
                arguments
                    .iter()
                    .for_each(|argument| inside_the_call.visit_expr(argument));
            }
            if let Some(offence) = inside_the_call.offence {
                self.record(offence);
            }
        }

        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if let Some(harness_type) = CONCRETE_HARNESS_TYPES
                .into_iter()
                .find(|harness_type| ident == harness_type)
            {
                self.record(format!("names {harness_type}"));
            }
        }
    }

    /// What in `source` claims a provider dispatch of its own, if anything.
    fn provider_dispatch_offence(source: &str) -> Option<String> {
        let mut finder = ProviderDispatchFinder::default();
        finder.visit_file(&shipped_syntax_of(source));
        finder.offence
    }

    /// Every shipped module that sits above `harness_for`, including nested
    /// app modules. Only the root `models.rs` provider table and the root
    /// harness implementation subtree sit below this boundary. Read from disk
    /// rather than named one by one so a module added later is guarded without
    /// anyone remembering to list it.
    fn modules_above_harness_for(src: &std::path::Path) -> Vec<(String, String)> {
        let harness = src.join("harness");
        let sanctioned_provider_table = src.join("models.rs");
        let mut modules: Vec<(String, String)> = shipped_rust_sources_under(src, src)
            .into_iter()
            .filter(|path| path != &sanctioned_provider_table)
            .filter(|path| path != &harness.with_extension("rs") && !path.starts_with(&harness))
            .map(|path| {
                let source = std::fs::read_to_string(&path).expect("a readable src module");
                (
                    format!(
                        "src/{}",
                        path.strip_prefix(src)
                            .expect("a source under src")
                            .display()
                    ),
                    source,
                )
            })
            .collect();
        modules.sort();
        modules
    }

    /// `harness_for` is the only place a provider becomes an implementation. A
    /// caller above it that matches on `AgentProvider` or names a concrete
    /// harness or session type has opened a second dispatch, and every provider
    /// added after it has to be added in two places instead of one.
    #[test]
    fn open_session_is_the_only_provider_dispatch() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let modules = modules_above_harness_for(&src);
        assert!(
            modules.iter().any(|(path, _)| path == "src/lib.rs"),
            "the scan reached the crate's source facade, so a green run means something"
        );
        assert!(
            modules.iter().any(|(path, _)| {
                path.strip_prefix("src/app")
                    .is_some_and(|suffix| suffix == ".rs" || suffix.starts_with('/'))
            }),
            "the scan reached the app surface regardless of its facade layout"
        );

        for (path, source) in modules {
            if let Some(offence) = provider_dispatch_offence(&source) {
                panic!(
                    "{path} {offence}; only harness_for may dispatch on a provider, so a second \
                     dispatch above it makes the next provider a two-place change"
                );
            }
        }
    }

    /// The guard fires on every shape a second dispatch actually takes, so a
    /// green [`open_session_is_the_only_provider_dispatch`] is evidence rather
    /// than a detector that quietly stopped matching.
    #[test]
    fn a_second_provider_dispatch_is_caught() {
        const SECOND_DISPATCHES: [&str; 20] = [
            "match (provider, resume) {\n    (AgentProvider::Codex, true) => launch(),\n}",
            "match agent {\n    Agent { provider: AgentProvider::Codex, .. } => launch(),\n}",
            "match providers {\n    [AgentProvider::Codex, ..] => launch(),\n}",
            "match provider {\n    AgentProvider::Codex => launch(),\n}",
            "match named {\n    Some(AgentProvider::Codex) => launch(),\n}",
            "match provider {\n    AgentProvider::Codex if resume => launch(),\n}",
            "match provider {\n    AgentProvider::Codex if attempts > 1 => launch(),\n}",
            "match provider {\n    AgentProvider::Codex if resume.is_some() => launch(),\n}",
            "match provider {\n    AgentProvider::Codex if !resume => launch(),\n}",
            "match kind {\n    Kind::Default => match DEFAULT_PROVIDER {\n        \
             AgentProvider::Codex => launch(),\n    },\n}",
            "match provider {\n    AgentProvider::CodexAppServer\n        => launch(),\n}",
            "if provider == AgentProvider::Codex { launch() }",
            "if provider != AgentProvider::Claude { launch() }",
            "if matches!(provider, AgentProvider::Codex) { launch() }",
            "if matches!(\n    agent.choice.provider,\n    AgentProvider::CodexAppServer | \
             AgentProvider::Codex\n) {\n    launch()\n}",
            "if let AgentProvider::Codex = provider { launch() }",
            "let AgentProvider::Codex = provider else { return };",
            "while let AgentProvider::Codex = next() { launch() }",
            "let session: AdkSession = open(root);",
            "let session: CodexAppServerSession = open(root);",
        ];

        for source in SECOND_DISPATCHES {
            assert!(
                provider_dispatch_offence(&inside_a_function(source)).is_some(),
                "a second dispatch went unnoticed: {source}"
            );
        }

        let single_dispatch = "let provider = AgentProvider::from_wire(id)?;\nlet opened = \
                              harness_for(provider).open_session(request)?;\nmatch named {\n    \
                              Some(agent) if AgentProvider::from_wire(agent).is_some() => \
                              Err(already_named()),\n}";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(single_dispatch)),
            None
        );

        let compared_only_by_value = "if choice.model == catalogued.model { keep() }";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(compared_only_by_value)),
            None
        );

        let constructed_inside_a_binding = "if let Some(found) = catalog(AgentProvider::Claude) \
                                            { keep() }";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(constructed_inside_a_binding)),
            None
        );

        let checked_after_an_earlier_matches = "let quiet = matches!(status, Idle);\nlet \
                                                opened = harness_for(AgentProvider::Codex);";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(checked_after_an_earlier_matches)),
            None
        );

        let constructed_in_an_arm_body = "match found {\n    Ok(_) => AgentProvider::Claude,\n    \
                                         Err(_) => AgentProvider::Codex,\n}";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(constructed_in_an_arm_body)),
            None
        );

        let struct_built_in_an_arm_body = "match found {\n    Some(model) => ModelChoice { \
                                          provider: AgentProvider::Codex, model },\n    None => \
                                          fallback(),\n}";
        assert_eq!(
            provider_dispatch_offence(&inside_a_function(struct_built_in_an_arm_body)),
            None
        );

        let dispatch_only_in_tests = format!(
            "pub fn run() {{}}\n#[cfg(test)]\nmod tests {{\n    {}\n}}\n",
            inside_a_function("let harness = ClaudeHarness;")
        );
        assert_eq!(provider_dispatch_offence(&dispatch_only_in_tests), None);
    }

    /// A statement or expression as it would sit in a shipped function, so a
    /// fixture reads as the compiler would read it.
    fn inside_a_function(body: &str) -> String {
        format!("fn shipped() {{\n{body}\n}}\n")
    }

    /// Only an out-of-line module behind `#[cfg(test)]` names a file the walk
    /// must skip; an inline gated module and an ungated declaration name none.
    #[test]
    fn test_only_modules_are_the_gated_out_of_line_declarations() {
        let declaring_file =
            "mod session;\n#[cfg(test)]\nmod tests;\n#[cfg(test)]\npub(crate) mod \
                              stream_fixtures;\n#[cfg(test)]\nmod inline_tests {\n    fn \
                              scaffold() {}\n}\n";
        assert_eq!(
            test_only_modules_in(declaring_file),
            ["tests", "stream_fixtures"]
        );
    }

    fn write_source(source_root: &std::path::Path, relative: &str, source: &str) {
        let path = source_root.join(relative);
        std::fs::create_dir_all(path.parent().expect("a source file has a parent"))
            .expect("fixture directories are writable");
        std::fs::write(path, source).expect("a fixture source is writable");
    }

    /// The provider guard follows a shipped app through directory modules, so
    /// moving the facade from `app.rs` to `app/mod.rs` cannot hide a dispatch.
    #[test]
    fn nested_app_dispatch_is_guarded_with_a_mod_rs_facade() {
        let fixture = tempfile::tempdir().expect("temp source tree");
        let src = fixture.path().join("src");
        write_source(
            &src,
            "lib.rs",
            "pub mod app;\npub mod harness;\npub mod models;\n",
        );
        write_source(&src, "app/mod.rs", "mod rpc;\n");
        write_source(&src, "app/rpc/mod.rs", "mod dispatch;\n");
        write_source(
            &src,
            "app/rpc/dispatch.rs",
            &inside_a_function("if provider == AgentProvider::Codex { launch() }"),
        );
        write_source(&src, "harness/mod.rs", "pub fn harness_for() {}\n");
        write_source(&src, "models.rs", "pub enum AgentProvider { Codex }\n");

        let modules = modules_above_harness_for(&src);
        let (path, source) = modules
            .iter()
            .find(|(path, _)| path == "src/app/rpc/dispatch.rs")
            .expect("the scan reaches a nested shipped app module");
        assert_eq!(
            provider_dispatch_offence(source),
            Some("compares against AgentProvider::Codex".to_string()),
            "{path}"
        );
        assert!(modules.iter().any(|(path, _)| path == "src/app/mod.rs"));
    }

    /// Test-only module files and their directory children do not ship, while
    /// a same-named production module below app is still guarded. The harness
    /// implementation and root provider wire table are the only exclusions.
    #[test]
    fn source_guard_excludes_only_sanctioned_and_test_only_subtrees() {
        let fixture = tempfile::tempdir().expect("temp source tree");
        let src = fixture.path().join("src");
        write_source(
            &src,
            "lib.rs",
            "pub mod app;\npub mod harness;\npub mod models;\n#[cfg(test)]\nmod root_tests;\n\
             #[cfg(test)]\nmod shared;\n",
        );
        write_source(&src, "main.rs", "mod shared;\n");
        write_source(&src, "shared.rs", "pub fn shipped() {}\n");
        write_source(
            &src,
            "app/mod.rs",
            "mod models;\n#[cfg(test)]\nmod tests;\n",
        );
        write_source(&src, "app/models.rs", "pub fn shipped() {}\n");
        write_source(&src, "app/tests/mod.rs", "mod support;\n");
        write_source(
            &src,
            "app/tests/support.rs",
            &inside_a_function("let harness = ClaudeHarness;"),
        );
        write_source(&src, "root_tests/mod.rs", "mod support;\n");
        write_source(
            &src,
            "root_tests/support.rs",
            &inside_a_function("let harness = ClaudeHarness;"),
        );
        write_source(&src, "harness/mod.rs", "mod private;\n");
        write_source(
            &src,
            "harness/private.rs",
            &inside_a_function("let harness = ClaudeHarness;"),
        );
        write_source(&src, "models.rs", "pub enum AgentProvider { Codex }\n");

        let paths: Vec<_> = modules_above_harness_for(&src)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert!(paths.iter().any(|path| path == "src/app/models.rs"));
        assert!(paths.iter().any(|path| path == "src/shared.rs"));
        for excluded in [
            "src/models.rs",
            "src/harness/mod.rs",
            "src/harness/private.rs",
            "src/app/tests/mod.rs",
            "src/app/tests/support.rs",
            "src/root_tests/mod.rs",
            "src/root_tests/support.rs",
        ] {
            assert!(!paths.iter().any(|path| path == excluded), "{paths:?}");
        }
    }

    /// The walk that finds opened sessions reads every shipped harness module
    /// and none of the test-only ones, so a session type is found wherever it
    /// is implemented and test scaffolding never widens the ban list.
    #[test]
    fn the_session_walk_reads_shipped_modules_and_skips_test_only_ones() {
        let harness_modules = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("harness");
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let walked: Vec<String> = shipped_rust_sources_under(&source_root, &harness_modules)
            .iter()
            .map(|path| {
                path.strip_prefix(&harness_modules)
                    .expect("a walked path sits under src/harness")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        for shipped in ["adk.rs", "codex_app_server/session.rs"] {
            assert!(walked.iter().any(|path| path == shipped), "{walked:?}");
        }
        for test_only in [
            "stream_fixtures.rs",
            "codex_app_server/fixtures.rs",
            "codex_app_server/tests.rs",
        ] {
            assert!(!walked.iter().any(|path| path == test_only), "{walked:?}");
        }
    }

    /// The ban list the guard reads, the registry that constructs harnesses and
    /// the harness modules that open sessions are one fact, so a provider added
    /// under `harness_for` cannot leave a harness or session type the guard
    /// would let a caller above it name.
    #[test]
    fn the_ban_list_covers_every_harness_harness_for_constructs() {
        let constructed = harnesses_named_by_harness_for();
        assert_eq!(constructed.len(), AgentProvider::ALL.len());
        let opened = sessions_opened_by_the_harness_modules();
        assert!(
            !opened.is_empty(),
            "the walk over src/harness found the sessions its modules open"
        );

        for harness_type in constructed {
            assert!(
                CONCRETE_HARNESS_TYPES.contains(&harness_type.as_str()),
                "harness_for constructs {harness_type} but the guard does not ban it above \
                 harness_for"
            );
        }
        for session_type in opened {
            assert!(
                CONCRETE_HARNESS_TYPES.contains(&session_type.as_str()),
                "a harness module opens {session_type} but the guard does not ban it above \
                 harness_for"
            );
        }
    }
}
