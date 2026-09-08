//! Codex CLI.
//!
//! Everything this provider is told arrives as `--config` overrides on the
//! command that starts the session: the MCP server to run, the tools that
//! server is allowed to expose, the trust grant for the worktree, and the paste
//! handling Build's prompt delivery depends on. Nothing is written outside the
//! process, which is why this harness needs no workspace preparation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::harness::{
    Harness, HarnessContext, SessionLocator, INHERITED_AGENT_MARKERS, REAL_TUI_SETTLE,
    REAL_TUI_SUBMIT_DELAY,
};
use crate::models::{AgentProvider, ModelChoice, ModelOption};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

/// Reasoning-effort levels codex accepts, across all its models.
pub const EFFORT_LEVELS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

pub(super) fn models() -> Vec<ModelOption> {
    const THROUGH_XHIGH: &[&str] = &["low", "medium", "high", "xhigh"];
    const THROUGH_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];
    vec![
        ModelOption {
            id: "gpt-6-astra",
            label: "GPT-6-Astra",
            supports_effort: true,
            efforts: &EFFORT_LEVELS,
        },
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

pub struct CodexHarness;

impl Harness for CodexHarness {
    fn provider(&self) -> AgentProvider {
        AgentProvider::Codex
    }

    fn label(&self) -> &'static str {
        "Codex TUI"
    }

    /// The curated catalog, most capable first. (cached: 2026-09)
    ///
    /// Codex has an experimental debug catalog command, but Build cannot assume
    /// every installed CLI version exposes it; shipping the catalog keeps the
    /// web contract deterministic.
    fn models(&self) -> Vec<ModelOption> {
        models()
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
    ) -> Result<HarnessSpec, crate::harness::HarnessError> {
        let mut spec = HarnessSpec::new("codex")
            .settle(REAL_TUI_SETTLE)
            .submit_delay(REAL_TUI_SUBMIT_DELAY)
            .unset_all(INHERITED_AGENT_MARKERS)
            .arg("--dangerously-bypass-approvals-and-sandbox");
        for arg in self.model_args(choice) {
            spec = spec.arg(arg);
        }
        for override_arg in CodexMcpConfig::new(options, context).overrides() {
            spec = spec.arg("--config").arg(override_arg);
        }
        for override_arg in [
            format!(
                "projects.{}.trust_level=\"trusted\"",
                serde_json::to_string(&options.cwd.to_string_lossy())
                    .expect("worktree path serializes")
            ),
            // Build writes prompt bytes and Enter back-to-back. Codex's
            // fallback detector otherwise classifies that stream as a paste
            // burst and turns Enter into a newline, so the prompt remains
            // visible but unsent. This PTY advertises and frames real pastes
            // explicitly; the fallback is unnecessary.
            "disable_paste_burst=true".to_string(),
        ] {
            spec = spec.arg("--config").arg(override_arg);
        }
        // Alternatives, never both — claude's rule in codex's shape: resume is
        // a SUBCOMMAND here, and it keeps the argv tail the guess already
        // worked from, behind every global `--config` override.
        match options.resume_session_id.as_deref() {
            Some(named) => spec = spec.arg("resume").arg(named),
            None if options.continue_session => spec = spec.arg("resume").arg("--last"),
            None => {}
        }
        Ok(spec)
    }

    fn has_transcript(&self, home: &Path, cwd: &Path) -> bool {
        transcript_exists(&home.join(".codex/sessions"), cwd)
    }

    fn session_locator(&self, home: &Path, cwd: &Path) -> Option<Box<dyn SessionLocator>> {
        Some(Box::new(CodexSessionLocator::watching(
            home.join(".codex/sessions"),
            cwd,
        )))
    }

    /// Read off FILENAMES: every rollout carries its uuid in its own name, so
    /// the question is answered without opening a single file.
    fn holds_conversation(&self, home: &Path, _cwd: &Path, id: &str) -> bool {
        if !crate::harness::is_a_filename(id) {
            return false;
        }
        let named = format!("-{id}.jsonl");
        let mut held = false;
        visit_rollouts(&home.join(".codex/sessions"), "", &mut |path, _| {
            held = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&named));
            !held
        });
        held
    }
}

pub(super) struct CodexMcpConfig {
    command: String,
    args: String,
    socket: String,
    token: String,
    tools: String,
}

impl CodexMcpConfig {
    pub(super) fn new(options: &SpawnOptions, context: &HarnessContext) -> CodexMcpConfig {
        CodexMcpConfig {
            command: serde_json::to_string(&context.bridge_exe).expect("path serializes"),
            args: serde_json::to_string(&vec!["mcp", "--task", options.owner_id.as_str()])
                .expect("MCP args serialize"),
            socket: serde_json::to_string(&context.mcp_socket).expect("socket serializes"),
            token: serde_json::to_string(&options.mcp_session_token).expect("MCP token serializes"),
            tools: serde_json::to_string(&mcp_tool_names(&options.owner_id))
                .expect("tool names serialize"),
        }
    }

    pub(super) fn overrides(&self) -> Vec<String> {
        vec![
            format!("mcp_servers.build.command={}", self.command),
            format!("mcp_servers.build.args={}", self.args),
            format!("mcp_servers.build.env.BRIDGE_MCP_SOCKET={}", self.socket),
            format!("mcp_servers.build.env.BRIDGE_MCP_TOKEN={}", self.token),
            "mcp_servers.build.required=true".to_string(),
            format!("mcp_servers.build.enabled_tools={}", self.tools),
            "mcp_servers.build.default_tools_approval_mode=\"approve\"".to_string(),
        ]
    }
}

/// Names the conversation a codex session is having by the rollout codex opens
/// for it.
///
/// Two things make this harder than claude's, and both are codex's shape rather
/// than a choice here. The rollouts are GLOBAL — every checkout's conversations
/// land in one dated tree — so a candidate counts only when the cwd in its own
/// header is this session's; recency alone misattributes the moment two codex
/// sessions run at once. And the tree grows for the life of the machine, so the
/// walk is bounded to the dated directories at or after the newest one that
/// existed when this locator was built: the child's rollout can only land
/// there.
struct CodexSessionLocator {
    sessions_root: PathBuf,
    /// The checkout this session is running in, canonicalized once so every
    /// candidate is compared the same way.
    cwd: PathBuf,
    /// The dated directory the walk starts at — see the struct doc.
    from: String,
    /// The rollouts already inside that bound. Everything else is this
    /// session's candidate.
    before: HashSet<PathBuf>,
    named: Mutex<Option<String>>,
}

impl CodexSessionLocator {
    fn watching(sessions_root: PathBuf, cwd: &Path) -> CodexSessionLocator {
        let mut newest = String::new();
        visit_rollouts(&sessions_root, "", &mut |_, dated| {
            if dated > newest.as_str() {
                newest = dated.to_string();
            }
            true
        });
        let mut before = HashSet::new();
        visit_rollouts(&sessions_root, &newest, &mut |path, _| {
            before.insert(path.to_path_buf());
            true
        });
        CodexSessionLocator {
            sessions_root,
            cwd: std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf()),
            from: newest,
            before,
            named: Mutex::new(None),
        }
    }
}

impl SessionLocator for CodexSessionLocator {
    fn session_id(&self) -> Option<String> {
        let mut named = self.named.lock().unwrap();
        if named.is_none() {
            let mut candidates = Vec::new();
            visit_rollouts(&self.sessions_root, &self.from, &mut |path, _| {
                if !self.before.contains(path) {
                    if let Some(header) = rollout_header(path) {
                        if rollout_cwd(&header).is_some_and(|opened_in| opened_in == self.cwd) {
                            candidates.push(
                                rollout_id(&header)
                                    .or_else(|| uuid_in_rollout_name(path))
                                    .unwrap_or_default(),
                            );
                        }
                    }
                }
                // A second candidate already settles the answer at "refuse",
                // so there is nothing left to learn from the rest of the walk.
                candidates.len() < 2
            });
            // Exactly one, or none — the claude locator's rule, for the same
            // reason: two of this checkout's sessions opened together write two
            // rollouts nobody can tell apart, and guessing between them is the
            // misattribution this exists to end.
            if let [only] = candidates.as_slice() {
                if !only.is_empty() {
                    *named = Some(only.clone());
                }
            }
        }
        named.clone()
    }
}

/// Call `visit` for every rollout at or after the dated directory `from`, with
/// the path and the directory's own dated key (its path relative to `root`).
///
/// Codex's dated layout is zero-padded `<Y>/<M>/<D>`, so the relative directory
/// compares lexicographically the way the dates it names compare — which is
/// what lets a walk skip everything older than the session asking. `from` empty
/// means the whole tree.
///
/// `visit` answers whether to keep walking, so a question with a first answer —
/// "is there one at all", "is this id still here" — stops at it instead of
/// opening every rollout the machine has ever written.
///
/// Never follows a symlink: these directories are the user's, and a loop in
/// them must not hang a sweep tick.
fn visit_rollouts(root: &Path, from: &str, visit: &mut dyn FnMut(&Path, &str) -> bool) {
    let mut dirs = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, dated)) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let below = if dated.is_empty() {
                    name.to_string()
                } else {
                    format!("{dated}/{name}")
                };
                // Either this directory is on the way down to the bound, or it
                // is at or past it. Anything else is a date older than the one
                // this session started on, and cannot hold its rollout.
                if from.starts_with(&below) || below.as_str() >= from {
                    dirs.push((path, below));
                }
                continue;
            }
            if kind.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
                && !visit(&path, &dated)
            {
                return;
            }
        }
    }
}

/// Codex's first rollout line: the session metadata, carrying the checkout the
/// session was opened in and the id it gave the conversation. Scanning that one
/// small header is all any question here needs.
fn rollout_header(path: &Path) -> Option<serde_json::Value> {
    use std::io::BufRead;

    let file = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    serde_json::from_str(&first).ok()
}

/// The checkout a rollout header says its session was opened in, canonicalized
/// so two spellings of one directory compare equal.
fn rollout_cwd(header: &serde_json::Value) -> Option<PathBuf> {
    let opened_in = PathBuf::from(
        header
            .pointer("/payload/cwd")
            .and_then(serde_json::Value::as_str)?,
    );
    Some(std::fs::canonicalize(&opened_in).unwrap_or(opened_in))
}

/// The name a rollout header gives its conversation.
fn rollout_id(header: &serde_json::Value) -> Option<String> {
    header
        .pointer("/payload/id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// The uuid codex put in the rollout's own filename
/// (`rollout-<timestamp>-<uuid>.jsonl`) — the same name the header carries, and
/// the fallback for a header that did not write one.
fn uuid_in_rollout_name(path: &Path) -> Option<String> {
    const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    let groups: Vec<&str> = stem.rsplitn(6, '-').take(5).collect();
    if groups.len() < 5 {
        return None;
    }
    let uuid: Vec<&str> = groups.into_iter().rev().collect();
    uuid.iter()
        .zip(UUID_GROUPS)
        .all(|(group, width)| group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| uuid.join("-"))
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
    let wanted = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut found = false;
    visit_rollouts(root, "", &mut |path, _| {
        found = rollout_header(path)
            .as_ref()
            .and_then(rollout_cwd)
            .is_some_and(|opened_in| opened_in == wanted);
        !found
    });
    found
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

    fn spec_for(options: &SpawnOptions) -> String {
        CodexHarness
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

    /// The same three arms as claude's, in codex's shape: resume is a
    /// SUBCOMMAND, not a flag, and it stays at the argv tail behind the global
    /// `--config` overrides — the position the guess already worked from.
    #[test]
    fn codex_resumes_by_name_or_guesses_by_cwd_or_starts_fresh() {
        let named = spec_for(&SpawnOptions {
            resume_session_id: Some("sess-1".to_string()),
            continue_session: true,
            ..SpawnOptions::default()
        });
        assert!(named.ends_with("resume sess-1"), "{named}");
        assert!(
            !named.contains("--last"),
            "the name wins outright, never both: {named}"
        );
        assert!(
            named.contains("--config disable_paste_burst=true resume sess-1"),
            "the overrides stay ahead of the subcommand: {named}"
        );

        let guessed = spec_for(&SpawnOptions {
            continue_session: true,
            ..SpawnOptions::default()
        });
        assert!(guessed.ends_with("resume --last"), "{guessed}");

        let fresh = spec_for(&SpawnOptions::default());
        assert!(!fresh.contains("resume"), "{fresh}");
    }

    /// Write one dated rollout, the way codex does: a `session_meta` first line
    /// carrying the checkout it was opened in and the id it gave the
    /// conversation.
    fn write_rollout(dated: &Path, name: &str, cwd: &Path, id: &str) {
        std::fs::create_dir_all(dated).unwrap();
        std::fs::write(
            dated.join(name),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":{},\"cwd\":{}}}}}\n{{}}\n",
                serde_json::to_string(id).unwrap(),
                serde_json::to_string(&cwd.display().to_string()).unwrap()
            ),
        )
        .unwrap();
    }

    /// Codex's rollouts are GLOBAL — every checkout's conversations land in one
    /// dated tree — so recency alone misattributes the moment two sessions run
    /// at once. The locator matches the header's cwd instead, and reads the id
    /// out of the same header.
    #[test]
    fn the_codex_locator_matches_the_rollout_by_cwd_and_never_by_recency() {
        let home = tempfile::tempdir().unwrap();
        let mine = home.path().join("mine");
        let theirs = home.path().join("theirs");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&theirs).unwrap();
        let dated = home.path().join(".codex/sessions/2026/08/29");
        // What was already there when Build reserved this session.
        write_rollout(
            &dated,
            "rollout-2026-08-29T09-00-00-11111111-1111-1111-1111-111111111111.jsonl",
            &mine,
            "an-older-one",
        );

        let locator = CodexHarness
            .session_locator(home.path(), &mine)
            .expect("an opaque CLI wrapper names its conversation on disk");
        assert_eq!(locator.session_id(), None, "nothing new has appeared yet");

        // The other checkout's session opens first, and is NEWER — the shape
        // that a guess by recency gets wrong.
        write_rollout(
            &dated,
            "rollout-2026-08-29T10-00-00-22222222-2222-2222-2222-222222222222.jsonl",
            &theirs,
            "not-mine",
        );
        assert_eq!(
            locator.session_id(),
            None,
            "another checkout's conversation is never this session's"
        );

        write_rollout(
            &dated,
            "rollout-2026-08-29T10-00-01-33333333-3333-3333-3333-333333333333.jsonl",
            &mine,
            "mine-at-last",
        );
        assert_eq!(
            locator.session_id().as_deref(),
            Some("mine-at-last"),
            "the id comes off the header codex wrote, beside the cwd that matched"
        );
    }

    /// Two of this checkout's own sessions open together — a branch legally
    /// carries several agents — and neither file can be told from the other.
    /// The locator refuses rather than guessing.
    #[test]
    fn the_codex_locator_refuses_to_guess_between_two_of_its_own_checkouts_rollouts() {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().join("shared");
        std::fs::create_dir_all(&cwd).unwrap();
        let dated = home.path().join(".codex/sessions/2026/08/29");
        std::fs::create_dir_all(&dated).unwrap();

        let locator = CodexHarness.session_locator(home.path(), &cwd).unwrap();
        write_rollout(
            &dated,
            "rollout-2026-08-29T10-00-00-44444444-4444-4444-4444-444444444444.jsonl",
            &cwd,
            "first",
        );
        write_rollout(
            &dated,
            "rollout-2026-08-29T10-00-01-55555555-5555-5555-5555-555555555555.jsonl",
            &cwd,
            "second",
        );
        assert_eq!(locator.session_id(), None);
    }

    /// The filename's uuid is codex's own record of the same name, so a header
    /// that never named the conversation still yields one.
    #[test]
    fn the_codex_locator_falls_back_to_the_uuid_in_the_filename() {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let dated = home.path().join(".codex/sessions/2026/08/29");
        std::fs::create_dir_all(&dated).unwrap();

        let locator = CodexHarness.session_locator(home.path(), &cwd).unwrap();
        std::fs::write(
            dated.join("rollout-2026-08-29T10-00-00-66666666-6666-6666-6666-666666666666.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{}}}}}\n",
                serde_json::to_string(&cwd.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        assert_eq!(
            locator.session_id().as_deref(),
            Some("66666666-6666-6666-6666-666666666666")
        );
    }

    /// A recorded id is worth spending only while the rollout it names is still
    /// there. Codex reads it off FILENAMES: no file is opened to answer this.
    #[test]
    fn codex_holds_a_conversation_named_by_a_rollout_filename() {
        let home = tempfile::tempdir().unwrap();
        let cwd = home.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = "77777777-7777-7777-7777-777777777777";
        assert!(!CodexHarness.holds_conversation(home.path(), &cwd, id));

        let dated = home.path().join(".codex/sessions/2026/08/29");
        let name = format!("rollout-2026-08-29T10-00-00-{id}.jsonl");
        write_rollout(&dated, &name, &cwd, id);
        assert!(CodexHarness.holds_conversation(home.path(), &cwd, id));
        assert!(!CodexHarness.holds_conversation(
            home.path(),
            &cwd,
            "88888888-8888-8888-8888-888888888888"
        ));

        std::fs::remove_file(dated.join(&name)).unwrap();
        assert!(
            !CodexHarness.holds_conversation(home.path(), &cwd, id),
            "a conversation that is gone is not resumed by name"
        );
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
