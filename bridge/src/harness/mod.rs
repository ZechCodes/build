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
//! Nothing above these traits matches on a provider. Adding one means adding an
//! `AgentProvider` variant, a module here, and an arm in [`harness_for`]; the
//! compiler finds the rest.

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
mod session;
pub mod shell_tail;
#[cfg(test)]
pub(crate) mod stream_fixtures;
pub mod surfaces;

pub use session::{
    ActivityReport, AgentActivity, AgentSession, AgentStatus, HarnessError, SessionOutput,
    TerminalView, ToolOutcome, Turn,
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
    pub bridge_exe: String,
    /// The socket that session's MCP server is reachable on.
    pub mcp_socket: String,
}

/// Everything Build knows about one coding-agent provider.
pub trait Harness: Send + Sync {
    /// The provider this implementation is reached by.
    fn provider(&self) -> AgentProvider;

    /// What a human sees this provider called.
    fn label(&self) -> &'static str;

    /// The curated model catalog, most capable first.
    fn models(&self) -> Vec<ModelOption>;

    /// Every reasoning effort this provider accepts, across all its models.
    /// A model may still accept only some of them ([`ModelOption::efforts`]).
    fn effort_levels(&self) -> &'static [&'static str];

    /// The argv fragment carrying a model selection. Providers disagree about
    /// reasoning effort in particular: a flag for one, a config override for
    /// another.
    fn model_args(&self, choice: &ModelChoice) -> Vec<String>;

    /// The command that opens an interactive session for `options`.
    ///
    /// The prompt is never part of this: every turn travels through the session
    /// after it is running, never baked into argv.
    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> HarnessSpec;

    /// Open this provider's running session and subscribe to its output before
    /// startup can emit anything.
    ///
    /// Opaque CLI providers share this PTY implementation. Protocol providers
    /// override it so their concrete process and session types remain private
    /// to the provider module.
    fn open_session(&self, request: SessionOpenRequest) -> Result<OpenedSession, HarnessError> {
        open_terminal_session(&request.spec, request.root, request.terminal)
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
    /// `None` by default, which is the answer for a carrier that announces its
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

/// Finds the name a harness gave the conversation a PTY session is having, by
/// watching the harness's own transcript tree — the durable records the resume
/// probe has always read, never the screen.
///
/// The terminal carrier's answer to [`AgentSession::session_id`]: a CLI wrapper
/// announces nothing to Build, but it writes down what it is doing, and where
/// it writes is the same place the resume it performs reads from.
pub trait SessionLocator: Send + Sync {
    /// The id, once exactly one transcript this session could be has appeared.
    /// `None` until then; cached once found, so a locator never changes its
    /// answer and the steady-state cost is a field read.
    fn session_id(&self) -> Option<String>;
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
pub struct TerminalOpenOptions {
    pub size: PtySize,
    pub turn_ready_grace: Option<Duration>,
    pub session_locator: Option<Box<dyn SessionLocator>>,
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
    let session =
        PtySession::spawn(spec, Some(root), options.size)?.named_by(options.session_locator);
    let output = match session.terminal() {
        Some(terminal) => SessionOutput::painting(terminal.subscribe()),
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
/// The two capabilities are alternatives, not extras, and a carrier with
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every provider is reachable and answers for itself — the invariant that
    /// makes `harness_for` the single registry rather than one of several.
    #[test]
    fn every_provider_has_a_harness_that_names_itself() {
        for provider in AgentProvider::ALL {
            let harness = harness_for(provider);
            assert_eq!(harness.provider(), provider);
            assert!(!harness.label().is_empty(), "{provider:?}");
            assert!(!harness.models().is_empty(), "{provider:?}");
            assert!(!harness.effort_levels().is_empty(), "{provider:?}");
        }
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

    /// The alternatives hold their shape: a carrier Build can only see the
    /// outside of has its conversation named FOR it, off the harness's own
    /// transcript tree, and one that announces its own id needs no locator —
    /// two records of one answer, free to disagree, is the shape this spec
    /// rejects everywhere else.
    #[test]
    fn a_locator_is_offered_exactly_where_a_terminal_is() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp worktree");
        for provider in AgentProvider::ALL {
            let harness = harness_for(provider);
            assert_eq!(
                harness.session_locator(home.path(), cwd.path()).is_some(),
                harness.has_terminal(),
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
        for provider in AgentProvider::ALL
            .into_iter()
            .filter(|provider| *provider != AgentProvider::CodexAppServer)
        {
            assert!(
                !harness_for(provider).holds_conversation(home.path(), cwd.path(), "sess-1"),
                "{provider:?}"
            );
        }
        assert!(
            harness_for(AgentProvider::CodexAppServer).holds_conversation(
                home.path(),
                cwd.path(),
                "sess-1"
            ),
            "app-server conversation ids are verified by exact resume, not a transcript guess"
        );

        // Both claude carriers write and read the ONE tree, so an id captured
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
    /// where the carrier is chosen rather than at the caller.
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
                session_locator: None,
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
                session_locator: None,
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

    /// The locator travels with the carrier that has a use for it: a terminal
    /// hands its answers back as its own, and a session Build opens with none
    /// — the human's shell, and every carrier that names its conversation
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
                session_locator: Some(Box::new(Says("sess-located"))),
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
                session_locator: None,
            },
        )
        .expect("the session opens");
        assert_eq!(unnamed.session.session_id(), None);
        unnamed.session.end();
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
                session_locator: None,
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
            ) -> HarnessSpec {
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
    /// The part of a module that ships: everything before its inline test
    /// module, so scaffolding that drives one carrier on purpose is not read as
    /// a production dispatch.
    fn production_source(source: &str) -> &str {
        let mut consumed = 0;
        let mut lines = source.lines().peekable();
        while let Some(line) = lines.next() {
            let opens_a_test_module = line == "#[cfg(test)]"
                && lines
                    .peek()
                    .is_some_and(|next| next.starts_with("mod ") && next.ends_with(" {"));
            if opens_a_test_module {
                return &source[..consumed];
            }
            consumed += line.len() + 1;
        }
        source
    }

    /// The implementation types no module above `harness_for` may name: the
    /// harnesses `harness_for` constructs, and the sessions they open. Kept
    /// honest against its owner by
    /// [`the_ban_list_covers_every_harness_harness_for_constructs`].
    const CONCRETE_HARNESS_TYPES: [&str; 6] = [
        "AdkHarness",
        "AdkSession",
        "ClaudeHarness",
        "CodexHarness",
        "CodexAppServerHarness",
        "CodexAppServerSession",
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

    /// The session types the harness modules open, read out of every shipped
    /// `impl AgentSession for` under `src/harness` so the guard's ban list is
    /// checked against the modules that own those types rather than against a
    /// second hand-kept copy. `PtySession` lives in `src/pty.rs`, outside the
    /// walk, and stays out of the list.
    fn sessions_opened_by_the_harness_modules() -> Vec<String> {
        const SESSION_IMPL: &str = "impl AgentSession for ";

        let harness_modules = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("harness");
        rust_sources_under(&harness_modules)
            .iter()
            .flat_map(|path| {
                let source = std::fs::read_to_string(path).expect("a readable harness module");
                production_source(&source)
                    .lines()
                    .filter_map(|line| line.strip_prefix(SESSION_IMPL))
                    .map(|implementor| {
                        implementor
                            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .next()
                            .expect("an impl names its type")
                            .to_string()
                    })
                    .collect::<Vec<String>>()
            })
            .collect()
    }

    /// Every Rust source directly inside `directory`, without descending.
    fn rust_sources_directly_in(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(directory)
            .expect("a readable source directory")
            .map(|entry| entry.expect("a readable source entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .collect()
    }

    /// Every Rust source inside `directory` and the directories under it.
    fn rust_sources_under(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
        let nested = std::fs::read_dir(directory)
            .expect("a readable source directory")
            .map(|entry| entry.expect("a readable source entry").path())
            .filter(|path| path.is_dir())
            .flat_map(|subdirectory| rust_sources_under(&subdirectory));
        rust_sources_directly_in(directory)
            .into_iter()
            .chain(nested)
            .collect()
    }

    const PROVIDER_PATH: &str = "AgentProvider::";

    /// The variant a provider mention at `mention` names, and the code that
    /// follows it once closing delimiters and whitespace are skipped.
    fn variant_and_continuation(shipped: &str, mention: usize) -> (&str, &str) {
        let variant_onward = &shipped[mention + PROVIDER_PATH.len()..];
        let after_variant =
            variant_onward.trim_start_matches(|c: char| c.is_alphanumeric() || c == '_');
        let variant = &variant_onward[..variant_onward.len() - after_variant.len()];
        (
            variant,
            after_variant.trim_start_matches([')', ']', ' ', '\n']),
        )
    }

    /// Whether the mention sits in the pattern of the nearest `let` — between
    /// the keyword and its `=` — which is where `if let`, `while let` and
    /// let-else all place the value they dispatch on.
    fn sits_in_a_let_pattern(preceding: &str) -> bool {
        const BINDING: &str = "let ";

        preceding
            .rfind(BINDING)
            .is_some_and(|binding| !preceding[binding + BINDING.len()..].contains(['=', ';']))
    }

    /// Whether the mention sits in the pattern of a still-open `matches!` —
    /// after its first comma — wherever rustfmt has broken the call's lines.
    fn sits_in_a_matches_pattern(preceding: &str) -> bool {
        const MACRO: &str = "matches!(";

        preceding.rfind(MACRO).is_some_and(|call| {
            let arguments = &preceding[call + MACRO.len()..];
            let still_open = arguments
                .chars()
                .try_fold(1usize, |depth, character| match character {
                    '(' => Some(depth + 1),
                    ')' => (depth > 1).then_some(depth - 1),
                    _ => Some(depth),
                })
                .is_some();
            still_open && arguments.contains(',')
        })
    }

    /// Whether the provider mention at `mention` tests it for equality or binds
    /// it as a pattern rather than matching on it — the same second dispatch
    /// worn as an `if`, a `while` or a `let`.
    fn compares_against_a_provider(shipped: &str, mention: usize) -> bool {
        let preceding = &shipped[..mention];
        let (_, continuation) = variant_and_continuation(shipped, mention);
        let compared_by_operator = preceding.trim_end().ends_with("==")
            || preceding.trim_end().ends_with("!=")
            || continuation.starts_with("==")
            || continuation.starts_with("!=");
        compared_by_operator
            || sits_in_a_let_pattern(preceding)
            || sits_in_a_matches_pattern(preceding)
    }

    /// What in `source` claims a provider dispatch of its own, if anything.
    fn provider_dispatch_offence(source: &str) -> Option<String> {
        let shipped = production_source(source);
        for (mention, _) in shipped.match_indices(PROVIDER_PATH) {
            let (variant, arm_head) = variant_and_continuation(shipped, mention);
            let guard_reaches_an_arrow = arm_head.starts_with("if ")
                && arm_head
                    .lines()
                    .next()
                    .is_some_and(|first| first.contains("=>"));
            if arm_head.starts_with("=>") || guard_reaches_an_arrow {
                return Some(format!("matches on {PROVIDER_PATH}{variant}"));
            }
            if compares_against_a_provider(shipped, mention) {
                return Some(format!("compares against {PROVIDER_PATH}{variant}"));
            }
        }

        CONCRETE_HARNESS_TYPES
            .into_iter()
            .find(|harness_type| shipped.contains(harness_type))
            .map(|harness_type| format!("names {harness_type}"))
    }

    /// Every module that sits above `harness_for`: the crate's top-level
    /// sources, minus `models.rs`, whose wire table is the one sanctioned
    /// per-provider list. Read from disk rather than named one by one so a
    /// module added later is guarded without anyone remembering to list it.
    fn modules_above_harness_for() -> Vec<(String, String)> {
        const SANCTIONED_PROVIDER_TABLE: &str = "models.rs";

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut modules: Vec<(String, String)> = rust_sources_directly_in(&src)
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name != SANCTIONED_PROVIDER_TABLE)
            })
            .map(|path| {
                let source = std::fs::read_to_string(&path).expect("a readable src module");
                (
                    format!(
                        "src/{}",
                        path.file_name().expect("a named module").to_string_lossy()
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
        let modules = modules_above_harness_for();
        assert!(
            modules.iter().any(|(path, _)| path == "src/app.rs"),
            "the scan reached the crate's own modules, so a green run means something"
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
        const SECOND_DISPATCHES: [&str; 13] = [
            "match provider {\n    AgentProvider::Codex => launch(),\n}",
            "match named {\n    Some(AgentProvider::Codex) => launch(),\n}",
            "match provider {\n    AgentProvider::Codex if resume => launch(),\n}",
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
                provider_dispatch_offence(source).is_some(),
                "a second dispatch went unnoticed: {source}"
            );
        }

        let single_dispatch = "let provider = AgentProvider::from_wire(id)?;\nlet opened = \
                              harness_for(provider).open_session(request)?;\nmatch named {\n    \
                              Some(agent) if AgentProvider::from_wire(agent).is_some() => \
                              Err(already_named()),\n}";
        assert_eq!(provider_dispatch_offence(single_dispatch), None);

        let compared_only_by_value = "if choice.model == catalogued.model { keep() }";
        assert_eq!(provider_dispatch_offence(compared_only_by_value), None);

        let constructed_inside_a_binding = "if let Some(found) = catalog(AgentProvider::Claude) \
                                            { keep() }";
        assert_eq!(
            provider_dispatch_offence(constructed_inside_a_binding),
            None
        );

        let checked_after_an_earlier_matches = "let quiet = matches!(status, Idle);\nlet \
                                                opened = harness_for(AgentProvider::Codex);";
        assert_eq!(
            provider_dispatch_offence(checked_after_an_earlier_matches),
            None
        );

        let dispatch_only_in_tests = format!(
            "pub fn run() {{}}\n#[cfg(test)]\nmod tests {{\n    {}\n}}\n",
            "let harness = ClaudeHarness;"
        );
        assert_eq!(provider_dispatch_offence(&dispatch_only_in_tests), None);
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
