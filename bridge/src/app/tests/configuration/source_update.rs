//! `project.update_source`: a source's label, base branch, remote and folder,
//! edited in place. The remote is written to the source's checkout, where the
//! row reads it back from.

use super::*;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    state: AppState,
    project_id: String,
    code: PathBuf,
    docs: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let code = init_repo_named(&root, "code");
    let docs = init_repo_named(&root, "docs");
    git_in(&docs, &["branch", "develop"]);
    git_in(
        &docs,
        &["remote", "add", "origin", "git@example.com:old/docs.git"],
    );
    let config = root.join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let mut state =
        AppState::new_unrooted(root.join("worktrees"), "main", true, "/tmp/test-mcp.sock")
            .with_config(&config)
            .unwrap();
    let opened = state.handle(req(
        "project.create",
        json!({"name": "pair", "sources": [{"path": code}, {"path": docs}]}),
    ));
    assert_eq!(opened["ok"], true, "{opened:?}");
    let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
    Fixture {
        _dir: dir,
        root,
        config,
        state,
        project_id,
        code,
        docs,
    }
}

impl Fixture {
    fn update(&mut self, source_id: &str, change: Value) -> Value {
        let mut params = json!({"project_id": self.project_id, "source_id": source_id});
        params
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        self.state.handle(req("project.update_source", params))
    }

    fn restarted(&self) -> AppState {
        AppState::new_unrooted(
            self.root.join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&self.config)
        .unwrap()
    }
}

fn source<'a>(row: &'a Value, id: &str) -> &'a Value {
    row["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["id"] == id)
        .unwrap()
}

#[test]
fn a_label_is_renamed_and_its_folder_name_in_workspaces_is_kept() {
    let mut f = fixture();

    let answer = f.update("source-2", json!({"name": "Documentation"}));

    assert_eq!(answer["ok"], true, "{answer:?}");
    let docs = source(&answer["result"], "source-2");
    assert_eq!(docs["name"], "Documentation");
    assert_eq!(
        docs["mount"], "docs",
        "existing workspaces find their folder by its mount"
    );
    assert_eq!(answer["result"]["checkouts_updated"], 0);
    let listed = f.restarted().project_list();
    assert_eq!(
        source(&listed["projects"][0], "source-2")["name"],
        "Documentation"
    );
}

#[test]
fn a_base_branch_is_one_the_checkout_has_and_the_first_sources_is_the_projects() {
    let mut f = fixture();
    git_in(&f.code, &["branch", "trunk"]);

    let docs = f.update("source-2", json!({"base_branch": "develop"}));
    assert_eq!(
        source(&docs["result"], "source-2")["base_branch"],
        "develop",
        "{docs:?}"
    );
    assert_eq!(docs["result"]["base_branch"], "main");

    let code = f.update("source-1", json!({"base_branch": "trunk"}));
    assert_eq!(code["result"]["base_branch"], "trunk", "{code:?}");

    let refused = f.update("source-2", json!({"base_branch": "--orphan"}));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let missing = f.update("source-2", json!({"base_branch": "nope"}));
    assert_eq!(missing["ok"], false, "{missing:?}");
    let listed = f.restarted().project_list();
    assert_eq!(listed["projects"][0]["base_branch"], "trunk");
    assert_eq!(
        source(&listed["projects"][0], "source-2")["base_branch"],
        "develop"
    );
}

#[test]
fn a_remote_is_written_to_the_sources_checkout() {
    let mut f = fixture();

    let answer = f.update(
        "source-2",
        json!({"remote": "git@example.com:new/docs.git"}),
    );

    assert_eq!(answer["ok"], true, "{answer:?}");
    assert_eq!(
        source(&answer["result"], "source-2")["remote"],
        "git@example.com:new/docs.git"
    );
    assert_eq!(
        git_remote_origin(&f.docs).as_deref(),
        Some("git@example.com:new/docs.git")
    );
    assert_eq!(
        git_remote_origin(&f.code),
        None,
        "only that source's checkout"
    );

    let added = f.update(
        "source-1",
        json!({"remote": "https://example.com/code.git"}),
    );
    assert_eq!(
        added["result"]["remote"], "https://example.com/code.git",
        "{added:?}"
    );
    let cleared = f.update("source-2", json!({"remote": ""}));
    assert_eq!(
        source(&cleared["result"], "source-2")["remote"],
        Value::Null,
        "{cleared:?}"
    );
    assert_eq!(git_remote_origin(&f.docs), None);
}

#[test]
fn a_hostile_remote_is_refused_before_git_sees_it() {
    let mut f = fixture();

    let answer = f.update("source-2", json!({"remote": "--upload-pack=sh"}));

    assert_eq!(answer["ok"], false, "{answer:?}");
    assert!(
        answer["error"].as_str().unwrap().contains("Git remote"),
        "{answer:?}"
    );
    assert_eq!(
        git_remote_origin(&f.docs).as_deref(),
        Some("git@example.com:old/docs.git")
    );
}

#[test]
fn a_remote_moves_existing_workspace_copies_that_still_named_the_old_one() {
    let mut f = fixture();
    let created = f.state.handle(req(
        "workspace.create",
        json!({"project_id": f.project_id, "name": "work", "isolation": "worktree"}),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let checkout = PathBuf::from(
        created["result"]["directories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|directory| directory["source_id"] == "source-2")
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    // What a Rift or plain-copy checkout is: a repository of its own, with its
    // own config, standing where the workspace's directory is.
    git_in(
        &f.docs,
        &["worktree", "remove", "--force", checkout.to_str().unwrap()],
    );
    git_in(
        &f.root,
        &[
            "clone",
            "-q",
            f.docs.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    git_in(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.com:old/docs.git",
        ],
    );

    let answer = f.update(
        "source-2",
        json!({"remote": "git@example.com:new/docs.git"}),
    );

    assert_eq!(answer["ok"], true, "{answer:?}");
    assert_eq!(answer["result"]["checkouts_updated"], 1, "{answer:?}");
    assert_eq!(
        git_remote_origin(&checkout).as_deref(),
        Some("git@example.com:new/docs.git")
    );
}

#[test]
fn a_later_source_moves_to_another_folder_held_to_the_same_checks_as_adding_one() {
    let mut f = fixture();
    let moved_to = init_repo_named(&f.root, "docs-v2");

    let answer = f.update("source-2", json!({"path": moved_to}));
    assert_eq!(answer["ok"], true, "{answer:?}");
    assert_eq!(
        source(&answer["result"], "source-2")["path"],
        moved_to.display().to_string()
    );

    for refused in [
        json!({"path": f.code}),
        json!({"path": f.code.join("nested")}),
        json!({"path": f.root.join("missing")}),
        json!({"path": f.root.join("config.json")}),
    ] {
        let answer = f.update("source-2", refused.clone());
        assert_eq!(answer["ok"], false, "{refused:?} {answer:?}");
    }

    let primary = f.update(
        "source-1",
        json!({"path": init_repo_named(&f.root, "other")}),
    );
    assert_eq!(
        primary["ok"], false,
        "the first source is the project's home: {primary:?}"
    );
}

#[test]
fn an_edit_names_a_real_source_and_something_to_change() {
    let mut f = fixture();

    assert_eq!(f.update("source-9", json!({"name": "x"}))["ok"], false);
    assert_eq!(f.update("source-2", json!({}))["ok"], false);
    assert_eq!(f.update("source-2", json!({"name": "  "}))["ok"], false);
    assert_eq!(f.update("source-2", json!({"name": "a\nb"}))["ok"], false);
}
