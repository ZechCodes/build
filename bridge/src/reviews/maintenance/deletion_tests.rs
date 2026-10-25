//! Real native Git writers, including a writer already inside the bare gitdir.
use super::{
    admission,
    deletion::{self, DeletionStep},
};
use crate::git_fixture::{git_command, git_in, init_repo};
use crate::reviews::receivers::{ensure_receiver, plan_receiver, ReviewReceiver};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const FOREIGN_REF: &str = "refs/heads/user-race";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    receiver: ReviewReceiver,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let (temporary, source) = init_repo();
        let root = temporary.path().join("receivers");
        fs::create_dir_all(root.join("operations")).unwrap();
        let receiver = plan_receiver(&source, &root).unwrap();
        ensure_receiver(&receiver).unwrap();
        let head = git2::Repository::open(&source)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap()
            .to_string();
        git_in(
            &receiver.path,
            &[
                "fetch",
                "--no-write-fetch-head",
                source.to_str().unwrap(),
                &head,
            ],
        );
        Self {
            _temporary: temporary,
            root,
            source,
            receiver,
            head,
        }
    }
}

struct GateLibrary {
    _temporary: tempfile::TempDir,
    path: PathBuf,
}

fn gate_library() -> &'static Path {
    static LIBRARY: OnceLock<GateLibrary> = OnceLock::new();
    &LIBRARY
        .get_or_init(|| {
            let temporary = tempfile::tempdir().unwrap();
            let source = temporary.path().join("native_ref_gate.c");
            let path = temporary.path().join("native_ref_gate.so");
            fs::write(&source, include_str!("native_ref_gate.c")).unwrap();
            let output = Command::new("cc")
                .args(["-shared", "-fPIC"])
                .arg(source)
                .arg("-o")
                .arg(&path)
                .arg("-ldl")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            GateLibrary {
                _temporary: temporary,
                path,
            }
        })
        .path
}

struct GatedGit {
    child: Option<Child>,
    release: PathBuf,
}

impl GatedGit {
    fn start(fixture: &Fixture, from_ref_directory: bool, after_lock: bool) -> Self {
        let signal = fixture
            ._temporary
            .path()
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&signal).unwrap();
        let ready = signal.join("ready");
        let release = signal.join("release");
        let working = if from_ref_directory {
            fixture.receiver.path.join("refs/heads")
        } else {
            fixture.receiver.path.clone()
        };
        fs::create_dir_all(&working).unwrap();
        let child = git_command(&working, &["update-ref", FOREIGN_REF, &fixture.head])
            .env("LD_PRELOAD", gate_library())
            .env(
                "BUILD_RECEIVER_GATE_PHASE",
                if after_lock { "after" } else { "before" },
            )
            .env("BUILD_RECEIVER_GATE_SUFFIX", format!("{FOREIGN_REF}.lock"))
            .env("BUILD_RECEIVER_GATE_READY", &ready)
            .env("BUILD_RECEIVER_GATE_RELEASE", &release)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut gated = Self {
            child: Some(child),
            release,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            if let Some(status) = gated.child.as_mut().unwrap().try_wait().unwrap() {
                panic!("native Git exited before its ref lock gate: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "native Git did not reach ref lock gate"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        gated
    }

    fn release(&mut self) -> Output {
        fs::write(&self.release, "continue\n").unwrap();
        self.child.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for GatedGit {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn find_file(directory: &Path, name: &str, expected: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(directory) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_dir() {
            find_file(&entry.path(), name, expected)
        } else {
            kind.is_file()
                && entry.file_name() == name
                && fs::read(entry.path()).is_ok_and(|bytes| bytes == expected)
        }
    })
}

fn has_quarantined_commit(fixture: &Fixture) -> bool {
    let head = git2::Oid::from_str(&fixture.head).unwrap();
    fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("receiver-deleting-")
                && git2::Repository::open_bare(entry.path())
                    .is_ok_and(|repository| repository.find_commit(head).is_ok())
        })
}

#[test]
fn closed_receiver_hooks_reject_native_update_and_receive_pack() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    assert!(fixture.receiver.path.join(admission::MARKER).is_file());
    let update = git_command(
        &fixture.receiver.path,
        &["update-ref", FOREIGN_REF, &fixture.head],
    )
    .output()
    .unwrap();
    assert!(
        !update.status.success(),
        "closed receiver accepted a fresh native update-ref"
    );
    let push = git_command(
        &fixture.source,
        &[
            "push",
            fixture.receiver.path.to_str().unwrap(),
            "main:refs/heads/user-push",
        ],
    )
    .output()
    .unwrap();
    assert!(
        !push.status.success(),
        "closed receiver accepted receive-pack"
    );
    let repository = git2::Repository::open_bare(&fixture.receiver.path).unwrap();
    assert_eq!(repository.references().unwrap().count(), 0);
}

#[test]
fn closure_grace_is_durable_and_does_not_reset() {
    assert_eq!(admission::ADMISSION_DRAIN_SECS, 3600);
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    let marker = fs::read(fixture.receiver.path.join(admission::MARKER)).unwrap();
    assert!(
        !admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS - 1).unwrap()
    );
    assert_eq!(
        fs::read(fixture.receiver.path.join(admission::MARKER)).unwrap(),
        marker
    );
    assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
}

#[test]
fn admission_drain_includes_nested_object_locks() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    let lock = fixture
        .receiver
        .path
        .join("objects/pack/multi-pack-index.lock");
    fs::write(&lock, b"native object maintenance").unwrap();
    assert!(!admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
    fs::remove_file(lock).unwrap();
    assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
}

#[test]
fn closure_waits_for_a_preexisting_native_ref_lock() {
    let fixture = Fixture::new();
    let mut git = GatedGit::start(&fixture, false, true);
    let lock = fixture.receiver.path.join(format!("{FOREIGN_REF}.lock"));
    assert!(
        lock.is_file(),
        "after-open gate must hold a real native Git lock"
    );
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    assert!(
        !admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap(),
        "elapsed grace cannot bypass an in-flight native lock"
    );
    let output = git.release();
    assert!(!lock.exists());
    if output.status.success() {
        assert!(
            admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).is_err(),
            "pre-marker publication must preserve changed refs"
        );
        assert!(find_file(
            &fixture.root,
            "user-race",
            format!("{}\n", fixture.head).as_bytes()
        ));
    } else {
        assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
    }
}

#[test]
fn cached_absolute_git_reappearance_preserves_canonical_and_quarantined_data() {
    let fixture = Fixture::new();
    let mut git = GatedGit::start(&fixture, true, false);
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
    let mut output = None;
    let result =
        deletion::prepare_with_checkpoint(&fixture.root, &fixture.receiver.path, |step, _, _| {
            if matches!(step, DeletionStep::Quarantined) {
                output = Some(git.release());
            }
            Ok(())
        });
    let output = output.expect("must exercise quarantine checkpoint");
    assert!(
        output.status.success(),
        "must exercise cached pre-marker native configuration: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reported = match result {
        Err(_) => true,
        Ok(job) => job.finish().is_err(),
    };
    assert!(
        reported,
        "canonical reappearance must be reported and preserved"
    );
    assert!(find_file(
        &fixture.receiver.path,
        "user-race",
        format!("{}\n", fixture.head).as_bytes()
    ));
    assert!(
        has_quarantined_commit(&fixture),
        "existing quarantine objects must survive canonical reappearance"
    );
}

#[test]
fn cached_absolute_git_after_deletion_is_preserved_on_next_attempt() {
    let fixture = Fixture::new();
    let mut git = GatedGit::start(&fixture, true, false);
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
    deletion::prepare(&fixture.root, &fixture.receiver.path)
        .unwrap()
        .finish()
        .unwrap();
    let output = git.release();
    assert!(
        output.status.success(),
        "must exercise accepted pre-marker cached-path residual: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        deletion::prepare(&fixture.root, &fixture.receiver.path).is_err(),
        "recreated canonical path has no valid ownership and must be preserved"
    );
    assert!(find_file(
        &fixture.receiver.path,
        "user-race",
        format!("{}\n", fixture.head).as_bytes()
    ));
}

#[test]
fn approved_deletion_uses_a_stable_lease_across_journal_replacement() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    let job = deletion::prepare(&fixture.root, &fixture.receiver.path).unwrap();
    assert!(
        deletion::resume(&fixture.root, None).unwrap().is_none(),
        "replaced journal must not bypass the live deletion lease"
    );
    drop(job);
    let resumed = deletion::resume(&fixture.root, None)
        .unwrap()
        .expect("dropped job must release its stable lease");
    resumed.finish().unwrap();
}

#[test]
fn native_ref_after_inventory_scan_preserves_unapproved_quarantine_on_restart() {
    let fixture = Fixture::new();
    let mut written = false;
    let result =
        deletion::prepare_with_checkpoint(&fixture.root, &fixture.receiver.path, |step, _, _| {
            if matches!(step, DeletionStep::BeforeRename) {
                git_in(
                    &fixture.receiver.path,
                    &["update-ref", FOREIGN_REF, &fixture.head],
                );
                written = true;
            }
            Ok(())
        });
    assert!(written && result.is_err());
    assert!(deletion::resume(&fixture.root, None).unwrap().is_none());
    assert!(find_file(
        &fixture.root,
        "user-race",
        format!("{}\n", fixture.head).as_bytes()
    ));
    assert!(has_quarantined_commit(&fixture));
}

#[test]
fn a_new_opening_clears_closure_and_native_writes_work() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    ensure_receiver(&fixture.receiver).unwrap();
    assert!(!fixture.receiver.path.join(admission::MARKER).exists());
    git_in(
        &fixture.receiver.path,
        &["update-ref", FOREIGN_REF, &fixture.head],
    );
}

#[test]
fn closed_receiver_hooks_reject_fresh_native_update_from_ref_subdirectory() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    let working = fixture.receiver.path.join("refs/heads");
    let output = git_command(&working, &["update-ref", FOREIGN_REF, &fixture.head])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "fresh native Git below refs/ bypassed closed admission"
    );
    assert_eq!(
        git2::Repository::open_bare(&fixture.receiver.path)
            .unwrap()
            .references()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn after_marker_cwd_root_writer_is_rejected_after_quarantine_inventory() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    assert!(admission::ready(&fixture.receiver.path, admission::ADMISSION_DRAIN_SECS).unwrap());
    // Git starts after admission closes, caches a relative gitdir, then pauses
    // before creating its ref lock. Keep it paused through the final inventory.
    let mut git = GatedGit::start(&fixture, false, false);
    let job = deletion::prepare(&fixture.root, &fixture.receiver.path).unwrap();
    let output = git.release();
    assert!(
        !output.status.success(),
        "post-marker cwd-root Git bypassed admission after quarantine: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("closed for reclamation"));
    assert!(!find_file(
        &fixture.root,
        "user-race",
        format!("{}\n", fixture.head).as_bytes()
    ));
    job.finish().unwrap();
}

#[test]
fn quarantine_requires_the_close_marker_and_active_hooks() {
    for changed in [admission::MARKER, "hooks/reference-transaction", "config"] {
        let fixture = Fixture::new();
        assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
        let result = deletion::prepare_with_checkpoint(
            &fixture.root,
            &fixture.receiver.path,
            |step, quarantine, _| {
                if matches!(step, DeletionStep::Quarantined) {
                    if changed == "config" {
                        git_in(quarantine, &["config", "core.hooksPath", "/dev/null"]);
                    } else {
                        let path = quarantine.join(changed);
                        fs::remove_file(path).unwrap();
                    }
                }
                Ok(())
            },
        );
        assert!(result.is_err(), "quarantine accepted changed {changed}");
        assert!(has_quarantined_commit(&fixture));
        assert!(deletion::resume(&fixture.root, None).unwrap().is_none());
    }
}

#[test]
fn owned_legacy_absolute_hook_migration_restarts_the_full_admission_drain() {
    let fixture = Fixture::new();
    assert!(!admission::ready(&fixture.receiver.path, 0).unwrap());
    let marker = fs::read(fixture.receiver.path.join(admission::MARKER)).unwrap();
    let hooks = fixture.receiver.path.join("hooks");
    let legacy = fixture.receiver.path.join(".build-review-hooks");
    fs::create_dir_all(&legacy).unwrap();
    for name in ["reference-transaction", "pre-receive"] {
        let hook = hooks.join(name);
        if hook.exists() {
            fs::copy(hook, legacy.join(name)).unwrap();
        }
    }
    git_in(
        &fixture.receiver.path,
        &["config", "core.hooksPath", legacy.to_str().unwrap()],
    );
    let migrated_at = admission::ADMISSION_DRAIN_SECS;
    assert!(
        !admission::ready(&fixture.receiver.path, migrated_at).unwrap(),
        "previously ineffective absolute hooks cannot count toward the new admission drain"
    );
    let migrated = fs::read(fixture.receiver.path.join(admission::MARKER)).unwrap();
    assert_ne!(migrated, marker);
    let migrated: serde_json::Value = serde_json::from_slice(&migrated).unwrap();
    assert_eq!(migrated["closed_at"].as_u64(), Some(migrated_at));
    assert!(!admission::ready(
        &fixture.receiver.path,
        migrated_at + admission::ADMISSION_DRAIN_SECS - 1
    )
    .unwrap());
    assert!(admission::ready(
        &fixture.receiver.path,
        migrated_at + admission::ADMISSION_DRAIN_SECS
    )
    .unwrap());
    let configured = git_command(
        &fixture.receiver.path,
        &["config", "--get", "core.hooksPath"],
    )
    .output()
    .unwrap();
    assert!(
        !configured.status.success(),
        "legacy absolute hooksPath remains"
    );
    assert!(hooks.join("reference-transaction").is_file());
}

#[test]
fn inherited_hooks_override_preserves_receiver_before_admission_closes() {
    let fixture = Fixture::new();
    let included = fixture._temporary.path().join("included-git-config");
    fs::write(&included, "[core]\n\thooksPath = /dev/null\n").unwrap();
    git_in(
        &fixture.receiver.path,
        &["config", "include.path", included.to_str().unwrap()],
    );
    let original_config = fs::read(fixture.receiver.path.join("config")).unwrap();
    assert!(admission::ready(&fixture.receiver.path, 0).is_err());
    assert_eq!(
        fs::read(fixture.receiver.path.join("config")).unwrap(),
        original_config,
        "refusing an inherited hook override must preserve the original config byte-for-byte"
    );
    assert!(!fixture.receiver.path.join(admission::MARKER).exists());
    assert!(fixture.receiver.path.is_dir());
}
