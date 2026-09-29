//! A source's remote is its checkout's `origin`. The config does not keep a
//! second copy that can drift from it (#228: the Do project's config still
//! named the url it was cloned from after the checkout was pointed elsewhere).

use super::*;

fn two_source_project(dir: &Path, config: &Path) -> (AppState, String, PathBuf, PathBuf) {
    let code = init_repo_named(dir, "code");
    let docs = init_repo_named(dir, "docs");
    git_in(
        &code,
        &["remote", "add", "origin", "git@example.com:org/code.git"],
    );
    git_in(
        &docs,
        &["remote", "add", "origin", "git@example.com:org/docs.git"],
    );
    let mut state =
        AppState::new_unrooted(dir.join("worktrees"), "main", true, "/tmp/test-mcp.sock")
            .with_config(config)
            .unwrap();
    let opened = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": code}, {"path": docs}]}),
    ));
    assert_eq!(opened["ok"], true, "{opened:?}");
    let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
    (state, project_id, code, docs)
}

fn listed_project(state: &AppState, project_id: &str) -> Value {
    state.project_list()["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["project_id"] == project_id)
        .cloned()
        .unwrap()
}

#[test]
fn every_source_answers_the_origin_its_checkout_holds_now() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let (mut state, project_id, code, docs) = two_source_project(dir.path(), &config);
    git_in(
        &docs,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.com:moved/docs.git",
        ],
    );
    git_in(
        &code,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.com:moved/code.git",
        ],
    );

    let listed = listed_project(&state, &project_id);
    assert_eq!(
        listed["sources"][0]["remote"],
        "git@example.com:moved/code.git"
    );
    assert_eq!(
        listed["sources"][1]["remote"],
        "git@example.com:moved/docs.git"
    );
    assert_eq!(
        listed["remote"], "git@example.com:moved/code.git",
        "the project's is its first source's"
    );

    let answered = state.handle(req(
        "project.set_isolation",
        json!({"project_id": project_id, "isolation": null}),
    ));
    assert_eq!(
        answered["result"]["sources"][1]["remote"], "git@example.com:moved/docs.git",
        "{answered:?}"
    );
}

#[test]
fn the_config_keeps_no_copy_of_a_sources_remote_and_ignores_a_stale_one() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let (_state, project_id, _code, docs) = two_source_project(dir.path(), &config);
    let mut written: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    let sources = written["projects"][0]["sources"].as_array_mut().unwrap();
    assert!(
        sources.iter().all(|source| source.get("remote").is_none()),
        "{sources:?}"
    );

    // What a bridge before #228 wrote: the url the source was added with.
    sources[1]["remote"] = json!("git@example.com:stale/docs.git");
    std::fs::write(&config, serde_json::to_vec(&written).unwrap()).unwrap();
    let restarted = AppState::new_unrooted(
        dir.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();

    let listed = listed_project(&restarted, &project_id);
    assert_eq!(
        listed["sources"][1]["remote"],
        "git@example.com:org/docs.git"
    );
    assert_eq!(
        crate::worktree::git_remote_origin(&docs).as_deref(),
        Some("git@example.com:org/docs.git")
    );
}

#[test]
fn a_source_whose_checkout_has_no_origin_answers_none() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let (state, project_id, _code, docs) = two_source_project(dir.path(), &config);
    git_in(&docs, &["remote", "remove", "origin"]);

    let listed = listed_project(&state, &project_id);
    assert_eq!(listed["sources"][1]["remote"], Value::Null);
}
