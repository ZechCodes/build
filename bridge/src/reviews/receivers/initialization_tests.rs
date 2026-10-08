use super::initialization::{ensure_with, initialize_receiver};
use super::*;
use crate::git_fixture::init_repo;

#[cfg(unix)]
#[test]
fn interrupted_receiver_initialization_does_not_block_retry() {
    if let Some(source) = std::env::var_os("BUILD_REVIEW_INTERRUPTED_INIT_SOURCE") {
        let root = std::env::var_os("BUILD_REVIEW_INTERRUPTED_INIT_ROOT").unwrap();
        let receiver = plan_receiver(Path::new(&source), Path::new(&root)).unwrap();
        ensure_with(&receiver, |attempt| {
            write_owned_json(&attempt.path.join(OWNERSHIP_FILE), &ownership(attempt))?;
            git(&attempt.path, &["init", "--bare"])?;
            fs::write(attempt.path.join("config.lock"), "interrupted Git child\n")
                .map_err(|error| error.to_string())?;
            fs::write(
                Path::new(&root).join("interrupted-attempt.json"),
                serde_json::to_vec(&attempt.path).unwrap(),
            )
            .map_err(|error| error.to_string())?;
            std::process::exit(25);
        })
        .unwrap();
        panic!("initialization child must exit without dropping its attempt");
    }

    let (temporary, source) = init_repo();
    let root = temporary.path().join("receivers");
    let receiver = plan_receiver(&source, &root).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "reviews::receivers::initialization_tests::interrupted_receiver_initialization_does_not_block_retry",
            "--nocapture",
        ])
        .env("BUILD_REVIEW_INTERRUPTED_INIT_SOURCE", &source)
        .env("BUILD_REVIEW_INTERRUPTED_INIT_ROOT", &root)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(25));
    let abandoned: PathBuf =
        serde_json::from_slice(&fs::read(root.join("interrupted-attempt.json")).unwrap()).unwrap();
    let external = abandoned.join("refs/heads/external");
    fs::write(&external, "external ref contents\n").unwrap();
    ensure_receiver(&receiver).unwrap();
    assert!(git2::Repository::open_bare(&receiver.path)
        .unwrap()
        .is_bare());
    assert!(!receiver.path.join("config.lock").exists());
    assert_ne!(abandoned, receiver.path);
    assert_eq!(
        fs::read_to_string(abandoned.join("config.lock")).unwrap(),
        "interrupted Git child\n"
    );
    assert_eq!(
        fs::read_to_string(external).unwrap(),
        "external ref contents\n"
    );
    let config = git2::Repository::open_bare(&receiver.path)
        .unwrap()
        .config()
        .unwrap();
    assert_eq!(config.get_string("core.hooksPath").unwrap(), "/dev/null");
    assert_eq!(config.get_i32("gc.auto").unwrap(), 0);
    assert!(!config.get_bool("maintenance.auto").unwrap());
}

#[test]
fn initialization_preserves_a_late_unowned_empty_directory() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    let error = ensure_with(&receiver, |attempt| {
        initialize_receiver(attempt)?;
        fs::create_dir(&receiver.path).unwrap();
        Ok(())
    })
    .unwrap_err();
    assert!(error.contains("ownership"));
    assert_eq!(fs::read_dir(&receiver.path).unwrap().count(), 0);
}

#[test]
fn initialization_preserves_a_changed_attempt_marker() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    let mut changed_marker = None;
    let error = ensure_with(&receiver, |attempt| {
        initialize_receiver(attempt)?;
        let marker = attempt.path.join(OWNERSHIP_FILE);
        fs::write(&marker, "{}").unwrap();
        changed_marker = Some(marker);
        Ok(())
    })
    .unwrap_err();
    assert!(error.contains("ownership"));
    assert!(!receiver.path.exists());
    assert_eq!(fs::read_to_string(changed_marker.unwrap()).unwrap(), "{}");
}

#[test]
fn initialization_preserves_a_changed_reservation() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    let reservation = receiver.path.parent().unwrap().join(format!(
        ".{}.receiver-reservation.json",
        receiver.repository_id
    ));
    let error = ensure_with(&receiver, |attempt| {
        initialize_receiver(attempt)?;
        fs::write(&reservation, "{}").unwrap();
        Ok(())
    })
    .unwrap_err();
    assert!(error.contains("ownership"));
    assert!(!receiver.path.exists());
    assert_eq!(fs::read_to_string(reservation).unwrap(), "{}");
}

#[test]
fn a_complete_receiver_reservation_replay_preserves_external_changes() {
    let (temporary, source) = init_repo();
    let receiver = plan_receiver(&source, &temporary.path().join("receivers")).unwrap();
    ensure_receiver(&receiver).unwrap();
    git(
        &receiver.path,
        &["config", "--local", "build.external", "true"],
    )
    .unwrap();
    let config = fs::read(receiver.path.join("config")).unwrap();
    let external_ref = receiver.path.join("refs/heads/external");
    fs::write(&external_ref, "external ref contents\n").unwrap();
    write_owned_json(
        &receiver.path.parent().unwrap().join(format!(
            ".{}.receiver-reservation.json",
            receiver.repository_id
        )),
        &ownership(&receiver),
    )
    .unwrap();
    ensure_receiver(&receiver).unwrap();
    assert_eq!(fs::read(receiver.path.join("config")).unwrap(), config);
    assert_eq!(
        fs::read_to_string(external_ref).unwrap(),
        "external ref contents\n"
    );
}
