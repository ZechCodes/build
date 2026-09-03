use std::path::Path;
use std::sync::Arc;

use crate::harness::codex::{self, CodexHarness, CodexMcpConfig, EFFORT_LEVELS};
use crate::harness::{
    Harness, HarnessContext, HarnessError, OpenedSession, SessionOpenRequest, SessionOutput,
    INHERITED_AGENT_MARKERS,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

mod connection;
mod limits;
mod policy;
mod process;
mod protocol;
mod session;
mod state;
mod translator;

pub struct CodexAppServerHarness;

impl Harness for CodexAppServerHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::CodexAppServer
    }

    fn label(&self) -> &'static str {
        "Codex"
    }

    fn models(&self) -> Vec<ModelOption> {
        codex::models()
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        &EFFORT_LEVELS
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        CodexHarness.model_args(choice)
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> HarnessSpec {
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
        spec
    }

    fn open_session(&self, request: SessionOpenRequest) -> Result<OpenedSession, HarnessError> {
        let (session, activity) = session::CodexAppServerSession::spawn(
            &request.spec,
            request.root,
            request.choice,
            request.resume_session_id,
            limits::AppServerLimits::default(),
        )?;
        Ok(OpenedSession {
            session: Arc::new(session),
            output: SessionOutput::reporting(activity),
        })
    }

    fn has_terminal(&self) -> bool {
        false
    }

    fn has_transcript(&self, _home: &Path, _cwd: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;
