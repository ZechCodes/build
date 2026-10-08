use super::*;

const TEST_NAME: &str = "reviews::publication::tests::native_publication::initial_publication_avoids_native_receive_pack_ref_locks";

fn injector(temporary: &Path) -> std::path::PathBuf {
    let source = temporary.join("native_receive_fault.c");
    let library = temporary.join("native_receive_fault.so");
    fs::write(&source, include_str!("native_receive_fault.c")).unwrap();
    let compiled = std::process::Command::new("cc")
        .args(["-shared", "-fPIC"])
        .arg(source)
        .arg("-o")
        .arg(&library)
        .arg("-ldl")
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    library
}

#[test]
fn initial_publication_avoids_native_receive_pack_ref_locks() {
    if let Some(path) = std::env::var_os("BUILD_REVIEW_NATIVE_PUBLICATION_BINDING") {
        let binding: ReviewBranchBinding =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let result = publish_initial(&binding).map_err(|error| error.to_string());
        fs::write(
            std::env::var_os("BUILD_REVIEW_NATIVE_PUBLICATION_RESULT").unwrap(),
            serde_json::to_vec(&result).unwrap(),
        )
        .unwrap();
        return;
    }
    let (temporary, source) = init_repo();
    let binding = binding(&source, &temporary.path().join("receivers"));
    configure_remote(&binding).unwrap();
    let library = injector(temporary.path());
    let lock_suffix = format!("{}.lock", binding.receiving_ref);
    let control = temporary.path().join("native-control.git");
    git2::Repository::init_bare(&control).unwrap();
    let control_trace = temporary.path().join("control-trace");
    let lease = format!("--force-with-lease={}:", binding.receiving_ref);
    let refspec = format!("{}:{}", binding.initial_head, binding.receiving_ref);
    // Prove this injector interrupts a real receive-pack ref write, rather
    // than merely interrupting a Rust checkpoint or a hook before Git starts.
    let native = git_command(
        &source,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "push.followTags=false",
            "push",
            "--porcelain",
            "--no-verify",
            "--no-follow-tags",
            "--recurse-submodules=no",
            &lease,
            "--",
            control.to_str().unwrap(),
            &refspec,
        ],
    )
    .env("LD_PRELOAD", &library)
    .env("BUILD_REVIEW_NATIVE_REF_LOCK", &lock_suffix)
    .env("BUILD_REVIEW_NATIVE_REF_TRACE", &control_trace)
    .output()
    .unwrap();
    assert!(!native.status.success());
    assert!(fs::read_to_string(control_trace)
        .unwrap()
        .contains("native ref write:"));
    assert!(control.join(&lock_suffix).exists());

    let unrelated_lock = binding
        .receiving_repository
        .join("refs/heads/user-owned.lock");
    fs::write(&unrelated_lock, "user lock\n").unwrap();
    let binding_path = temporary.path().join("binding.json");
    fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();
    let result_path = temporary.path().join("initial-result.json");
    let publication_trace = temporary.path().join("publication-trace");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("LD_PRELOAD", library)
        .env("BUILD_REVIEW_NATIVE_REF_LOCK", &lock_suffix)
        .env("BUILD_REVIEW_NATIVE_REF_TRACE", &publication_trace)
        .env("BUILD_REVIEW_NATIVE_PUBLICATION_BINDING", binding_path)
        .env("BUILD_REVIEW_NATIVE_PUBLICATION_RESULT", &result_path)
        .status()
        .unwrap();
    assert!(status.success());
    let first: Result<String, String> =
        serde_json::from_slice(&fs::read(result_path).unwrap()).unwrap();
    let retry = publish_initial(&binding);
    let cancel = cleanup_initial(&binding);
    assert!(
        retry.is_ok() && cancel.is_ok(),
        "initial={first:?}; retry={retry:?}; cancel={cancel:?}"
    );
    assert_eq!(first.unwrap(), binding.initial_head);
    assert!(
        !publication_trace.exists(),
        "initial publication reached native receive-pack's unregistered ref writer"
    );
    assert!(!binding.receiving_repository.join(&lock_suffix).exists());
    assert_eq!(received_head(&binding).unwrap(), None);
    assert_eq!(fs::read_to_string(&unrelated_lock).unwrap(), "user lock\n");

    // An unknown lock at the very same receiving ref must remain a refusal.
    let unknown_lock = binding.receiving_repository.join(&lock_suffix);
    fs::write(&unknown_lock, "another user's lock\n").unwrap();
    assert!(publish_initial(&binding).is_err());
    assert!(cleanup_initial(&binding).is_err());
    assert_eq!(
        fs::read_to_string(unknown_lock).unwrap(),
        "another user's lock\n"
    );
}
