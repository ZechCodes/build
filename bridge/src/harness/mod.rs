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
use std::time::Duration;

use portable_pty::PtySize;

use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::{HarnessSpec, PtySession};

pub(crate) mod claude;
pub(crate) mod codex;
mod session;

pub use session::{AgentSession, AgentStatus, HarnessError, SessionOutput, TerminalView, Turn};

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
    /// basement while that agent is still idle, and the spawn decides which
    /// carrier to open. One authority, one answer, so the rail never offers a
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
}

/// Open a live session for `spec`, rooted at `root`, waiting up to
/// `turn_ready_grace` for it to be able to take a turn. Returns the session
/// and its output, subscribed before the first byte can be missed.
///
/// The one place a launch description becomes a running agent. Every spec names
/// a binary today, so every session is a subprocess in a full PTY; a carrier
/// that is not a subprocess is chosen here and nowhere else has to notice.
///
/// The readiness wait is the PTY arm's alone, because readiness is how a
/// *terminal* opens: an interactive TUI paints a banner — or a modal
/// workspace-trust dialog — long before its line editor will accept a turn, so
/// a prompt written on first byte lands in whatever owns the keyboard. A
/// carrier that takes a turn as a value has nothing to wait for. `None` is for
/// a session Build will never hand a turn to (the human's own shell): waiting
/// on a login shell for a signal it may never send would stall the caller for
/// the whole grace.
///
/// The subscribe happens BEFORE that wait, and the order is not incidental: a
/// harness paints its entire startup while readiness is being waited out — and
/// a harness that dies there paints its last words — so a stream subscribed
/// afterwards would open blank on a live agent and lose the epitaph of a dead
/// one.
pub fn open_session(
    spec: &HarnessSpec,
    root: PathBuf,
    size: PtySize,
    turn_ready_grace: Option<Duration>,
) -> Result<(Box<dyn AgentSession>, SessionOutput), HarnessError> {
    let session = PtySession::spawn(spec, Some(root), size)?;
    let output = session.terminal().map(TerminalView::subscribe);
    if let Some(grace) = turn_ready_grace {
        session.ready_within(grace);
    }
    Ok((Box::new(session), output))
}

/// The implementation for `provider`. The only way to reach one.
pub fn harness_for(provider: AgentProvider) -> &'static dyn Harness {
    match provider {
        AgentProvider::Claude => &claude::ClaudeHarness,
        AgentProvider::Codex => &codex::CodexHarness,
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

    /// Every provider Build ships today is a CLI wrapper in a full PTY —
    /// opaque, so every one of them needs the escape hatch. This is the
    /// equivalence that keeps the provider-sourced answer a refactor: the
    /// digest of a not-yet-started agent says exactly what it always did.
    #[test]
    fn every_provider_today_offers_a_terminal() {
        for provider in AgentProvider::ALL {
            assert!(harness_for(provider).has_terminal(), "{provider:?}");
        }
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
        let (session, _output) = open_session(
            &slow_to_open_spec(),
            root.path().to_path_buf(),
            one_pty(),
            Some(Duration::from_secs(5)),
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
            one_pty(),
            None,
        )
        .expect("the session opens");
        let waited = started.elapsed();
        session.end();
        assert!(
            waited < Duration::from_millis(200),
            "opening a session nobody will speak to waited {waited:?} for readiness"
        );
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
