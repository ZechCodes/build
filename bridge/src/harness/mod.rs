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

    /// Whether a session opened for this provider offers a terminal.
    ///
    /// The provider answers because it is the only authority that exists BOTH
    /// before and after a spawn: the rail decides whether to offer an agent a
    /// basement while that agent is still idle, and the spawn decides whether
    /// to open a terminal or a session protocol. One authority, one answer, so the rail never offers a
    /// TUI button the spawn would then refuse.
    ///
    /// True by default, and true for every provider today: a CLI wrapper is
    /// opaque — Build sees what it launched and what the agent reported, and
    /// nothing in between — so it needs the escape hatch. A harness that
    /// reports its own reasoning and tool calls has nothing to escape to.
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

/// Finds the name a harness gave the conversation a PTY session is having, by
/// watching the harness's own transcript tree — the durable records the resume
/// probe has always read, never the screen.
///
/// A terminal's answer to [`AgentSession::session_id`]: a CLI wrapper
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

/// How a spawn talks to its agent — a terminal or a session protocol — and
/// what each needs to know.
///
/// The provider decides ([`Harness::has_terminal`]) and this is the shape that
/// decision travels in, so the two arms carry only what their own kind has an
/// answer for: a grid and a readiness wait belong to a terminal, and a session
/// protocol has neither.
pub enum AgentIo {
    /// A full PTY around an opaque CLI wrapper.
    ///
    /// `turn_ready_grace` is how long to wait for the harness to be able to
    /// take a turn, and `None` is for a session Build will never hand one to
    /// (the human's own shell): waiting on a login shell for a signal it may
    /// never send would stall the caller for the whole grace.
    ///
    /// `locator` is how a terminal answers
    /// [`AgentSession::session_id`] — the provider's watcher over its own
    /// transcript tree, built by the caller BEFORE the child exists so the
    /// child's own record can be told from what was already there. `None` for
    /// the human's shell, which is having no conversation to name.
    Terminal {
        size: PtySize,
        turn_ready_grace: Option<Duration>,
        locator: Option<Box<dyn SessionLocator>>,
    },
    /// A session protocol over piped stdio. There is no readiness dance: a
    /// turn is a value, and the child says for itself when it can take one.
    Protocol,
}

/// Open a live session for `spec`, rooted at `root`, as the terminal or the
/// session protocol the provider chose. Returns the session and its output, subscribed before its
/// first word can be missed.
///
/// The session comes back behind an [`Arc`] because the daemon keeps it inside
/// the state it locks, and hands turns to it with that lock RELEASED — see
/// [`AgentSession::send_turn`]. A shared handle is what lets a caller take the
/// session out of the registry without holding the registry open across the
/// turn.
///
/// The one place a launch description becomes a running agent, and the only
/// place a terminal and a session protocol are told apart: above here a session is a session.
///
/// The readiness wait is the PTY arm's alone, because readiness is how a
/// *terminal* opens: an interactive TUI paints a banner — or a modal
/// workspace-trust dialog — long before its line editor will accept a turn, so
/// a prompt written on first byte lands in whatever owns the keyboard. A
/// session protocol that takes a turn as a value has nothing to wait for.
///
/// The subscribe happens BEFORE that wait, and the order is not incidental: a
/// harness paints its entire startup while readiness is being waited out — and
/// a harness that dies there paints its last words — so a stream subscribed
/// afterwards would open blank on a live agent and lose the epitaph of a dead
/// one. The protocol arm subscribes inside its own spawn for the same reason.
pub fn open_session(
    spec: &HarnessSpec,
    root: PathBuf,
    io: AgentIo,
) -> Result<(Arc<dyn AgentSession>, SessionOutput), HarnessError> {
    let (session, output): (Arc<dyn AgentSession>, SessionOutput) = match io {
        AgentIo::Terminal {
            size,
            turn_ready_grace,
            locator,
        } => {
            let session = PtySession::spawn(spec, Some(root), size)?.named_by(locator);
            let output = match session.terminal() {
                Some(terminal) => SessionOutput::painting(terminal.subscribe()),
                None => SessionOutput::silent(),
            };
            if let Some(grace) = turn_ready_grace {
                session.ready_within(grace);
            }
            (Arc::new(session), output)
        }
        AgentIo::Protocol => {
            let (session, activity) = adk::AdkSession::spawn(spec, Some(root))?;
            (Arc::new(session), SessionOutput::reporting(activity))
        }
    };
    refuse_a_session_nobody_can_watch(session.as_ref())?;
    Ok((session, output))
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

    /// The terminal is a capability, and exactly the opaque CLI wrappers have
    /// it: Build sees what it launched and what they reported, and nothing in
    /// between, so the human needs the escape hatch. The headless provider
    /// reports its own reasoning and tool calls, so it has nothing to escape
    /// to — and this is the answer the rail and the spawn BOTH read, which is
    /// what keeps the rail from offering a button the spawn would refuse.
    #[test]
    fn only_the_opaque_cli_wrappers_offer_a_terminal() {
        for provider in [AgentProvider::Claude, AgentProvider::Codex] {
            assert!(harness_for(provider).has_terminal(), "{provider:?}");
        }
        assert!(!harness_for(AgentProvider::ClaudeAdk).has_terminal());
    }

    /// The alternatives hold their shape: a terminal, which Build can only see
    /// the outside of, has its conversation named FOR it, off the harness's own
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
        for provider in AgentProvider::ALL {
            assert!(
                !harness_for(provider).holds_conversation(home.path(), cwd.path(), "sess-1"),
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
        let (session, _output) = open_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            AgentIo::Terminal {
                size: one_pty(),
                turn_ready_grace: Some(Duration::from_secs(5)),
                locator: None,
            },
        )
        .expect("the session opens");
        let waited = started.elapsed();
        session.end();
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
        let (session, _output) = open_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            AgentIo::Terminal {
                size: one_pty(),
                turn_ready_grace: None,
                locator: None,
            },
        )
        .expect("the session opens");
        let waited = started.elapsed();
        session.end();
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
        let (named, _output) = open_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            AgentIo::Terminal {
                size: one_pty(),
                turn_ready_grace: None,
                locator: Some(Box::new(Says("sess-located"))),
            },
        )
        .expect("the session opens");
        assert_eq!(named.session_id().as_deref(), Some("sess-located"));
        named.end();

        let (unnamed, _output) = open_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            AgentIo::Terminal {
                size: one_pty(),
                turn_ready_grace: None,
                locator: None,
            },
        )
        .expect("the session opens");
        assert_eq!(unnamed.session_id(), None);
        unnamed.end();
    }

    /// A fake stream-json harness: it announces its session, then sits with its
    /// stdin open the way the real one does between turns.
    fn fake_protocol_spec() -> HarnessSpec {
        HarnessSpec::new("sh").arg("-c").arg(
            "printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s-1\"}'; \
             cat >/dev/null",
        )
    }

    /// The choice between a terminal and a session protocol, made in the one
    /// place it is made: a harness with no
    /// terminal opens a session protocol, and what comes back offers the
    /// alternative capability instead of an empty one.
    #[test]
    fn a_harness_with_no_terminal_opens_a_session_protocol_that_reports_itself() {
        let root = tempfile::tempdir().expect("temp worktree");
        let (session, output) = open_session(
            &fake_protocol_spec(),
            root.path().to_path_buf(),
            AgentIo::Protocol,
        )
        .expect("the session opens");

        assert!(
            session.terminal().is_none(),
            "a session protocol has nothing to escape to"
        );
        assert!(output.bytes.is_none(), "and nothing to paint into a grid");
        assert!(
            output.activity.is_some(),
            "what it has instead is its own account of its work"
        );
        session.end();
    }

    /// A session that offers NEITHER capability is refused rather than opened.
    ///
    /// Not because Build could not watch it work — because the death rites hang
    /// off a stream closing. A session with no stream has no close to hang them
    /// on, so its tab would keep reading as live and its conversation would
    /// stay in session until the idle sweep explained the exit as silence,
    /// minutes later.
    #[test]
    fn a_session_offering_neither_capability_is_refused() {
        struct MuteSession;
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
            fn end(&self) {}
            fn backdate_last_output(&self, _ago: Duration) {}
        }

        let refusal = refuse_a_session_nobody_can_watch(&MuteSession)
            .expect_err("a session nobody can watch is not opened");
        assert!(
            refusal
                .to_string()
                .contains("neither a terminal nor an activity stream"),
            "the refusal says what is missing: {refusal}"
        );

        let root = tempfile::tempdir().expect("temp worktree");
        let (session, _output) = open_session(
            &fake_protocol_spec(),
            root.path().to_path_buf(),
            AgentIo::Protocol,
        )
        .expect("a session protocol that reports itself opens");
        assert!(refuse_a_session_nobody_can_watch(session.as_ref()).is_ok());
        session.end();
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
}
