// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn stageable_paths_fences_and_filters() {
    let mixed: Vec<String> = vec!["src/a.rs".into(), ".build/mcp.json".into()];
    // Survivors come out as :(literal) pathspecs — glob-proof; the MCP
    // config filter compares against the raw path, before decoration.
    assert_eq!(stageable_paths(&mixed).unwrap(), vec![":(literal)src/a.rs"]);
    assert_eq!(stageable_paths(&["*".into()]).unwrap(), vec![":(literal)*"]);

    assert!(stageable_paths(&["../evil".into()]).is_err());
    assert!(stageable_paths(&["/etc/passwd".into()]).is_err());
    assert!(stageable_paths(&["./relative".into()]).is_err());
    assert!(stageable_paths(&["".into()]).is_err());
    // One bad path poisons the whole list.
    assert!(stageable_paths(&["fine.txt".into(), "../evil".into()]).is_err());
}
#[test]
fn merging_state_maps_and_merge_abort_clears_it() {
    let dir = tempfile::tempdir().unwrap();
    init_diverged(dir.path());
    assert!(!git_run(dir.path(), &["merge", "feature"]).status.success());
    assert_eq!(repo_state(dir.path()), "merging");
    merge_abort(dir.path()).unwrap();
    assert_eq!(repo_state(dir.path()), "clean");
}
#[test]
fn bisect_maps_and_merge_abort_resets_it() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let first = String::from_utf8_lossy(&git_run(dir.path(), &["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_string();
    for i in 0..4 {
        write(dir.path(), "f.txt", &format!("v{i}\n"));
        git_ok(dir.path(), &["commit", "-q", "-am", &format!("v{i}")]);
    }
    git_ok(dir.path(), &["bisect", "start"]);
    git_ok(dir.path(), &["bisect", "bad", "HEAD"]);
    git_ok(dir.path(), &["bisect", "good", &first]);
    assert_eq!(repo_state(dir.path()), "bisecting");
    merge_abort(dir.path()).unwrap();
    assert_eq!(repo_state(dir.path()), "clean");
}
#[test]
fn checkout_refused_mid_cherry_pick_names_the_cherry_pick() {
    let dir = tempfile::tempdir().unwrap();
    init_diverged(dir.path());
    assert!(!git_run(dir.path(), &["cherry-pick", "feature"])
        .status
        .success());
    let err = checkout_ref(dir.path(), "refs/heads/feature").unwrap_err();
    assert!(err.contains("cherry-pick"), "{err}");
    assert!(!err.contains("merge"), "must not name the wrong op: {err}");
}
#[test]
fn checkout_refusal_message_names_each_operation() {
    assert!(checkout_refusal_message("merging").contains("merge"));
    assert!(checkout_refusal_message("rebasing").contains("rebase"));
    assert!(checkout_refusal_message("cherry-picking").contains("cherry-pick"));
    assert!(checkout_refusal_message("reverting").contains("revert"));
    assert!(checkout_refusal_message("bisecting").contains("bisect"));
    // A conflicted tree has no operation to abort — the files are resolved.
    let conflicted = checkout_refusal_message("conflicted");
    assert!(conflicted.contains("resolve"), "{conflicted}");
    assert!(conflicted.contains("conflict"), "{conflicted}");
    // "other" falls back without naming a specific verb.
    assert!(checkout_refusal_message("other").contains("in-progress"));
}
