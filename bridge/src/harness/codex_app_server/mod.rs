use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use crate::harness::codex::{self, CodexHarness, CodexMcpConfig, EFFORT_LEVELS};
use crate::harness::{
    installed, AgentSession, Harness, HarnessContext, HarnessError, OpenedSession,
    SessionOpenRequest, SessionOutput, INHERITED_AGENT_MARKERS,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

mod connection;
mod diagnostics;
mod limits;
mod observation;
mod policy;
mod process;
mod protocol;
mod session;
mod state;
mod subagents;
mod translator;

pub struct CodexAppServerHarness;

impl CodexAppServerHarness {
    fn probe_version(binary: &Path) -> Result<String, String> {
        type ProbeCell = Arc<OnceLock<Result<String, String>>>;
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, ProbeCell>>> = OnceLock::new();

        let resolved = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
        let cell = CACHE
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .entry(resolved.clone())
            .or_insert_with(|| Arc::new(OnceLock::new()))
            .clone();
        cell.get_or_init(|| run_version_probe(&resolved)).clone()
    }
}

fn run_version_probe(binary: &Path) -> Result<String, String> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot run {} --version: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version exited with {}",
            binary.display(),
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|error| format!("Codex version output is not UTF-8: {error}"))
}

impl Harness for CodexAppServerHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::CodexAppServer
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn binary(&self) -> &'static str {
        "codex"
    }

    fn models(&self) -> Vec<ModelOption> {
        codex::models()
    }

    fn cli_name(&self) -> &'static str {
        CodexHarness.cli_name()
    }

    fn cli_probe(&self) -> &'static dyn installed::CliProbe {
        CodexHarness.cli_probe()
    }

    fn offer(&self, reading: Option<&installed::CliReading>) -> installed::ModelOffer {
        CodexHarness.offer(reading)
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        &EFFORT_LEVELS
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        CodexHarness.model_args(choice)
    }

    /// `/compact` too: the session hands it to Codex as `thread/compact/start`,
    /// so it arrives bare rather than wrapped as a reviewer turn.
    fn requires_unadorned_command(&self, prompt: &str) -> bool {
        CodexHarness.requires_unadorned_command(prompt) || self.starts_compaction(prompt)
    }

    fn starts_compaction(&self, prompt: &str) -> bool {
        CodexHarness.starts_compaction(prompt)
    }

    fn compacts_on_command(&self) -> bool {
        CodexHarness.compacts_on_command()
    }

    /// `thread/compact/start` takes no focus.
    fn compaction_command(&self, focus: Option<&str>) -> String {
        CodexHarness.compaction_command(focus)
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, HarnessError> {
        let mut spec = HarnessSpec::new("codex")
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("app-server")
            .arg("--stdio");
        if let Some(effort) = &choice.effort {
            spec = spec.arg("--config").arg(format!(
                "model_reasoning_effort={}",
                serde_json::to_string(effort).expect("effort serializes")
            ));
        }
        for override_arg in CodexMcpConfig::new(options, context).overrides() {
            spec = spec.arg("--config").arg(override_arg);
        }
        // The same trust grant the terminal carrier passes: the worktree is
        // one Build minted moments ago, and codex's project trust is keyed by
        // path whichever front end opens it.
        Ok(spec
            .arg("--config")
            .arg(codex::trust_override(&options.cwd)))
    }

    fn open_session(&self, request: SessionOpenRequest) -> Result<OpenedSession, HarnessError> {
        let (session, activity) = session::CodexAppServerSession::spawn(
            &request.spec,
            request.root,
            request.choice,
            request.resume_session_id,
            limits::AppServerLimits::default(),
        )?;
        let surfaces = session.surfaces_changed();
        Ok(OpenedSession {
            session: Arc::new(session),
            output: SessionOutput::reporting(activity, surfaces),
        })
    }

    fn has_terminal(&self) -> bool {
        false
    }

    /// The app-server writes the same dated rollouts under `~/.codex/sessions`
    /// the TUI does (its header says `originator: build_bridge`), so the two
    /// codex carriers answer the transcript questions off one tree — the
    /// same dividend the two claude carriers get from `~/.claude/projects`.
    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        CodexHarness.has_transcript(home, cwd)
    }

    /// A recorded thread id is spent only while its rollout is still on disk.
    /// Before this the trait default said "always", and a stale id cost a
    /// failed `thread/resume` and a dead session instead of the fresh start
    /// the claude carriers get from the same situation.
    fn holds_conversation(&self, home: &Path, cwd: &Path, id: &str) -> bool {
        CodexHarness.holds_conversation(home, cwd, id)
    }
}

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
