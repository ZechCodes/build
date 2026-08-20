//! Codex CLI.
//!
//! Everything this provider is told arrives as `--config` overrides on the
//! command that starts the session: the MCP server to run, the tools that
//! server is allowed to expose, the trust grant for the worktree, and the paste
//! handling Build's prompt delivery depends on. Nothing is written outside the
//! process, which is why this harness needs no workspace preparation.

use std::path::Path;

use crate::harness::{
    Harness, HarnessContext, INHERITED_AGENT_MARKERS, REAL_TUI_SETTLE, REAL_TUI_SUBMIT_DELAY,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

/// Reasoning-effort levels codex accepts, across all its models.
pub const EFFORT_LEVELS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

pub struct CodexHarness;

impl Harness for CodexHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::Codex
    }

    fn label(&self) -> &'static str {
        "Codex CLI"
    }

    /// The curated catalog, most capable first. (cached: 2026-07)
    ///
    /// Codex has an experimental debug catalog command, but Build cannot assume
    /// every installed CLI version exposes it; shipping the catalog keeps the
    /// web contract deterministic.
    fn models(&self) -> Vec<ModelOption> {
        const THROUGH_XHIGH: &[&str] = &["low", "medium", "high", "xhigh"];
        const THROUGH_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];
        vec![
            ModelOption {
                id: "gpt-5.6-sol",
                label: "GPT-5.6-Sol",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "gpt-5.6-terra",
                label: "GPT-5.6-Terra",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "gpt-5.6-luna",
                label: "GPT-5.6-Luna",
                supports_effort: true,
                efforts: THROUGH_MAX,
            },
            ModelOption {
                id: "gpt-5.5",
                label: "GPT-5.5",
                supports_effort: true,
                efforts: THROUGH_XHIGH,
            },
            ModelOption {
                id: "gpt-5.2",
                label: "GPT-5.2",
                supports_effort: true,
                efforts: THROUGH_XHIGH,
            },
        ]
    }

    fn effort_levels(&self) -> &'static [&'static str] {
        &EFFORT_LEVELS
    }

    fn model_args(&self, choice: &ModelChoice) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = &choice.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(effort) = &choice.effort {
            args.push("--config".to_string());
            args.push(format!(
                "model_reasoning_effort={}",
                serde_json::to_string(effort).expect("effort serializes")
            ));
        }
        args
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> HarnessSpec {
        let mut spec = HarnessSpec::new("codex")
            .settle(REAL_TUI_SETTLE)
            .submit_delay(REAL_TUI_SUBMIT_DELAY)
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("--dangerously-bypass-approvals-and-sandbox");
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        let mcp_args = serde_json::to_string(&vec!["mcp", "--task", options.owner_id.as_str()])
            .expect("MCP args serialize");
        for override_arg in [
            format!(
                "mcp_servers.build.command={}",
                serde_json::to_string(&context.bridge_exe).expect("path serializes")
            ),
            format!("mcp_servers.build.args={mcp_args}"),
            format!(
                "mcp_servers.build.env.BRIDGE_MCP_SOCKET={}",
                serde_json::to_string(&context.mcp_socket).expect("socket serializes")
            ),
            format!(
                "mcp_servers.build.env.BRIDGE_MCP_TOKEN={}",
                serde_json::to_string(&options.mcp_session_token).expect("MCP token serializes")
            ),
            format!(
                "projects.{}.trust_level=\"trusted\"",
                serde_json::to_string(&options.cwd.to_string_lossy())
                    .expect("worktree path serializes")
            ),
            "mcp_servers.build.required=true".to_string(),
            // The tools this session's surface actually has. A router
            // allow-listed for a coding agent's tools would be a session with
            // nothing it can call.
            format!(
                "mcp_servers.build.enabled_tools={}",
                serde_json::to_string(&mcp_tool_names(&options.owner_id))
                    .expect("tool names serialize")
            ),
            "mcp_servers.build.default_tools_approval_mode=\"approve\"".to_string(),
            // Build writes prompt bytes and Enter back-to-back. Codex's
            // fallback detector otherwise classifies that stream as a paste
            // burst and turns Enter into a newline, so the prompt remains
            // visible but unsent. This PTY advertises and frames real pastes
            // explicitly; the fallback is unnecessary.
            "disable_paste_burst=true".to_string(),
        ] {
            spec = spec.arg("--config").arg(override_arg);
        }
        if options.continue_session {
            spec = spec.arg("resume").arg("--last");
        }
        spec
    }

    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        transcript_exists(&home.join(".codex/sessions"), cwd)
    }
}

/// The tools a session owned by `owner_id` may call. Codex wants the allow-list
/// up front, in the argv that starts the session; the surface the id names is
/// the one source for it.
fn mcp_tool_names(owner_id: &str) -> Vec<&'static str> {
    match crate::mcp::McpSurface::for_owner(owner_id) {
        crate::mcp::McpSurface::Coding => vec![
            "read_unread_messages",
            "post_thread_message",
            "done",
            "search_conversation",
        ],
        crate::mcp::McpSurface::Router => vec![
            "list_projects",
            "list_work",
            "read_conversation",
            "create_issue",
            "dispatch_branch",
            "ask_user",
            "done",
        ],
    }
}

/// Codex stores dated JSONL rollouts. The first line is session metadata with
/// the canonical cwd; scanning that small header is enough to decide whether
/// `codex resume --last` has a cwd-scoped conversation to continue.
pub(crate) fn transcript_exists(root: &Path, cwd: &Path) -> bool {
    use std::io::BufRead;

    let wanted = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Never follow a user-created symlink loop while looking through
            // Codex's dated session directories.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            if !kind.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut first = String::new();
            if std::io::BufReader::new(file).read_line(&mut first).is_err() {
                continue;
            }
            let session_cwd = serde_json::from_str::<serde_json::Value>(&first)
                .ok()
                .and_then(|meta| {
                    meta.pointer("/payload/cwd")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                });
            if session_cwd.is_some_and(|path| {
                let path = std::path::PathBuf::from(path);
                std::fs::canonicalize(&path).unwrap_or(path) == wanted
            }) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_transcript_probe_reads_nested_session_metadata_by_cwd() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let dated = root.path().join("sessions/2026/07/22");
        std::fs::create_dir_all(&dated).unwrap();
        std::fs::write(
            dated.join("rollout.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{}}}}}\n{{}}\n",
                serde_json::to_string(&cwd.display().to_string()).unwrap()
            ),
        )
        .unwrap();

        assert!(transcript_exists(&root.path().join("sessions"), &cwd));
        assert!(!transcript_exists(
            &root.path().join("sessions"),
            &root.path().join("other")
        ));
    }

    /// The trait method and the free function have to agree about where codex
    /// keeps rollouts, or a resume decision is made against the wrong tree.
    #[test]
    fn the_probe_looks_under_the_homes_codex_sessions_dir() {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        assert!(!CodexHarness.has_transcript(home.path(), &cwd));

        let dated = home.path().join(".codex/sessions/2026/07/22");
        std::fs::create_dir_all(&dated).unwrap();
        std::fs::write(
            dated.join("rollout.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{}}}}}\n",
                serde_json::to_string(&cwd.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        assert!(CodexHarness.has_transcript(home.path(), &cwd));
    }
}
