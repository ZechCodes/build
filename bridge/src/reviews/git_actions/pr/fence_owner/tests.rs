use super::*;
use crate::git_fixture::{git_command, init_repo};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "reviews::git_actions::pr::fence_owner::tests::crashing_owner_child";

#[test]
fn crashing_owner_child() {
    let Ok(repository) = std::env::var("FENCE_TEST_REPOSITORY") else {
        return;
    };
    let signal = PathBuf::from(std::env::var("FENCE_TEST_SIGNAL").unwrap());
    let fence = OwnedFence::create(Path::new(&repository)).unwrap();
    let live = std::env::var("FENCE_TEST_LIVE").is_ok();
    let descendant = std::env::var("FENCE_TEST_DESCENDANT").is_ok();
    let script = if descendant {
        format!("#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\nsleep 60 &\nprintf '%s' \"$!\" > '{}.background'\n", signal.display())
    } else if live {
        format!("#!/bin/sh\n[ \"$1\" = prepared ] || exit 0\nprintf ready > '{}.ready'\nwhile [ ! -f '{}.release' ]; do sleep 0.05; done\n", signal.display(), signal.display())
    } else {
        "#!/bin/sh\nexit 0\n".into()
    };
    fs::write(fence.path().join("reference-transaction"), script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        fence.path().join("reference-transaction"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fence.seal().unwrap();
    if std::env::var("FENCE_TEST_TORN_FRAME").is_ok() {
        use std::io::Write;
        let mut marker = fs::OpenOptions::new()
            .append(true)
            .open(fence.path().join(MARKER))
            .unwrap();
        marker.write_all(b"{\"partial_launch_frame\":").unwrap();
        marker.sync_all().unwrap();
    }
    if std::env::var("FENCE_TEST_UNVERIFIED").is_ok() {
        fence.begin_launch().unwrap();
    }
    let mut pid = 0;
    if live || descendant {
        fence.begin_launch().unwrap();
        let hooks = format!("core.hooksPath={}", fence.path().display());
        let mut command = git_command(
            Path::new(&repository),
            &["-c", &hooks, "update-ref", "refs/heads/fence-test", "HEAD"],
        );
        use std::os::unix::process::CommandExt;
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        pid = child.id();
        fence.observe(GitProcessEvent::Started(pid)).unwrap();
        if descendant {
            assert!(child.wait().unwrap().success());
        } else {
            let ready = signal.with_extension("ready");
            wait_until(|| ready.exists());
        }
        std::mem::forget(child);
    }
    fs::write(signal, format!("{}\n{pid}", fence.path().display())).unwrap();
    std::process::exit(77);
}

fn wait_until(ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < deadline, "child reached its checkpoint");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn crash(repository: &Path, signal: &Path, live: bool) -> (PathBuf, u32) {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", CHILD, "--nocapture"])
        .env("FENCE_TEST_REPOSITORY", repository)
        .env("FENCE_TEST_SIGNAL", signal);
    if live {
        child.env("FENCE_TEST_LIVE", "true");
    }
    assert_eq!(child.status().unwrap().code(), Some(77));
    let data = fs::read_to_string(signal).unwrap();
    let (path, pid) = data.split_once('\n').unwrap();
    (PathBuf::from(path), pid.parse().unwrap())
}

fn tidy_test_fence(path: &Path) {
    let _ = fs::remove_file(path.join("reference-transaction"));
    let _ = fs::remove_dir(path);
}

#[test]
fn subprocess_crash_is_recovered_from_registered_ownership() {
    let (home, repository) = init_repo();
    let (path, _) = crash(&repository, &home.path().join("signal"), false);
    recover_fences(&repository).unwrap();
    let recovered = !path.exists();
    tidy_test_fence(&path);
    assert!(recovered, "a killed bridge leaves recoverable owned hooks");
}

#[test]
fn registering_hooks_waits_for_short_registry_contention() {
    let (_home, repository) = init_repo();
    let fence = OwnedFence::create(&repository).unwrap();
    let held = registry_lease(&fence.registry).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| send.send(fence.seal()).unwrap());
        let early = receive.recv_timeout(Duration::from_millis(50));
        let waited = matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout));
        drop(held);
        if waited {
            assert_eq!(
                receive.recv_timeout(Duration::from_secs(5)).unwrap(),
                Ok(())
            );
        }
        assert!(
            waited,
            "registry contention must wait instead of failing: {early:?}"
        );
    });
}

#[test]
#[cfg(target_os = "linux")]
fn surviving_git_child_keeps_hooks_until_its_group_finishes() {
    let (home, repository) = init_repo();
    let signal = home.path().join("signal");
    let (path, pid) = crash(&repository, &signal, true);
    recover_fences(&repository).unwrap();
    assert!(path.join("reference-transaction").exists());
    fs::write(signal.with_extension("release"), "release").unwrap();
    wait_until(|| {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| stat.rsplit_once(')').unwrap().1.split_whitespace().next() == Some("Z"))
            .unwrap_or(true)
    });
    recover_fences(&repository).unwrap();
    let recovered = !path.exists();
    tidy_test_fence(&path);
    assert!(recovered, "dead actual target Git child releases its hooks");
}

#[test]
#[cfg(target_os = "linux")]
fn surviving_hook_descendant_keeps_the_fence_after_git_has_exited() {
    let (home, repository) = init_repo();
    let signal = home.path().join("signal");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", CHILD, "--nocapture"])
        .env("FENCE_TEST_REPOSITORY", &repository)
        .env("FENCE_TEST_SIGNAL", &signal)
        .env("FENCE_TEST_DESCENDANT", "true");
    assert_eq!(child.status().unwrap().code(), Some(77));
    let raw = fs::read_to_string(&signal).unwrap();
    let (path, group) = raw.split_once('\n').unwrap();
    let path = PathBuf::from(path);
    let group: i32 = group.parse().unwrap();
    recover_fences(&repository).unwrap();
    assert!(path.join("reference-transaction").exists());
    // SAFETY: the test owns this dedicated Git process group.
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    wait_until(|| process::Process::capture(group as u32).is_err());
    let deadline = Instant::now() + Duration::from_secs(8);
    while path.exists() {
        recover_fences(&repository).unwrap();
        assert!(
            Instant::now() < deadline,
            "dead hook descendant releases its fence"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn ordinary_drop_removes_owned_symlinks_and_keeps_original_resources() {
    let (home, repository) = init_repo();
    let original = home.path().join("user-resource");
    fs::write(&original, "user resource").unwrap();
    let path = {
        let fence = OwnedFence::create(&repository).unwrap();
        fs::write(fence.path().join("reference-transaction"), "generated hook").unwrap();
        std::os::unix::fs::symlink(&original, fence.path().join("resource")).unwrap();
        fence.seal().unwrap();
        fence.path().to_path_buf()
    };
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(original).unwrap(), "user resource");
}

#[test]
fn unknown_entries_preserve_the_entire_crashed_fence() {
    let (home, repository) = init_repo();
    let (path, _) = crash(&repository, &home.path().join("signal"), false);
    fs::write(path.join("foreign"), "foreign data").unwrap();
    recover_fences(&repository).unwrap();
    assert!(path.join("reference-transaction").exists());
    assert_eq!(
        fs::read_to_string(path.join("foreign")).unwrap(),
        "foreign data"
    );
}

#[test]
fn replaced_hook_inode_and_content_are_preserved() {
    let (home, repository) = init_repo();
    let (path, _) = crash(&repository, &home.path().join("signal"), false);
    fs::remove_file(path.join("reference-transaction")).unwrap();
    fs::write(path.join("reference-transaction"), "replacement hook").unwrap();
    recover_fences(&repository).unwrap();
    assert_eq!(
        fs::read_to_string(path.join("reference-transaction")).unwrap(),
        "replacement hook"
    );
}

#[test]
fn replaced_marker_with_the_same_record_is_preserved() {
    let (home, repository) = init_repo();
    let (path, _) = crash(&repository, &home.path().join("signal"), false);
    let marker = path.join(MARKER);
    let content = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    fs::write(&marker, &content).unwrap();
    recover_fences(&repository).unwrap();
    assert_eq!(fs::read(marker).unwrap(), content);
    assert!(path.join("reference-transaction").exists());
}

#[test]
fn replaced_fence_directory_is_preserved_without_following_it() {
    let (home, repository) = init_repo();
    let (path, _) = crash(&repository, &home.path().join("signal"), false);
    let original = home.path().join("original-fence");
    fs::rename(&path, &original).unwrap();
    fs::create_dir(&path).unwrap();
    fs::write(path.join("foreign"), "foreign data").unwrap();
    recover_fences(&repository).unwrap();
    assert_eq!(
        fs::read_to_string(path.join("foreign")).unwrap(),
        "foreign data"
    );
    assert!(original.join("reference-transaction").exists());
}

#[test]
fn crash_between_spawn_intent_and_child_observation_preserves_hooks() {
    let (home, repository) = init_repo();
    let signal = home.path().join("signal");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", CHILD, "--nocapture"])
        .env("FENCE_TEST_REPOSITORY", &repository)
        .env("FENCE_TEST_SIGNAL", &signal)
        .env("FENCE_TEST_UNVERIFIED", "true");
    assert_eq!(child.status().unwrap().code(), Some(77));
    let raw = fs::read_to_string(&signal).unwrap();
    let path = PathBuf::from(raw.split_once('\n').unwrap().0);
    recover_fences(&repository).unwrap();
    assert!(path.join("reference-transaction").exists());
}

#[test]
fn crash_during_a_marker_append_recovers_the_last_complete_ownership_frame() {
    let (home, repository) = init_repo();
    let signal = home.path().join("signal");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", CHILD, "--nocapture"])
        .env("FENCE_TEST_REPOSITORY", &repository)
        .env("FENCE_TEST_SIGNAL", &signal)
        .env("FENCE_TEST_TORN_FRAME", "true");
    assert_eq!(child.status().unwrap().code(), Some(77));
    let raw = fs::read_to_string(&signal).unwrap();
    let path = PathBuf::from(raw.split_once('\n').unwrap().0);
    recover_fences(&repository).unwrap();
    assert!(
        !path.exists(),
        "a torn later append preserves the last valid sealed ownership"
    );
}

#[test]
fn replacing_the_installed_hook_directory_prevents_git_launch() {
    let (home, repository) = init_repo();
    let fence = OwnedFence::create(&repository).unwrap();
    fs::write(fence.path().join("reference-transaction"), "generated hook").unwrap();
    fence.seal().unwrap();
    fs::rename(fence.path(), home.path().join("renamed-original")).unwrap();
    fs::create_dir(fence.path()).unwrap();
    fs::write(fence.path().join("reference-transaction"), "foreign hook").unwrap();
    assert!(
        fence.begin_launch().is_err(),
        "Git must never receive a replaced hook directory"
    );
    assert_eq!(
        fs::read_to_string(fence.path().join("reference-transaction")).unwrap(),
        "foreign hook"
    );
}
