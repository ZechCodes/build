use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;

const AGENT: &str = "workspace-agent-one";
const SESSION: &str = "11111111-1111-1111-1111-111111111111";
const SIBLING: &str = "22222222-2222-2222-2222-222222222222";

struct Fixture {
    home: tempfile::TempDir,
    state: tempfile::TempDir,
    cwd: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            cwd: tempfile::tempdir().unwrap(),
        }
    }

    fn lineage(&self, provider: AgentProvider, id: &str) -> SessionLineage {
        serde_json::from_value(serde_json::json!({
            "id": "process-one", "agent_id": AGENT, "provider": provider.wire_id(),
            "cwd": self.cwd.path(), "resume_session_id": id,
            "phase": "conversation", "started_at": "2026-10-03T00:00:00Z"
        }))
        .unwrap()
    }

    fn prepare(
        &self,
        provider: AgentProvider,
        lineages: &[SessionLineage],
    ) -> Result<HarnessConversationCleanup, HarnessError> {
        HarnessConversationCleanup::prepare(
            self.home.path(),
            self.state.path(),
            AGENT,
            provider,
            lineages,
        )
    }

    fn claude(&self, id: &str) -> PathBuf {
        self.home
            .path()
            .join(".claude/projects")
            .join(super::super::claude::encode_project_dir(self.cwd.path()))
            .join(format!("{id}.jsonl"))
    }

    fn codex(&self, id: &str) -> PathBuf {
        self.home
            .path()
            .join(".codex/sessions/2026/10/03")
            .join(format!("rollout-2026-10-03T00-00-00-{id}.jsonl"))
    }

    fn pi(&self, agent: &str) -> PathBuf {
        self.state.path().join("harness/pi/sessions").join(agent)
    }

    fn write_rollout(&self, path: &Path, cwd: &Path, id: &str) {
        write(
            path,
            &format!(
                "{}\n",
                serde_json::json!({
                    "type": "session_meta", "payload": {"cwd": cwd, "id": id}
                })
            ),
        );
    }
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn cleanup_stages_and_deletes_only_exact_claude_and_adk_transcripts() {
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        let f = Fixture::new();
        let exact = f.claude(SESSION);
        let sibling = f.claude(SIBLING);
        write(&exact, "old conversation");
        write(&sibling, "unrelated conversation");
        let cleanup = f
            .prepare(provider, &[f.lineage(provider, SESSION)])
            .unwrap();
        assert!(!exact.exists(), "staged transcripts cannot be resumed");
        assert_eq!(
            fs::read_to_string(&sibling).unwrap(),
            "unrelated conversation"
        );
        cleanup.commit().unwrap();
        assert!(!exact.exists());
        assert_eq!(fs::read_dir(sibling.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
fn cleanup_stages_and_deletes_only_verified_codex_and_appserver_rollouts() {
    for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
        let f = Fixture::new();
        let exact = f.codex(SESSION);
        let sibling = f.codex(SIBLING);
        f.write_rollout(&exact, f.cwd.path(), SESSION);
        f.write_rollout(&sibling, f.cwd.path(), SIBLING);
        let cleanup = f
            .prepare(provider, &[f.lineage(provider, SESSION)])
            .unwrap();
        assert!(!exact.exists());
        assert!(sibling.exists());
        cleanup.commit().unwrap();
        assert_eq!(fs::read_dir(sibling.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
fn pi_cleanup_removes_its_exact_agent_directory_even_without_lineage() {
    let f = Fixture::new();
    write(&f.pi(AGENT).join("nested/old.jsonl"), "old Pi session");
    write(
        &f.pi("another-agent").join("keep.jsonl"),
        "other Pi session",
    );
    let extension = f
        .state
        .path()
        .join("harness/pi/extensions/hash/build-tools.ts");
    write(&extension, "shared extension");
    f.prepare(AgentProvider::Pi, &[]).unwrap().commit().unwrap();
    assert!(!f.pi(AGENT).exists());
    assert!(f.pi("another-agent").join("keep.jsonl").exists());
    assert!(extension.exists());
}

#[test]
fn dropping_staged_cleanup_restores_all_original_artifacts_and_deduplicates_lineage() {
    let f = Fixture::new();
    let claude = f.claude(SESSION);
    let codex = f.codex(SESSION);
    write(&claude, "old conversation");
    f.write_rollout(&codex, f.cwd.path(), SESSION);
    let lineage = f.lineage(AgentProvider::Claude, SESSION);
    let cleanup = f
        .prepare(
            AgentProvider::Pi,
            &[
                lineage.clone(),
                lineage,
                f.lineage(AgentProvider::Codex, SESSION),
            ],
        )
        .unwrap();
    assert!(!claude.exists());
    assert!(!codex.exists());
    drop(cleanup);
    assert_eq!(fs::read_to_string(claude).unwrap(), "old conversation");
    assert!(codex.exists());
}

#[test]
fn missing_or_unattributed_lineage_does_not_guess_at_native_sessions() {
    let f = Fixture::new();
    let exact = f.claude(SESSION);
    write(&exact, "keep");
    let mut lineage = f.lineage(AgentProvider::Claude, SESSION);
    lineage.agent_id = "another-agent".into();
    f.prepare(AgentProvider::Claude, &[lineage])
        .unwrap()
        .commit()
        .unwrap();
    let mut lineage = f.lineage(AgentProvider::Claude, SESSION);
    lineage.agent_id.clear();
    f.prepare(AgentProvider::Claude, &[lineage])
        .unwrap()
        .commit()
        .unwrap();
    let mut lineage = f.lineage(AgentProvider::Claude, SESSION);
    lineage.resume_session_id = None;
    f.prepare(AgentProvider::Claude, &[lineage])
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(fs::read_to_string(exact).unwrap(), "keep");
}

#[test]
fn absent_native_state_is_an_idempotent_cleanup() {
    let f = Fixture::new();
    for provider in AgentProvider::ALL {
        f.prepare(provider, &[f.lineage(provider, SESSION)])
            .unwrap()
            .commit()
            .unwrap();
    }
    assert_eq!(fs::read_dir(f.home.path()).unwrap().count(), 0);
}

#[test]
fn traversal_in_native_ids_or_pi_agent_ids_is_refused() {
    let f = Fixture::new();
    for provider in [
        AgentProvider::Claude,
        AgentProvider::ClaudeAdk,
        AgentProvider::Codex,
        AgentProvider::CodexAppServer,
    ] {
        assert!(f
            .prepare(provider, &[f.lineage(provider, "../../outside")])
            .is_err());
    }
    assert!(HarnessConversationCleanup::prepare(
        f.home.path(),
        f.state.path(),
        "../../outside",
        AgentProvider::Pi,
        &[]
    )
    .is_err());
}

#[test]
fn cleanup_refuses_symlinked_provider_roots_and_preserves_their_targets() {
    for (provider, relative) in [
        (AgentProvider::Claude, ".claude"),
        (AgentProvider::ClaudeAdk, ".claude"),
        (AgentProvider::Codex, ".codex"),
        (AgentProvider::CodexAppServer, ".codex"),
    ] {
        let f = Fixture::new();
        let external = tempfile::tempdir().unwrap();
        write(&external.path().join("keep"), "unrelated");
        symlink(external.path(), f.home.path().join(relative)).unwrap();
        assert!(f
            .prepare(provider, &[f.lineage(provider, SESSION)])
            .is_err());
        assert_eq!(
            fs::read_to_string(external.path().join("keep")).unwrap(),
            "unrelated"
        );
    }
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    symlink(external.path(), f.state.path().join("harness")).unwrap();
    assert!(f.prepare(AgentProvider::Pi, &[]).is_err());
}

#[test]
fn cleanup_refuses_symlinked_exact_transcript_and_pi_directory() {
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    let outside = external.path().join("keep.jsonl");
    write(&outside, "unrelated");
    let exact = f.claude(SESSION);
    fs::create_dir_all(exact.parent().unwrap()).unwrap();
    symlink(&outside, &exact).unwrap();
    assert!(f
        .prepare(
            AgentProvider::Claude,
            &[f.lineage(AgentProvider::Claude, SESSION)]
        )
        .is_err());
    fs::create_dir_all(f.pi(AGENT).parent().unwrap()).unwrap();
    symlink(external.path(), f.pi(AGENT)).unwrap();
    assert!(f.prepare(AgentProvider::Pi, &[]).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "unrelated");
}

#[test]
fn codex_cleanup_never_spends_filename_matching_without_exact_header_lineage() {
    let f = Fixture::new();
    let exact = f.codex(SESSION);
    f.write_rollout(&exact, f.cwd.path(), SIBLING);
    f.prepare(
        AgentProvider::Codex,
        &[f.lineage(AgentProvider::Codex, SESSION)],
    )
    .unwrap()
    .commit()
    .unwrap();
    assert!(exact.exists());
    let other_cwd = tempfile::tempdir().unwrap();
    f.write_rollout(&exact, other_cwd.path(), SESSION);
    f.prepare(
        AgentProvider::Codex,
        &[f.lineage(AgentProvider::Codex, SESSION)],
    )
    .unwrap()
    .commit()
    .unwrap();
    assert!(exact.exists());
}

#[test]
fn codex_cleanup_refuses_ambiguous_verified_rollouts_and_never_follows_links() {
    let f = Fixture::new();
    let exact = f.codex(SESSION);
    f.write_rollout(&exact, f.cwd.path(), SESSION);
    let duplicate = exact
        .parent()
        .unwrap()
        .join(format!("rollout-other-{SESSION}.jsonl"));
    f.write_rollout(&duplicate, f.cwd.path(), SESSION);
    assert!(f
        .prepare(
            AgentProvider::Codex,
            &[f.lineage(AgentProvider::Codex, SESSION)]
        )
        .is_err());
    assert!(exact.exists());
    assert!(duplicate.exists());
    fs::remove_file(&duplicate).unwrap();
    let external = tempfile::tempdir().unwrap();
    let linked = external
        .path()
        .join(format!("rollout-linked-{SESSION}.jsonl"));
    f.write_rollout(&linked, f.cwd.path(), SESSION);
    symlink(
        external.path(),
        f.home.path().join(".codex/sessions/linked"),
    )
    .unwrap();
    f.prepare(
        AgentProvider::Codex,
        &[f.lineage(AgentProvider::Codex, SESSION)],
    )
    .unwrap()
    .commit()
    .unwrap();
    assert!(linked.exists());
    assert!(!exact.exists());
}

struct EndedSession(PathBuf);

impl AgentSession for EndedSession {
    fn send_turn(&self, _: &super::super::Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> super::super::AgentStatus {
        super::super::AgentStatus::Ended { code: Some(0) }
    }
    fn quiet_for(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
    fn exited_within(&self, _: std::time::Duration) -> bool {
        true
    }
    fn end(&self) {}
    fn conversation_artifacts(&self) -> Vec<PathBuf> {
        vec![self.0.clone()]
    }
    fn backdate_last_output(&self, _: std::time::Duration) {}
}

#[test]
fn cleanup_stages_only_the_closed_sessions_compaction_sidecar_and_restores_on_failure() {
    let f = Fixture::new();
    let exact = f.state.path().join("harness/claude/compactions/own.jsonl");
    let sibling = exact.parent().unwrap().join("another.jsonl");
    write(&exact, "old compaction boundaries");
    write(&sibling, "unrelated compaction boundaries");
    let mut cleanup = f.prepare(AgentProvider::Claude, &[]).unwrap();
    cleanup
        .stage_session(f.state.path(), &EndedSession(exact.clone()))
        .unwrap();
    assert!(!exact.exists());
    assert!(sibling.exists());
    drop(cleanup);
    assert!(exact.exists());
    let mut cleanup = f.prepare(AgentProvider::Claude, &[]).unwrap();
    cleanup
        .stage_session(f.state.path(), &EndedSession(exact.clone()))
        .unwrap();
    cleanup.commit().unwrap();
    assert!(!exact.exists());
    assert!(sibling.exists());
}

#[test]
fn cleanup_refuses_a_session_sidecar_outside_private_state() {
    let f = Fixture::new();
    let outside = f.home.path().join("unrelated.jsonl");
    write(&outside, "keep");
    let mut cleanup = f.prepare(AgentProvider::Claude, &[]).unwrap();
    assert!(cleanup
        .stage_session(f.state.path(), &EndedSession(outside.clone()))
        .is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
}

#[test]
fn a_filesystem_stage_failure_restores_previously_staged_provider_artifacts() {
    let f = Fixture::new();
    let claude = f.claude(SESSION);
    let codex = f.codex(SESSION);
    write(&claude, "old Claude session");
    f.write_rollout(&codex, f.cwd.path(), SESSION);
    let directory = codex.parent().unwrap();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).unwrap();
    let result = f.prepare(
        AgentProvider::Claude,
        &[
            f.lineage(AgentProvider::Claude, SESSION),
            f.lineage(AgentProvider::Codex, SESSION),
        ],
    );
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result.is_err(),
        "the unreadable cleanup destination refuses the reset"
    );
    assert_eq!(fs::read_to_string(claude).unwrap(), "old Claude session");
    assert!(codex.exists());
}

#[test]
fn a_stopped_pty_session_carries_only_its_exact_compaction_sidecar() {
    let f = Fixture::new();
    let exact = f.state.path().join("harness/claude/compactions/own.jsonl");
    let spec = crate::pty::HarnessSpec::new("/bin/sh")
        .arg("-c")
        .arg("read line")
        .env("HOME", f.home.path().to_string_lossy())
        .unset_all(super::super::DAEMON_IDENTITY_VARS)
        .compaction_sidecar(exact.clone());
    let session = crate::pty::PtySession::spawn(
        &spec,
        Some(f.cwd.path().to_path_buf()),
        portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        },
    )
    .unwrap();
    assert_eq!(session.conversation_artifacts(), vec![exact.clone()]);
    let mut cleanup = f.prepare(AgentProvider::Claude, &[]).unwrap();
    assert!(cleanup.stage_session(f.state.path(), &session).is_err());
    session.end();
    cleanup.stage_session(f.state.path(), &session).unwrap();
    cleanup.commit().unwrap();
    assert!(!exact.exists());
}

#[test]
fn native_history_keys_group_carriers_by_actual_transcript_storage() {
    for providers in [
        [AgentProvider::Claude, AgentProvider::ClaudeAdk],
        [AgentProvider::Codex, AgentProvider::CodexAppServer],
    ] {
        assert_eq!(
            harness_for(providers[0]).native_history_namespace(),
            harness_for(providers[1]).native_history_namespace()
        );
    }
    assert_ne!(
        harness_for(AgentProvider::Claude).native_history_namespace(),
        harness_for(AgentProvider::Codex).native_history_namespace()
    );
    assert_ne!(
        harness_for(AgentProvider::Pi).native_history_namespace(),
        harness_for(AgentProvider::Claude).native_history_namespace()
    );
}

#[test]
fn native_history_cwd_keys_find_codex_aliases_and_claude_encoded_collisions() {
    let f = Fixture::new();
    let alias = f.home.path().join("cwd-alias");
    symlink(f.cwd.path(), &alias).unwrap();
    for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
        let harness = harness_for(provider);
        assert_eq!(
            harness.native_history_cwd(&alias),
            harness.native_history_cwd(f.cwd.path())
        );
    }
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        let harness = harness_for(provider);
        assert_eq!(
            harness.native_history_cwd(Path::new("/projects/a-b")),
            harness.native_history_cwd(Path::new("/projects/a/b"))
        );
    }
}
