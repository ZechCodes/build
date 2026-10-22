//! Legacy primary terminals retain the registered root when that checkout
//! cannot currently be canonicalized, matching list and attach semantics.
use super::*;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn terminal_creation_preserves_missing_primary_checkout_fallback() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project = {
        let mut app = state.lock().unwrap();
        let shell = dir.path().join("finite-primary-shell");
        std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        app.term_shell = shell.display().to_string();
        app.project_at(0).id.clone()
    };
    // Keep the configured primary path but take its checkout away. Historically
    // the terminal layer accepts the raw registry key and the PTY chooses its
    // existing-directory fallback instead of failing scope resolution.
    std::fs::rename(&repo, dir.path().join("parked-primary")).unwrap();
    let created = call(&handler, "term.create", json!({"project_id":project}));
    let observations = if created["ok"] == true {
        let term_id = created["result"]["term_id"].clone();
        let (root, child) = {
            let app = state.lock().unwrap();
            let key = app.tab_key_of_wire_id(term_id.as_str().unwrap()).unwrap();
            (
                key.root.clone(),
                Arc::clone(&app.session_registry.test_tab(&key).unwrap().session),
            )
        };
        let listed = call(&handler, "term.list", json!({"project_id":project}));
        let attached = call(
            &handler,
            "term.attach",
            json!({"project_id":project,"term_id":term_id}),
        );
        let closed = call(&handler, "term.close", json!({"term_id":term_id}));
        child.end();
        let reaped = child.exited_within(Duration::from_secs(2));
        Some((root, listed, attached, closed, reaped))
    } else {
        None
    };
    // Every successful path retires its exact owned child before assertions.
    assert_eq!(created["ok"], true, "{created}");
    let (root, listed, attached, closed, reaped) = observations.unwrap();
    assert_eq!(root, repo, "the missing primary retains its raw scope key");
    assert_eq!(listed["ok"], true, "{listed}");
    assert!(listed["result"]["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["term_id"] == created["result"]["term_id"]));
    assert_eq!(attached["ok"], true, "{attached}");
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(reaped, "the owned primary shell must be reaped");
}
