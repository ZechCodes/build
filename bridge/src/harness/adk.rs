//! Claude Code's headless stream-JSON provider.

use crate::harness::claude::ClaudeHarness;
use crate::harness::{
    AgentSession, Harness, HarnessContext, HarnessError, OpenedSession, SessionLocator,
    SessionOpenRequest, SessionOutput, INHERITED_AGENT_MARKERS,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

/// A no-op hook makes Claude expose the beginning of compaction on its
/// stream-json protocol. `compact_boundary` is the matching completion event.
const COMPACTION_HOOK_SETTINGS: &str =
    r#"{"hooks":{"PreCompact":[{"hooks":[{"type":"command","command":"true"}]}]}}"#;
use std::path::Path;
use std::sync::Arc;

mod activity;
#[cfg(test)]
pub(crate) mod fake;
mod protocol;
mod reader;
mod session;
#[cfg(test)]
mod tests;
mod translation;

#[cfg(test)]
pub(crate) use activity::reports_minted_by;
#[cfg(test)]
pub(crate) use protocol::ACTIVITY_TEXT_LIMIT;
pub(crate) use protocol::TOOL_SUMMARY_LIMIT;
pub use session::AdkSession;
#[cfg(test)]
pub(crate) use translation::tool_result_text;
pub(crate) use translation::{
    bounded_activity_text, one_line, task_status_failed, task_status_is_terminal,
};

pub struct AdkHarness;

impl Harness for AdkHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::ClaudeAdk
    }

    /// The default provider of this CLI, so it owns the plain name the human
    /// knows. The terminal provider beside it is "Claude Code TUI" — the
    /// difference a human can see, never the word the code uses for it.
    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn binary(&self) -> &'static str {
        "claude"
    }

    fn models(&self) -> Vec<ModelOption> {
        ClaudeHarness.models()
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        ClaudeHarness.effort_levels()
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        ClaudeHarness.model_args(choice)
    }

    fn requires_unadorned_command(&self, prompt: &str) -> bool {
        ClaudeHarness.requires_unadorned_command(prompt)
    }

    fn compacts_on_command(&self) -> bool {
        ClaudeHarness.compacts_on_command()
    }

    /// The interactive argv with the TUI swapped for the protocol, and nothing
    /// else moved.
    ///
    /// The MCP half is identical on purpose — the same per-agent
    /// `--mcp-config`, the same `--strict-mcp-config`, the same
    /// `BRIDGE_MCP_SOCKET` / `BRIDGE_MCP_TOKEN` — because
    /// `post_thread_message` and `search_conversation`
    /// arrive over the same unix socket whichever provider is running. No
    /// settle window and no submit delay: those are how a prompt is typed into
    /// a line editor, and this harness is handed a turn as a value.
    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, HarnessError> {
        let mut spec = HarnessSpec::new("claude")
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("-p")
            .arg("--input-format")
            .arg("stream-json")
            .arg("--output-format")
            .arg("stream-json")
            // Without it the child emits only the final result of each turn,
            // and the conversation would learn what the agent did after it had
            // finished doing it.
            .arg("--verbose")
            .arg("--settings")
            .arg(COMPACTION_HOOK_SETTINGS)
            .for_agent(&options.owner_id)
            .arg("--mcp-config")
            .arg(options.mcp_config())
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions");
        // The two are alternatives and never both: `--resume` names the exact
        // conversation this agent was having, `--continue` guesses the newest
        // one in the checkout, and passing both would ask for two different
        // conversations. The name wins where there is one; the guess is what
        // answers for a session that died before it could say its own.
        match options.resume_session_id.as_deref() {
            Some(named) => spec = spec.arg("--resume").arg(named),
            None if options.continue_session => spec = spec.arg("--continue"),
            None => {}
        }
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        Ok(spec
            .env(
                "BRIDGE_MCP_SOCKET",
                context.mcp_socket.to_string_lossy().into_owned(),
            )
            .env("BRIDGE_MCP_TOKEN", &options.mcp_session_token))
    }

    fn open_session(&self, request: SessionOpenRequest) -> Result<OpenedSession, HarnessError> {
        let (session, activity) =
            AdkSession::spawn(&request.spec, Some(request.root), &request.choice)?;
        let surfaces = session.surfaces_changed();
        Ok(OpenedSession {
            session: Arc::new(session),
            output: SessionOutput::reporting(activity, surfaces),
        })
    }

    /// No terminal, and this is the first provider to say so. A session that
    /// reports its own reasoning and tool calls is not opaque, so there is
    /// nothing for a human to escape to — the rail offers no TUI button and the
    /// terminal verbs refuse.
    fn has_terminal(&self) -> bool {
        false
    }

    /// The trust registry is the CLI's, not the TUI's: a headless session in an
    /// untrusted directory is refused the same way, and the refusal is worse
    /// here because there is no screen to show it on.
    fn prepare_workspace(&self, cwd: &Path) {
        ClaudeHarness.prepare_workspace(cwd)
    }

    /// Headless sessions write the same `~/.claude/projects/**/*.jsonl`
    /// transcripts the TUI does, so the resume question has the same answer —
    /// and a worktree the human left a conversation in is picked up whichever
    /// provider ran there.
    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        ClaudeHarness.has_transcript(home, cwd)
    }

    /// The same transcripts, so the same answer — and this is the dividend: an
    /// id captured under the TUI provider verifies here and resumes here, and
    /// one captured here resumes there. Both write that tree and both spend
    /// `--resume`, so a provider swap between the two claude providers keeps the
    /// exact conversation.
    fn holds_conversation(&self, home: &Path, cwd: &Path, id: &str) -> bool {
        ClaudeHarness.holds_conversation(home, cwd, id)
    }

    /// The same tree and the same `--resume`, so the same shadow.
    fn set_aside_shadowing_copy(
        &self,
        home: &Path,
        cwd: &Path,
        root: &Path,
        id: &str,
    ) -> std::io::Result<Option<std::path::PathBuf>> {
        ClaudeHarness.set_aside_shadowing_copy(home, cwd, root, id)
    }

    /// This protocol session reads its own id off the child's `init` line, so a
    /// transcript locator beside it would be a second answer free to disagree.
    fn session_locator(&self, home: &Path, cwd: &Path) -> Option<Box<dyn SessionLocator>> {
        let _ = (home, cwd);
        None
    }
}
