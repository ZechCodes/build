//! Deterministic cross-process interleavings at the actual persistence boundary.
#[path = "../../../src/http/endpoint_state.rs"]
mod endpoint_state;

use endpoint_state::{EndpointPolicy, EndpointRecord};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

fn policy() -> EndpointPolicy {
    EndpointPolicy {
        device_id: "device".into(),
        issuer: "https://app.example".into(),
        origin: "https://bridge.example".into(),
        allowed_origins: vec!["https://app.example".into()],
        certificate_fingerprint: "certificate".into(),
    }
}

fn record(path: &Path) -> EndpointRecord {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

struct HeldTransaction {
    child: Child,
    output: BufReader<ChildStdout>,
}
impl HeldTransaction {
    fn start(path: &Path, action: &str, desired: &EndpointPolicy) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "state_transaction_actor",
                "--ignored",
                "--nocapture",
            ])
            .env("BUILD_HTTP_STATE_ACTOR", action)
            .env("BUILD_HTTP_STATE_PATH", path)
            .env(
                "BUILD_HTTP_STATE_POLICY",
                serde_json::to_string(desired).unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut transaction = Self { child, output };
        loop {
            let mut line = String::new();
            assert_ne!(
                transaction.output.read_line(&mut line).unwrap(),
                0,
                "child exited before persistence barrier"
            );
            if line.trim() == "STATE_TRANSACTION_PAUSED" {
                break;
            }
        }
        transaction
    }

    fn release(mut self) {
        self.child
            .stdin
            .take()
            .unwrap()
            .write_all(b"release\n")
            .unwrap();
        let mut output = String::new();
        self.output.read_to_string(&mut output).unwrap();
        assert!(
            self.child.wait().unwrap().success(),
            "child failed: {output}"
        );
    }

    fn crash(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for HeldTransaction {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pause_before_persist() {
    println!("STATE_TRANSACTION_PAUSED");
    std::io::stdout().flush().unwrap();
    let mut release = String::new();
    std::io::stdin().read_line(&mut release).unwrap();
}

#[test]
#[ignore = "child-process actor, explicitly invoked by the concurrency witnesses"]
fn state_transaction_actor() {
    let path = PathBuf::from(std::env::var_os("BUILD_HTTP_STATE_PATH").unwrap());
    let desired: EndpointPolicy =
        serde_json::from_str(&std::env::var("BUILD_HTTP_STATE_POLICY").unwrap()).unwrap();
    match std::env::var("BUILD_HTTP_STATE_ACTOR").unwrap().as_str() {
        "prepare" => {
            EndpointRecord::test_prepare_before_persist(&path, Some(desired), pause_before_persist)
                .unwrap();
        }
        "acknowledge" => {
            let current = EndpointRecord::prepare(&path, Some(desired))
                .unwrap()
                .unwrap();
            current
                .test_acknowledge_before_persist(&path, pause_before_persist)
                .unwrap();
        }
        _ => panic!("unknown test actor"),
    }
}

fn stale_acknowledgement_cannot_overwrite(desired: Option<EndpointPolicy>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    let original = EndpointRecord::prepare(&path, Some(policy()))
        .unwrap()
        .unwrap();
    let paused = HeldTransaction::start(&path, "acknowledge", &policy());
    let contender = EndpointRecord::prepare(&path, desired.clone());
    let during = record(&path);
    paused.release(); // always release before assertions, including the red run
    assert_eq!(
        contender.err().as_deref(),
        Some("HTTP endpoint state is busy")
    );
    assert_eq!(during.endpoint_epoch, original.endpoint_epoch);
    let replacement = EndpointRecord::prepare(&path, desired).unwrap().unwrap();
    assert_ne!(replacement.endpoint_epoch, original.endpoint_epoch);
    assert!(original.acknowledge(&path).is_err());
    assert_eq!(record(&path).endpoint_epoch, replacement.endpoint_epoch);
    assert_eq!(record(&path).enabled, replacement.enabled);
    assert_eq!(
        EndpointRecord::prepare(&path, Some(replacement.policy.clone()))
            .unwrap()
            .unwrap()
            .policy,
        replacement.policy
    );
}

#[test]
fn stale_acknowledgement_interleaved_with_disable_preserves_disabled_epoch() {
    stale_acknowledgement_cannot_overwrite(None);
}

#[test]
fn stale_acknowledgement_interleaved_with_policy_change_preserves_new_policy() {
    let mut changed = policy();
    changed.allowed_origins.push("https://other.example".into());
    stale_acknowledgement_cannot_overwrite(Some(changed));
}

#[test]
fn concurrent_prepares_cannot_return_distinct_epochs_for_the_same_intent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    let paused = HeldTransaction::start(&path, "prepare", &policy());
    let contender = EndpointRecord::prepare(&path, Some(policy()));
    paused.release();
    assert_eq!(
        contender.err().as_deref(),
        Some("HTTP endpoint state is busy")
    );
    let committed = record(&path);
    let retry = EndpointRecord::prepare(&path, Some(policy()))
        .unwrap()
        .unwrap();
    assert_eq!(retry.endpoint_epoch, committed.endpoint_epoch);
}

#[cfg(unix)]
#[test]
fn parent_aliases_share_process_lock_and_crash_releases_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    EndpointRecord::prepare(&path, Some(policy())).unwrap();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(directory.path(), &alias).unwrap();
    let paused = HeldTransaction::start(&path, "acknowledge", &policy());
    let contender = EndpointRecord::prepare(&alias.join("endpoint.json"), None);
    paused.crash();
    assert_eq!(
        contender.err().as_deref(),
        Some("HTTP endpoint state is busy")
    );
    let disabled = EndpointRecord::prepare(&path, None).unwrap().unwrap();
    assert!(!disabled.enabled);
}

#[test]
fn read_and_persist_failures_release_lock_and_remove_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    std::fs::write(&path, b"invalid JSON").unwrap();
    assert!(EndpointRecord::prepare(&path, Some(policy())).is_err());
    std::fs::remove_file(&path).unwrap();
    let failure = EndpointRecord::test_prepare_before_persist(&path, Some(policy()), || {
        std::fs::create_dir(&path).unwrap(); // deterministic rename failure after acquiring ownership
    });
    assert_eq!(
        failure.err().as_deref(),
        Some("cannot replace HTTP endpoint state")
    );
    std::fs::remove_dir(&path).unwrap();
    assert!(std::fs::read_dir(directory.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
    EndpointRecord::prepare(&path, Some(policy()))
        .unwrap()
        .unwrap()
        .acknowledge(&path)
        .unwrap();
}

#[cfg(unix)]
#[test]
fn lock_inode_survives_record_renames_and_rejects_final_component_aliases() {
    use std::os::unix::fs::MetadataExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    let original = EndpointRecord::prepare(&path, Some(policy()))
        .unwrap()
        .unwrap();
    let lock_path = directory.path().join("endpoint.json.lock");
    let identity = |path: &Path| {
        let value = std::fs::metadata(path).unwrap();
        (value.dev(), value.ino())
    };
    let lock_inode = identity(&lock_path);
    original.acknowledge(&path).unwrap();
    EndpointRecord::prepare(&path, None).unwrap();
    assert_eq!(identity(&lock_path), lock_inode);
    let alias = directory.path().join("record-alias.json");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(EndpointRecord::prepare(&alias, Some(policy())).is_err());
    std::fs::remove_file(&alias).unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(EndpointRecord::prepare(&alias, Some(policy())).is_err());
    std::fs::remove_file(&alias).unwrap();
    EndpointRecord::prepare(&path, Some(policy())).unwrap();
    assert_eq!(identity(&lock_path), lock_inode);
}

#[cfg(unix)]
#[test]
fn unusable_or_aliased_lock_fails_closed_without_writing_record() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    let lock = directory.path().join("endpoint.json.lock");
    let other = directory.path().join("other");
    std::fs::write(&other, b"").unwrap();
    std::os::unix::fs::symlink(&other, &lock).unwrap();
    assert!(EndpointRecord::prepare(&path, Some(policy())).is_err());
    assert!(!path.exists());
    std::fs::remove_file(&lock).unwrap();
    std::fs::hard_link(&other, &lock).unwrap();
    assert!(EndpointRecord::prepare(&path, Some(policy())).is_err());
    assert!(!path.exists());
    std::fs::remove_file(&lock).unwrap();
    EndpointRecord::prepare(&path, Some(policy())).unwrap();
}

#[test]
fn transaction_drop_unlocks_even_while_a_duplicate_file_description_survives() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("endpoint.json");
    // A fork before close-on-exec can temporarily retain this same description.
    let duplicate = EndpointRecord::test_released_lock_clone(&path).unwrap();
    EndpointRecord::prepare(&path, Some(policy())).unwrap();
    drop(duplicate);
    EndpointRecord::prepare(&path, None).unwrap();
}
