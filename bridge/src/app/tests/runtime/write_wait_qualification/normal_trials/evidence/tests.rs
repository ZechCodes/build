use super::*;
use serde_json::{json, Value};

fn failure_json(root: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(root.join("entry-failure.json")).unwrap()).unwrap()
}

#[test]
fn original_root_and_files_cannot_replace_failed_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("originals");
    let bytes = b"invalid\xff original input\n";
    let captured = vec![("input-manifest.json", bytes.to_vec())];
    let artifacts = EntryArtifacts::claim(&root, &captured).unwrap();
    artifacts
        .write(
            "returned-suite.json",
            &json!({"qualified":false,"attempts":["small_trial_1"]}),
        )
        .unwrap();
    let original_report = std::fs::read(root.join("returned-suite.json")).unwrap();
    assert!(EntryArtifacts::claim(&root, &captured).is_err());
    assert!(artifacts
        .write("returned-suite.json", &json!({"qualified":true}))
        .is_err());
    assert_eq!(
        std::fs::read(root.join("input-manifest.json")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(root.join("returned-suite.json")).unwrap(),
        original_report
    );
}

#[test]
fn suite_error_and_panic_keep_inputs_and_explicit_failure() {
    let returned_error = suite_outcome(Ok(Err("setup failed after original intent".into())))
        .err()
        .expect("returned error must remain an error");
    let caught_panic =
        std::panic::catch_unwind(|| panic!("owned task panic before returned report"));
    let panic_error = suite_outcome(caught_panic)
        .err()
        .expect("caught panic must remain an error");
    assert_eq!(returned_error.0, "suite-returned-error");
    assert_eq!(returned_error.1, "setup failed after original intent");
    assert_eq!(panic_error.0, "suite-panicked");
    assert_eq!(panic_error.1, "owned task panic before returned report");
    for (stage, detail) in [returned_error, panic_error] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("originals");
        let bytes = b"{\"frozen\":true}\n";
        let artifacts =
            EntryArtifacts::claim(&root, &[("input-manifest.json", bytes.to_vec())]).unwrap();
        let panic = std::panic::catch_unwind(|| artifacts.fail(stage, &detail)).unwrap_err();
        assert!(panic_detail(&*panic).contains(&detail));
        let failure = failure_json(&root);
        assert_eq!(failure["stage"], stage);
        assert_eq!(failure["detail"], detail);
        assert_eq!(failure["qualified"], false);
        assert_eq!(failure["replacement_permitted"], false);
        assert_eq!(
            std::fs::read(root.join("input-manifest.json")).unwrap(),
            bytes
        );
    }
}

#[test]
fn preflight_failure_retains_exact_malformed_bytes_in_new_directory() {
    let bytes = b"malformed manifest\xff\n";
    let panic = std::panic::catch_unwind(|| {
        preflight_failure(
            &[("input-manifest.json", bytes.to_vec())],
            "provenance",
            "JSON parse rejected",
        )
    })
    .unwrap_err();
    let message = panic_detail(&*panic);
    let root = PathBuf::from(
        message
            .split("; artifacts=")
            .nth(1)
            .unwrap()
            .split("; failure_artifact=")
            .next()
            .unwrap(),
    );
    assert_eq!(
        std::fs::read(root.join("input-manifest.json")).unwrap(),
        bytes
    );
    assert_eq!(failure_json(&root)["detail"], "JSON parse rejected");
    // The production path keeps this root. This isolated test owns its cleanup.
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn artifact_write_failure_is_visible_and_never_claims_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("originals");
    let artifacts = EntryArtifacts::claim(&root, &[]).unwrap();
    std::fs::create_dir(root.join("entry-failure.json")).unwrap();
    let panic = std::panic::catch_unwind(|| artifacts.fail("suite-panicked", "original panic"))
        .unwrap_err();
    let message = panic_detail(&*panic);
    assert!(message.contains("original panic"));
    assert!(message.contains("failure_artifact=Err("), "{message}");
    assert!(root.join("entry-failure.json").is_dir());
}
