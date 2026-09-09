use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use crate::harness::codex::{self, CodexHarness, CodexMcpConfig, EFFORT_LEVELS};
use crate::harness::{
    AgentSession, Harness, HarnessContext, HarnessError, OpenedSession, SessionOpenRequest,
    SessionOutput, INHERITED_AGENT_MARKERS,
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
        Ok(spec)
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

    fn has_transcript(&self, _home: &Path, _cwd: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;
