//! Claude Code.
//!
//! Trust is the shape that makes this provider different from a purely
//! argv-configured one: claude keeps workspace trust in a shared registry under
//! the home directory, so a fresh worktree has to be recorded there BEFORE a
//! session opens in it. That is the one place Build writes outside its own
//! tree, and it only ever adds the flag for a path Build itself created.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::harness::{
    is_a_filename, Harness, HarnessContext, SessionLocator, INHERITED_AGENT_MARKERS,
    REAL_TUI_SETTLE, REAL_TUI_SUBMIT_DELAY,
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

    /// The terminal carrier of this CLI, named apart from the headless one
    /// because both are creatable side by side and an agent is locked to the
    /// one it was created on.
    fn label(&self) -> &'static str {
        "Claude Code TUI"
    }

    /// The curated catalog, most capable first. (cached: 2026-09)
    fn binary(&self) -> &'static str {
        "claude"
    }

    fn models(&self) -> Vec<ModelOption> {
        vec![
            ModelOption {
                id: "claude-fable-5-1",
                label: "Claude Fable 5.1",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-opus-5-5",
                label: "Claude Opus 5.5",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-opus-5",
                label: "Claude Opus 5",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-opus-4-8",
                label: "Claude Opus 4.8",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-sonnet-5",
                label: "Claude Sonnet 5",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-sonnet-4-6",
                label: "Claude Sonnet 4.6",
                supports_effort: true,
                efforts: &EFFORT_LEVELS,
                context_window: Some(1_000_000),
            },
            ModelOption {
                id: "claude-haiku-4-5",
                label: "Claude Haiku 4.5",
                supports_effort: false,
                efforts: &[],
                context_window: Some(200_000),
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

    fn requires_unadorned_command(&self, prompt: &str) -> bool {
        matches!(
            prompt.split_whitespace().next(),
            Some("/clear" | "/compact")
        )
    }

    fn compacts_on_command(&self) -> bool {
        true
    }

    fn spec(
        &self,
        choice: &ModelChoice,
        options: &SpawnOptions,
        context: &HarnessContext,
    ) -> Result<HarnessSpec, crate::harness::HarnessError> {
        let (compaction_log, compaction_settings) = compaction_hooks(context);
        let mut spec = HarnessSpec::new("claude")
            .settle(REAL_TUI_SETTLE)
            .submit_delay(REAL_TUI_SUBMIT_DELAY)
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("--mcp-config")
            .arg(options.mcp_config())
            .arg("--strict-mcp-config")
            .arg("--settings")
            .arg(compaction_settings)
            .arg("--dangerously-skip-permissions");
        // The two are alternatives and never both: `--resume` names the exact
        // conversation this agent was having, `--continue` guesses the newest
        // one in the checkout, and passing both would ask for two different
        // conversations. The name wins where there is one; the guess is what
        // answers for a session that died before Build could read its name off
        // the transcript tree.
        match options.resume_session_id.as_deref() {
            Some(named) => spec = spec.arg("--resume").arg(named),
            None if options.continue_session => spec = spec.arg("--continue"),
            None => {}
        }
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        Ok(spec
            .compaction_sidecar(compaction_log.clone())
            .env(
                "BUILD_COMPACTION_LOG",
                compaction_log.to_string_lossy().into_owned(),
            )
            .env(
                "BRIDGE_MCP_SOCKET",
                context.mcp_socket.to_string_lossy().into_owned(),
            )
            .env("BRIDGE_MCP_TOKEN", &options.mcp_session_token))
    }

    fn prepare_workspace(&self, cwd: &Path) {
        pre_trust_worktree(cwd);
    }

    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        transcript_exists(&home.join(".claude/projects"), cwd)
    }

    fn session_locator(&self, home: &Path, cwd: &Path) -> Option<Box<dyn SessionLocator>> {
        Some(Box::new(ClaudeSessionLocator::watching(project_dir(
            &home.join(".claude/projects"),
            cwd,
        ))))
    }

    fn holds_conversation(&self, home: &Path, cwd: &Path, id: &str) -> bool {
        is_a_filename(id)
            && project_dir(&home.join(".claude/projects"), cwd)
                .join(format!("{id}.jsonl"))
                .is_file()
    }
}

/// Names the conversation a claude session is having by the file claude opens
/// for it: one transcript per conversation, and the filename is the id.
///
/// The snapshot is what makes the answer this session's own — every stem
/// already in the directory when the spawn was reserved belongs to somebody
/// else's conversation, so only a stem that appears afterwards can be the
/// child's.
struct ClaudeSessionLocator {
    project_dir: PathBuf,
    /// The transcripts that were already there. Never re-read: a locator is
    /// built before the child exists, so this is the whole of "not mine".
    before: HashSet<String>,
    /// The answer, once there is one. A locator never changes its answer, so
    /// this is written once and read forever after.
    named: Mutex<Option<String>>,
}

impl ClaudeSessionLocator {
    fn watching(project_dir: PathBuf) -> ClaudeSessionLocator {
        ClaudeSessionLocator {
            before: transcript_stems(&project_dir).into_iter().collect(),
            project_dir,
            named: Mutex::new(None),
        }
    }
}

impl SessionLocator for ClaudeSessionLocator {
    fn session_id(&self) -> Option<String> {
        let mut named = self.named.lock().unwrap();
        if named.is_none() {
            let mut appeared = transcript_stems(&self.project_dir)
                .into_iter()
                .filter(|stem| !self.before.contains(stem));
            // Exactly one, or none. A branch legally carries several agents in
            // one checkout, and two sessions opened together there write two
            // files nobody can tell apart — so the locator refuses to guess
            // rather than name the wrong conversation. Refusing only forgoes
            // the sharper resume; naming wrongly hands an agent somebody
            // else's context.
            if let (Some(only), None) = (appeared.next(), appeared.next()) {
                *named = Some(only);
            }
        }
        named.clone()
    }
}

fn compaction_hooks(context: &HarnessContext) -> (PathBuf, String) {
    let directory = context.state_root.join("harness/claude/compactions");
    let path = directory.join(format!("{}.jsonl", uuid::Uuid::new_v4()));
    let hook = |completed| {
        json!({
            "hooks": [{
                "type": "command",
                "command": format!(
                    "printf '%s\\n' '{{\"type\":\"compaction\",\"completed\":{completed}}}' >> \"$BUILD_COMPACTION_LOG\" 2>/dev/null"
                )
            }]
        })
    };
    let settings = json!({ "hooks": {
        "PreCompact": [hook(false)],
        "PostCompact": [hook(true)]
    }});
    (path, settings.to_string())
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

/// Where `cwd`'s transcripts live under a `projects` root.
pub(crate) fn project_dir(root: &Path, cwd: &Path) -> PathBuf {
    root.join(encode_project_dir(cwd))
}

/// The name of every `.jsonl` transcript in `project_dir`, without its
/// extension — which for claude is the id of the conversation it holds. Empty
/// for a directory that does not exist, which is a checkout nobody has opened
/// claude in.
pub(crate) fn transcript_stems(project_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .collect()
}

/// True iff the encoded directory exists under `root` and holds at least one
/// `.jsonl` transcript.
pub(crate) fn transcript_exists(root: &Path, cwd: &Path) -> bool {
    !transcript_stems(&project_dir(root, cwd)).is_empty()
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

    fn spec_for(options: &SpawnOptions) -> String {
        ClaudeHarness
            .spec(
                &ModelChoice::default(),
                options,
                &HarnessContext {
                    bridge_exe: PathBuf::from("/usr/local/bin/build-bridge"),
                    mcp_socket: PathBuf::from("/tmp/build-mcp.sock"),
                    state_root: PathBuf::from("/tmp/build-state"),
                },
            )
            .unwrap()
            .args
            .join(" ")
    }

    #[test]
    fn claude_tui_installs_silent_scoped_compaction_hooks_without_writing_state() {
        let state = tempfile::tempdir().unwrap();
        let context = HarnessContext {
            bridge_exe: PathBuf::from("/usr/local/bin/build-bridge"),
            mcp_socket: PathBuf::from("/tmp/build-mcp.sock"),
            state_root: state.path().to_path_buf(),
        };
        let options = SpawnOptions {
            mcp_session_token: "secret-capability".to_string(),
            ..SpawnOptions::default()
        };
        let spec = ClaudeHarness
            .spec(&ModelChoice::default(), &options, &context)
            .unwrap();
        let settings_index = spec
            .args
            .iter()
            .position(|arg| arg == "--settings")
            .unwrap();
        let settings: Value = serde_json::from_str(&spec.args[settings_index + 1]).unwrap();
        assert!(settings["hooks"]["PreCompact"].is_array());
        assert!(settings["hooks"]["PostCompact"].is_array());
        let commands = settings["hooks"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|groups| {
                groups
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|group| group["hooks"].as_array().unwrap())
            });
        for command in commands {
            let command = command["command"].as_str().unwrap();
            assert!(command.contains("$BUILD_COMPACTION_LOG"));
            assert!(!command.contains("secret-capability"));
        }
        let sidecar = spec.compaction_sidecar.unwrap();
        assert!(sidecar.starts_with(state.path()));
        assert!(!sidecar.display().to_string().contains("secret-capability"));
        assert!(
            !sidecar.exists(),
            "building an unused spec has no disk side effects"
        );
    }

    /// The three arms, in the one shape claude's argv has for them.
    ///
    /// `--resume <id>` names the exact conversation Build was speaking to and
    /// `--continue` guesses the newest one in the checkout, so they are
    /// alternatives and never companions — and a spawn with no history to pick
    /// up asks for neither, because the checkout's old conversation belongs to
    /// whoever had it.
    #[test]
    fn claude_resumes_by_name_or_guesses_by_cwd_or_starts_fresh() {
        let named = spec_for(&SpawnOptions {
            resume_session_id: Some("sess-1".to_string()),
            continue_session: true,
            ..SpawnOptions::default()
        });
        assert!(named.contains("--resume sess-1"), "{named}");
        assert!(
            !named.contains("--continue"),
            "the name wins outright, never both: {named}"
        );

        let guessed = spec_for(&SpawnOptions {
            continue_session: true,
            ..SpawnOptions::default()
        });
        assert!(guessed.contains("--continue"), "{guessed}");
        assert!(!guessed.contains("--resume"), "{guessed}");

        let fresh = spec_for(&SpawnOptions::default());
        assert!(!fresh.contains("--resume"), "{fresh}");
        assert!(!fresh.contains("--continue"), "{fresh}");
    }

    /// The locator names the conversation THIS session is having, and it knows
    /// which one that is by what was already there: the transcripts present
    /// when the spawn reserved the session are somebody else's, and the file
    /// that appears afterwards is the child's own.
    #[test]
    fn the_claude_locator_names_the_transcript_that_appeared_after_it_was_built() {
        let home = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj");
        let project_dir = home
            .path()
            .join(".claude/projects")
            .join(encode_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        // A conversation somebody else had in this checkout, before Build
        // opened anything here.
        std::fs::write(project_dir.join("older-session.jsonl"), "{}\n").unwrap();

        let locator = ClaudeHarness
            .session_locator(home.path(), cwd)
            .expect("an opaque CLI wrapper names its conversation on disk");
        assert_eq!(
            locator.session_id(),
            None,
            "a pre-existing transcript is never this session's"
        );

        std::fs::write(project_dir.join("d2a1-mine.jsonl"), "{}\n").unwrap();
        assert_eq!(
            locator.session_id().as_deref(),
            Some("d2a1-mine"),
            "the filename IS the id claude gave the conversation"
        );

        // Cached: the answer a locator gave once is the answer it keeps, so a
        // later conversation in the same checkout cannot rename this session.
        std::fs::write(project_dir.join("someone-elses-later.jsonl"), "{}\n").unwrap();
        assert_eq!(locator.session_id().as_deref(), Some("d2a1-mine"));
    }

    /// Two sessions opened together in one checkout write two files nobody can
    /// tell apart, and a branch is allowed to carry several agents. Guessing by
    /// recency is the misattribution the locator exists to end, so it refuses
    /// to guess — indefinitely, because a missing id only forgoes the sharper
    /// resume and never errs.
    #[test]
    fn the_claude_locator_refuses_to_guess_between_two_new_transcripts() {
        let home = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/shared");
        let project_dir = home
            .path()
            .join(".claude/projects")
            .join(encode_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();

        let locator = ClaudeHarness.session_locator(home.path(), cwd).unwrap();
        std::fs::write(project_dir.join("one.jsonl"), "{}\n").unwrap();
        std::fs::write(project_dir.join("two.jsonl"), "{}\n").unwrap();
        assert_eq!(locator.session_id(), None);
        // And it stays refused: the files only accumulate from here.
        std::fs::write(project_dir.join("three.jsonl"), "{}\n").unwrap();
        assert_eq!(locator.session_id(), None);
    }

    /// A recorded id is worth spending only while the conversation it names is
    /// still on disk — the check that makes a dead id cost zero restarts.
    #[test]
    fn claude_holds_a_conversation_only_while_its_transcript_is_there() {
        let home = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj");
        assert!(!ClaudeHarness.holds_conversation(home.path(), cwd, "sess-1"));

        let project_dir = home
            .path()
            .join(".claude/projects")
            .join(encode_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(project_dir.join("sess-1.jsonl"), "{}\n").unwrap();
        assert!(ClaudeHarness.holds_conversation(home.path(), cwd, "sess-1"));
        assert!(!ClaudeHarness.holds_conversation(home.path(), cwd, "sess-2"));

        std::fs::remove_file(project_dir.join("sess-1.jsonl")).unwrap();
        assert!(
            !ClaudeHarness.holds_conversation(home.path(), cwd, "sess-1"),
            "a conversation that is gone is not resumed by name"
        );
    }

    /// An id is a filename component and nothing else. A recorded id rides a
    /// JSON record on disk, so the one that walks out of the tree it names is
    /// refused rather than stat'ed.
    #[test]
    fn a_session_id_that_is_not_a_filename_names_no_conversation() {
        let home = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj");
        let project_dir = home
            .path()
            .join(".claude/projects")
            .join(encode_project_dir(cwd));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(home.path().join("escaped.jsonl"), "{}\n").unwrap();

        assert!(!ClaudeHarness.holds_conversation(home.path(), cwd, "../../../escaped"));
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
