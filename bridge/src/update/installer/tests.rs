use super::*;

fn fixture(dir: &Path) -> Job {
    let home = dir.to_path_buf();
    fs::create_dir_all(home.join(".build")).unwrap();
    let installed_binary = home.join("build-bridge");
    fs::write(
        &installed_binary,
        b"#!/bin/sh\nprintf 'build-bridge 0.2.0\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&installed_binary, fs::Permissions::from_mode(0o700)).unwrap();
    super::super::provenance::write_marker(&home, &installed_binary).unwrap();
    let tasks_dir = home.join("tasks");
    fs::create_dir(&tasks_dir).unwrap();
    fs::write(tasks_dir.join("build.db"), b"old store").unwrap();
    let staged_binary = home.join("staged-build-bridge");
    fs::write(
        &staged_binary,
        b"#!/bin/sh\nprintf 'build-bridge 9.9.9\\n'\n",
    )
    .unwrap();
    fs::set_permissions(&staged_binary, fs::Permissions::from_mode(0o700)).unwrap();
    let staged_digest = super::super::provenance::binary_digest(&staged_binary).unwrap();
    Job {
        nonce: "attempt".into(),
        running_pid: std::process::id(),
        staged_digest,
        version: "9.9.9".into(),
        installed_binary,
        staged_binary,
        tasks_dir,
        home,
        uid: "1".into(),
    }
}

#[test]
fn swap_success_keeps_recoverable_original_and_updates_marker() {
    let dir = tempfile::tempdir().unwrap();
    let job = fixture(dir.path());
    let mut actions = Vec::new();
    install_with(
        &job,
        dir.path(),
        &job.installed_binary,
        &mut |action| {
            actions.push(action.to_string());
            Ok(())
        },
        &mut |job, _| {
            assert!(fs::read_to_string(&job.installed_binary)
                .unwrap()
                .contains("9.9.9"));
            fs::write(job.tasks_dir.join("build.db"), b"candidate store").unwrap();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(actions, ["stop", "start"]);
    assert_eq!(
        fs::read(job.tasks_dir.join("build.db")).unwrap(),
        b"candidate store"
    );
    assert_eq!(
        fs::read(
            job.tasks_dir
                .with_extension("bridge-update-attempt")
                .join("build.db")
        )
        .unwrap(),
        b"old store"
    );
    assert!(
        fs::read_to_string(super::super::provenance::marker_path(&job.home))
            .unwrap()
            .contains(&super::super::provenance::binary_digest(&job.installed_binary).unwrap())
    );
}

#[test]
fn failed_health_restores_binary_store_and_marker() {
    let dir = tempfile::tempdir().unwrap();
    let job = fixture(dir.path());
    let original_digest = super::super::provenance::binary_digest(&job.installed_binary).unwrap();
    let mut actions = Vec::new();
    let error = install_with(
        &job,
        dir.path(),
        &job.installed_binary,
        &mut |action| {
            actions.push(action.to_string());
            Ok(())
        },
        &mut |job, _| {
            fs::write(job.tasks_dir.join("build.db"), b"candidate store").unwrap();
            Err("unhealthy".into())
        },
    )
    .unwrap_err();
    assert_eq!(error.message(), "unhealthy");
    assert_eq!(actions, ["stop", "start", "stop", "start"]);
    assert_eq!(
        super::super::provenance::binary_digest(&job.installed_binary).unwrap(),
        original_digest
    );
    assert_eq!(
        fs::read(job.tasks_dir.join("build.db")).unwrap(),
        b"old store"
    );
    assert_eq!(
        fs::read(
            job.tasks_dir
                .with_extension("rejected-update-attempt")
                .join("build.db")
        )
        .unwrap(),
        b"candidate store"
    );
    assert!(
        fs::read_to_string(super::super::provenance::marker_path(&job.home))
            .unwrap()
            .contains(&original_digest)
    );
}

#[test]
fn stale_or_wrong_version_heartbeat_cannot_pass() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("bridge");
    fs::write(&binary, b"binary").unwrap();
    let job = Job {
        nonce: "expected".into(),
        running_pid: std::process::id(),
        staged_digest: String::new(),
        version: "1.2.3".into(),
        installed_binary: binary.clone(),
        staged_binary: dir.path().join("staged"),
        tasks_dir: dir.path().join("tasks"),
        home: dir.path().into(),
        uid: "1".into(),
    };
    let mut beat = Health {
        nonce: "wrong".into(),
        version: "1.2.3".into(),
        pid: std::process::id(),
        binary,
        timestamp: now(),
    };
    assert!(!valid_health(&job, &beat));
    beat.nonce = job.nonce.clone();
    assert!(valid_health(&job, &beat));
    beat.timestamp = 1;
    assert!(!valid_health(&job, &beat));
}

#[test]
fn stopped_store_snapshot_is_independent() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("build.db"), b"before").unwrap();
    copy_tree(&source, &target).unwrap();
    fs::write(target.join("build.db"), b"after").unwrap();
    assert_eq!(fs::read(source.join("build.db")).unwrap(), b"before");
}

#[test]
fn manager_pid_must_match_before_any_swap() {
    assert_eq!(parse_manager_pid("1234\n", "linux"), Some(1234));
    assert_eq!(
        parse_manager_pid("state = running\n    pid = 5678\n", "macos"),
        Some(5678)
    );
    let dir = tempfile::tempdir().unwrap();
    let mut job = fixture(dir.path());
    job.running_pid = u32::MAX;
    assert!(install(&job, dir.path()).is_err());
    assert!(!dir.path().join("previous-build-bridge").exists());
    assert_eq!(
        fs::read(job.tasks_dir.join("build.db")).unwrap(),
        b"old store"
    );
}

#[test]
fn launchd_stop_failure_is_only_tolerated_without_a_loaded_process() {
    assert!(stop_failure_is_absent("stop", "macos", Some(None)));
    assert!(!stop_failure_is_absent("stop", "macos", Some(Some(42))));
    assert!(!stop_failure_is_absent("start", "macos", Some(None)));
}

#[test]
fn completed_job_restart_clears_own_marker_after_shared_result_was_consumed() {
    let dir = tempfile::tempdir().unwrap();
    let job = fixture(dir.path());
    fs::create_dir_all(updates_dir(&job.home)).unwrap();
    write_json(&job_file(dir.path()), &job).unwrap();
    write_json(&active_path(&job.home), &job).unwrap();
    fs::write(dir.path().join("completed"), b"done").unwrap();
    assert!(!result_path(&job.home).exists());
    let old_digest = super::super::provenance::binary_digest(&job.installed_binary).unwrap();
    run_helper(dir.path()).unwrap();
    assert!(!active_path(&job.home).exists());
    assert_eq!(
        super::super::provenance::binary_digest(&job.installed_binary).unwrap(),
        old_digest
    );
    assert_eq!(
        fs::read(job.tasks_dir.join("build.db")).unwrap(),
        b"old store"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn uncertain_launcher_keeps_active_attempt_while_real_helper_process_runs() {
    let dir = tempfile::tempdir().unwrap();
    if std::env::var_os("BUILD_UPDATE_LAUNCH_TEST_CHILD").is_some() {
        let mut job = fixture(dir.path());
        job.staged_digest = super::super::provenance::binary_digest(&job.staged_binary).unwrap();
        let service = job.home.join(".config/systemd/user/build-bridge.service");
        fs::create_dir_all(service.parent().unwrap()).unwrap();
        fs::write(
            &service,
            format!("ExecStart=\"{}\" serve\n", job.installed_binary.display()),
        )
        .unwrap();
        write_json(&job_file(dir.path()), &job).unwrap();
        fs::create_dir_all(updates_dir(&job.home)).unwrap();
        assert!(launch(dir.path()).is_err());
        assert!(probation_active(&job.home));
        assert_eq!(active_attempt(&job.home).unwrap(), Some(job.nonce));
        return;
    }
    let fake_path = dir.path().join("bin");
    fs::create_dir(&fake_path).unwrap();
    let pid_file = dir.path().join("helper.pid");
    let launcher = fake_path.join("systemd-run");
    fs::write(
        &launcher,
        b"#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"$BUILD_UPDATE_HELPER_PID\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("update::installer::tests::uncertain_launcher_keeps_active_attempt_while_real_helper_process_runs")
            .env("BUILD_UPDATE_LAUNCH_TEST_CHILD", "1")
            .env("BUILD_UPDATE_HELPER_PID", &pid_file)
            .env("PATH", format!("{}:/usr/bin:/bin", fake_path.display()))
            .status()
            .unwrap();
    let pid: i32 = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    unsafe { libc::kill(pid, libc::SIGTERM) };
    assert!(status.success());
}

#[test]
fn changed_staged_bytes_are_rejected_before_version_probe_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut job = fixture(dir.path());
    job.staged_digest = super::super::provenance::binary_digest(&job.staged_binary).unwrap();
    let probe_marker = dir.path().join("probe-ran");
    fs::write(
        &job.staged_binary,
        format!(
            "#!/bin/sh\ntouch '{}'\nprintf 'build-bridge 9.9.9\\n'\n",
            probe_marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&job.staged_binary, fs::Permissions::from_mode(0o700)).unwrap();
    let mut stopped = false;
    let result = install_with(
        &job,
        dir.path(),
        &job.installed_binary,
        &mut |_| {
            stopped = true;
            Ok(())
        },
        &mut |_, _| Ok(()),
    );
    assert!(result.is_err());
    assert!(!probe_marker.exists());
    assert!(!stopped);
    assert!(fs::read_to_string(&job.installed_binary)
        .unwrap()
        .contains("0.2.0"));
}

#[test]
fn staged_probe_changing_its_own_bytes_cannot_be_installed() {
    let dir = tempfile::tempdir().unwrap();
    let mut job = fixture(dir.path());
    fs::write(
        &job.staged_binary,
        b"#!/bin/sh\nprintf 'build-bridge 9.9.9\\n'\nprintf '# changed after probe\\n' >> \"$0\"\n",
    )
    .unwrap();
    fs::set_permissions(&job.staged_binary, fs::Permissions::from_mode(0o700)).unwrap();
    job.staged_digest = super::super::provenance::binary_digest(&job.staged_binary).unwrap();
    let mut stopped = false;
    let result = install_with(
        &job,
        dir.path(),
        &job.installed_binary,
        &mut |_| {
            stopped = true;
            Ok(())
        },
        &mut |_, _| Ok(()),
    );
    assert!(result.is_err());
    assert!(!stopped);
    assert!(fs::read_to_string(&job.installed_binary)
        .unwrap()
        .contains("0.2.0"));
}

#[cfg(target_os = "linux")]
#[test]
fn real_file_syncs_precede_launch_stop_and_both_service_starts() {
    const TEST_NAME: &str =
        "update::installer::tests::real_file_syncs_precede_launch_stop_and_both_service_starts";
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let trace = root.join("fsync.log");
    if std::env::var_os("BUILD_UPDATE_SYNC_TEST_CHILD").is_some() {
        let home = PathBuf::from(std::env::var_os("BUILD_UPDATE_SYNC_TEST_ROOT").unwrap());
        let mut job = fixture(&home);
        job.staged_digest = super::super::provenance::binary_digest(&job.staged_binary).unwrap();
        let service = job.home.join(".config/systemd/user/build-bridge.service");
        fs::create_dir_all(service.parent().unwrap()).unwrap();
        fs::write(
            &service,
            format!("ExecStart=\"{}\" serve\n", job.installed_binary.display()),
        )
        .unwrap();
        write_json(&job_file(&home), &job).unwrap();
        fs::create_dir_all(updates_dir(&job.home)).unwrap();
        launch(&home).unwrap();
        let mut starts = 0;
        let outcome = install_with(
            &job,
            &home,
            &job.installed_binary,
            &mut |action| {
                let event = if action == "stop" {
                    if starts == 0 {
                        "stop-event"
                    } else {
                        "rollback-stop-event"
                    }
                } else {
                    starts += 1;
                    if starts == 1 {
                        "start-1-event"
                    } else {
                        "start-2-event"
                    }
                };
                let path = home.join(event);
                fs::write(&path, b"event").unwrap();
                sync_file(&path).unwrap();
                Ok(())
            },
            &mut |_, _| Err("force rollback".into()),
        );
        assert!(matches!(outcome, Err(TransactionFailure::Recovered(_))));
        return;
    }
    let source = root.join("trace.c");
    fs::write(
        &source,
        r#"#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int fsync(int fd) {
    static int (*real_fsync)(int);
    if (!real_fsync) real_fsync = dlsym(RTLD_NEXT, "fsync");
    char link[64], path[PATH_MAX], line[PATH_MAX + 16];
    snprintf(link, sizeof(link), "/proc/self/fd/%d", fd);
    ssize_t size = readlink(link, path, sizeof(path) - 1);
    const char *trace = getenv("BUILD_UPDATE_SYNC_TRACE");
    if (size > 0 && trace) {
        path[size] = '\0';
        int log = open(trace, O_WRONLY | O_APPEND | O_CREAT, 0600);
        if (log >= 0) {
            int count = snprintf(line, sizeof(line), "fsync %s\n", path);
            if (count > 0) write(log, line, (size_t)count);
            close(log);
        }
    }
    return real_fsync(fd);
}
"#,
    )
    .unwrap();
    let library = root.join("libsync_trace.so");
    assert!(Command::new("cc")
        .args(["-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(&source)
        .arg("-ldl")
        .status()
        .unwrap()
        .success());
    let fake_path = root.join("bin");
    fs::create_dir(&fake_path).unwrap();
    let launcher = fake_path.join("systemd-run");
    fs::write(&launcher, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let child_root = root.join("home");
    fs::create_dir(&child_root).unwrap();
    assert!(Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(TEST_NAME)
        .env("BUILD_UPDATE_SYNC_TEST_CHILD", "1")
        .env("BUILD_UPDATE_SYNC_TEST_ROOT", &child_root)
        .env("BUILD_UPDATE_SYNC_TRACE", &trace)
        .env("LD_PRELOAD", &library)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_path.display()))
        .status()
        .unwrap()
        .success());
    let events = fs::read_to_string(&trace).unwrap();
    let lines: Vec<_> = events.lines().collect();
    let index = |suffix: &str| {
        lines
            .iter()
            .position(|line| line.ends_with(suffix))
            .unwrap_or_else(|| panic!("missing fsync for {suffix}: {events}"))
    };
    let helper = index("/update-helper");
    let job_dir = lines
        .iter()
        .enumerate()
        .skip(helper + 1)
        .find(|(_, line)| line.ends_with("/home"))
        .map(|(index, _)| index)
        .unwrap();
    let active = index("/active.tmp");
    assert!(helper < job_dir && job_dir < active);
    let stop = index("/stop-event");
    for backup in [
        "/previous-build-bridge",
        "/running-build-bridge",
        "/previous-install-marker",
    ] {
        assert!(index(backup) < stop);
    }
    let marker_backup = index("/previous-install-marker");
    assert!(lines[marker_backup..stop]
        .iter()
        .any(|line| line.ends_with("/home")));
    let first_start = index("/start-1-event");
    let second_start = index("/start-2-event");
    let marker_syncs: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.ends_with("/installed-bridge.tmp"))
        .map(|(index, _)| index)
        .collect();
    assert!(marker_syncs
        .iter()
        .any(|entry| *entry > stop && *entry < first_start));
    assert!(marker_syncs
        .iter()
        .any(|entry| *entry > first_start && *entry < second_start));
    assert!(lines[stop..first_start]
        .iter()
        .any(|line| line.ends_with("/.build")));
    assert!(lines[first_start..second_start]
        .iter()
        .any(|line| line.ends_with("/.build")));
}
