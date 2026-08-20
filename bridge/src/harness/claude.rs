//! Claude Code.
//!
//! Trust is the shape that makes this provider different from a purely
//! argv-configured one: claude keeps workspace trust in a shared registry under
//! the home directory, so a fresh worktree has to be recorded there BEFORE a
//! session opens in it. That is the one place Build writes outside its own
//! tree, and it only ever adds the flag for a path Build itself created.

use std::path::Path;

use serde_json::{json, Value};

use crate::harness::{
    Harness, HarnessContext, INHERITED_AGENT_MARKERS, REAL_TUI_SETTLE, REAL_TUI_SUBMIT_DELAY,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

/// Reasoning-effort levels claude accepts (`claude --effort`).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub struct ClaudeHarness;

impl Harness for ClaudeHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::Claude
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    /// The curated catalog, most capable first. (cached: 2026-07)
    fn models(&self) -> Vec<ModelOption> {
        vec![
            ModelOption {
                id: "claude-fable-5",
                label: "Claude Fable 5",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "claude-opus-4-8",
                label: "Claude Opus 4.8",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "claude-sonnet-5",
                label: "Claude Sonnet 5",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "claude-sonnet-4-6",
                label: "Claude Sonnet 4.6",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
            },
            ModelOption {
                id: "claude-haiku-4-5",
                label: "Claude Haiku 4.5",
                supports_effort: false,
                efforts: &[],
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
            args.push("--effort".to_string());
            args.push(effort.clone());
        }
        args
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> HarnessSpec {
        let mut spec = HarnessSpec::new("claude")
            .settle(REAL_TUI_SETTLE)
            .submit_delay(REAL_TUI_SUBMIT_DELAY)
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("--mcp-config")
            .arg(crate::orchestrator::mcp_config_path(&options.owner_id))
            .arg("--strict-mcp-config")
            .arg("--dangerously-skip-permissions");
        if options.continue_session {
            spec = spec.arg("--continue");
        }
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        spec.env("BRIDGE_MCP_SOCKET", &context.mcp_socket)
            .env("BRIDGE_MCP_TOKEN", &options.mcp_session_token)
    }

    fn prepare_workspace(&self, cwd: &Path) {
        pre_trust_worktree(cwd);
    }

    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        transcript_exists(&home.join(".claude/projects"), cwd)
    }
}

/// The transcript directory name Claude Code uses for a cwd under
/// `~/.claude/projects/`: the absolute path with `/` and `.` replaced by `-`.
pub(crate) fn encode_project_dir(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// True iff the encoded directory exists under `root` and holds at least one
/// `.jsonl` transcript.
pub(crate) fn transcript_exists(root: &Path, cwd: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join(encode_project_dir(cwd))) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
}

/// Record a Build-created worktree as trusted in claude's project registry, so
/// the interactive session skips its workspace-trust dialog.
///
/// The dialog fires for any directory claude has not seen, and it owns the
/// keyboard until answered — so the prompt Build injects lands in the dialog,
/// the trailing Enter answers it, the agent receives nothing, and the run sits
/// in `building` until the idle sweep demotes it.
///
/// Best-effort by design: claude rewrites this file too, so an interleaved
/// write could drop the insert. Failing the spawn over that would be worse than
/// the dialog it prevents, so every error here is swallowed — the caller still
/// gets a session, and the worst case is the dialog.
fn pre_trust_worktree(cwd: &Path) {
    let Some(config) = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join(".claude.json"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".claude.json"))
        })
    else {
        return;
    };
    if let Err(error) = record_workspace_trust(&config, cwd) {
        eprintln!("pre-trust {}: {error}", cwd.display());
    }
}

/// The read-modify-write half, split out so tests drive a temp registry instead
/// of the developer's real one. Writes through a temp file + rename so a crash
/// mid-write cannot truncate a registry holding every project's state.
pub(crate) fn record_workspace_trust(config: &Path, cwd: &Path) -> Result<(), String> {
    let key = cwd.to_string_lossy().to_string();
    let mut registry: Value = match std::fs::read_to_string(config) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", config.display()))?
        }
        // No registry yet: claude will merge its own defaults into ours.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(format!("read {}: {error}", config.display())),
    };
    let projects = registry
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", config.display()))?
        .entry("projects")
        .or_insert_with(|| json!({}));
    let project = projects
        .as_object_mut()
        .ok_or_else(|| "projects is not a JSON object".to_string())?
        .entry(key)
        .or_insert_with(|| json!({}));
    let project = project
        .as_object_mut()
        .ok_or_else(|| "project entry is not a JSON object".to_string())?;
    if project.get("hasTrustDialogAccepted") == Some(&json!(true)) {
        return Ok(());
    }
    project.insert("hasTrustDialogAccepted".to_string(), json!(true));

    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let staged = config.with_extension("json.build-tmp");
    std::fs::write(
        &staged,
        serde_json::to_vec_pretty(&registry).map_err(|e| format!("serialize: {e}"))?,
    )
    .map_err(|e| format!("write {}: {e}", staged.display()))?;
    std::fs::rename(&staged, config).map_err(|e| format!("rename {}: {e}", config.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude keeps workspace trust in a shared registry that also holds every
    /// other project's state, so the grant must be additive and idempotent.
    #[test]
    fn claude_workspace_trust_is_added_without_disturbing_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "firstStartTime": "2026-01-01",
                "projects": {
                    "/Users/someone/other": { "hasTrustDialogAccepted": true, "lastCost": 1.5 }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let worktree = std::path::Path::new("/tmp/build worktrees/run-9");
        record_workspace_trust(&config, worktree).unwrap();
        record_workspace_trust(&config, worktree).expect("idempotent");

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            written["projects"]["/tmp/build worktrees/run-9"]["hasTrustDialogAccepted"],
            json!(true)
        );
        // Neighbouring state survives: this file is not Build's to own.
        assert_eq!(written["firstStartTime"], json!("2026-01-01"));
        assert_eq!(
            written["projects"]["/Users/someone/other"]["lastCost"],
            json!(1.5)
        );
        assert!(!dir.path().join(".claude.json.build-tmp").exists());
    }

    #[test]
    fn claude_workspace_trust_creates_a_registry_that_does_not_exist_yet() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("nested").join(".claude.json");
        record_workspace_trust(&config, std::path::Path::new("/tmp/wt")).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            written["projects"]["/tmp/wt"]["hasTrustDialogAccepted"],
            json!(true)
        );
    }

    #[test]
    fn the_encoded_project_dir_is_where_a_transcript_is_looked_for() {
        // Claude Code's transcript dir encoding: '/' and '.' both become '-'.
        assert_eq!(
            encode_project_dir(std::path::Path::new("/Users/z/proj.web")),
            "-Users-z-proj-web"
        );

        let root = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj.web");
        assert!(
            !transcript_exists(root.path(), cwd),
            "no encoded dir → no transcript"
        );
        let encoded_dir = root.path().join("-Users-z-proj-web");
        std::fs::create_dir_all(&encoded_dir).unwrap();
        assert!(
            !transcript_exists(root.path(), cwd),
            "an empty dir holds no transcript"
        );
        std::fs::write(encoded_dir.join("notes.txt"), "not a transcript").unwrap();
        assert!(
            !transcript_exists(root.path(), cwd),
            "only .jsonl files count"
        );
        std::fs::write(encoded_dir.join("session.jsonl"), "{}\n").unwrap();
        assert!(transcript_exists(root.path(), cwd));
    }

    /// The trait method and the free function have to agree about where claude
    /// keeps transcripts, or a resume decision is made against the wrong tree.
    #[test]
    fn the_probe_looks_under_the_homes_claude_projects_dir() {
        let home = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj");
        assert!(!ClaudeHarness.has_transcript(home.path(), cwd));

        let encoded = home
            .path()
            .join(".claude/projects")
            .join(encode_project_dir(cwd));
        std::fs::create_dir_all(&encoded).unwrap();
        std::fs::write(encoded.join("session.jsonl"), "{}\n").unwrap();
        assert!(ClaudeHarness.has_transcript(home.path(), cwd));
    }
}
