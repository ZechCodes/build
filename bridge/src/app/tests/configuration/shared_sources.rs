//! Directory sharing changes reference admission, never filesystem ownership.
use super::*;

struct SharedSources {
    root: tempfile::TempDir,
    config: PathBuf,
    a: PathBuf,
    b: PathBuf,
    shared: PathBuf,
    app: AppState,
}

impl SharedSources {
    fn new(both: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let a = init_repo_named(root.path(), "a");
        let b = init_repo_named(root.path(), "b");
        let shared = init_repo_named(root.path(), "shared");
        let config = root.path().join("config.json");
        let source = |id: &str, path: &Path| {
            json!({
                "id":id,"path":path,"name":id,"mount":id,"sync_base":false
            })
        };
        let mut sources_b = vec![source("source-1", &b)];
        if both {
            sources_b.push(source("source-2", &shared));
        }
        std::fs::write(&config, json!({"projects_dir":root.path().join("projects"),"projects":[
            {"id":"proj-1","path":a,"sources":[source("source-1", &a),source("source-2", &shared)]},
            {"id":"proj-2","path":b,"sources":sources_b}
        ]}).to_string()).unwrap();
        let app = Self::load(root.path(), &config);
        Self {
            root,
            config,
            a,
            b,
            shared,
            app,
        }
    }

    fn load(root: &Path, config: &Path) -> AppState {
        let context =
            crate::harness::HarnessContext::resolved(root.join("mcp.sock"), root.join("state"))
                .unwrap();
        AppState::new_unrooted_configured(root.join("workspaces"), "main", true, context)
            .with_config(config)
            .unwrap()
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.app.handle(req(method, params))
    }

    fn add(&mut self, project: &str, path: &Path) -> Value {
        self.call(
            "project.add_source",
            json!({"project_id":project,"path":path}),
        )
    }

    fn remove(&mut self, project: &str, id: &str) -> Value {
        self.call(
            "project.remove_source",
            json!({"project_id":project,"source_id":id}),
        )
    }

    fn reload(&mut self) {
        self.app = Self::load(self.root.path(), &self.config);
    }
}

#[test]
fn same_directory_shared_by_two_projects() {
    let mut f = SharedSources::new(false);
    let shared = f.shared.clone();
    let answer = f.add("proj-2", &shared);
    assert_eq!(answer["ok"], true, "{answer}");
    f.reload();
    for owner in ["proj-1", "proj-2"] {
        assert!(f
            .app
            .sources_for(owner)
            .unwrap()
            .iter()
            .any(|s| s.path == shared));
    }
    // The new reference is added to project 2, never copied into its environment.
    assert_eq!(f.app.projects.get("proj-2").unwrap().added_sources.len(), 1);
    #[cfg(unix)]
    {
        let alias = f.root.path().join("alias");
        std::os::unix::fs::symlink(&shared, &alias).unwrap();
        let answer = f.add("proj-2", &alias);
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(
            answer["error"]
                .as_str()
                .unwrap()
                .contains("already included in this project"),
            "{answer}"
        );
    }
}

#[test]
fn prepared_duplicate_is_rechecked_before_membership_write() {
    let mut f = SharedSources::new(false);
    let before = std::fs::read(&f.config).unwrap();
    let source = f.app.sources_for("proj-1").unwrap()[1].clone();
    let result = f.app.append_project_sources("proj-1", vec![source], vec![]);
    assert!(result
        .unwrap_err()
        .contains("already included in this project"));
    assert_eq!(std::fs::read(&f.config).unwrap(), before);
}

#[test]
fn source_overlap_refused_across_environments() {
    let mut f = SharedSources::new(false);
    let child = f.shared.join("nested");
    std::fs::create_dir(&child).unwrap();
    for path in [&child, f.root.path()] {
        let answer = f.app.handle(req(
            "project.add_source",
            json!({"project_id":"proj-2","path":path}),
        ));
        assert_eq!(answer["ok"], false, "{answer}");
        assert!(
            answer["error"].as_str().unwrap().contains("overlap"),
            "{answer}"
        );
    }
    let vault_root = f.root.path().join("device");
    let vault_folder = vault_root.join("secrets/source");
    std::fs::create_dir_all(&vault_folder).unwrap();
    f.app.secret_state_root = Some(vault_root);
    let answer = f.add("proj-2", &vault_folder);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"].as_str().unwrap().contains("vault"),
        "{answer}"
    );
    git_in(
        &f.shared,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/shared.git",
        ],
    );
    let answer = f.call(
        "project.add_source",
        json!({"project_id":"proj-2","remote":"https://example.invalid/shared.git"}),
    );
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("already registered"),
        "{answer}"
    );
    // A clone destination cannot be inside a different project's source either.
    f.app.projects_dir = f.shared.clone();
    let answer = f.call(
        "project.add_source",
        json!({"project_id":"proj-2","remote":"https://example.invalid/other.git"}),
    );
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("managed source destination"),
        "{answer}"
    );
}

#[test]
fn added_source_duplicate_within_project_refused() {
    let mut f = SharedSources::new(false);
    let before = std::fs::read(&f.config).unwrap();
    let shared = f.shared.clone();
    let answer = f.add("proj-1", &shared);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("already included in this project"),
        "{answer}"
    );
    assert_eq!(std::fs::read(&f.config).unwrap(), before);
    let extra = init_repo_named(f.root.path(), "extra");
    assert_eq!(f.add("proj-1", &extra)["ok"], true);
    let before = std::fs::read(&f.config).unwrap();
    let answer = f.add("proj-1", &extra);
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("already included in this project"),
        "{answer}"
    );
    assert_eq!(std::fs::read(&f.config).unwrap(), before);
}

#[test]
fn unlink_keeps_folder_and_other_reference() {
    let mut f = SharedSources::new(false);
    let before = std::fs::read(f.shared.join("README.md")).unwrap();
    let shared = f.shared.clone();
    assert_eq!(f.add("proj-2", &shared)["ok"], true);
    assert_eq!(f.app.projects.get("proj-2").unwrap().added_sources.len(), 1);
    let answer = f.remove("proj-2", "source-2");
    assert_eq!(answer["ok"], true, "{answer}");
    f.reload();
    assert_eq!(std::fs::read(f.shared.join("README.md")).unwrap(), before);
    assert!(f
        .app
        .sources_for("proj-2")
        .unwrap()
        .iter()
        .all(|s| s.path != f.shared));
    assert!(f
        .app
        .sources_for("proj-1")
        .unwrap()
        .iter()
        .any(|s| s.path == f.shared));
}

#[test]
fn unlink_last_managed_reference_keeps_clone() {
    let mut f = SharedSources::new(false);
    let managed = f.app.projects_dir.join("managed-sources/clone");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    git2::Repository::clone(f.shared.to_str().unwrap(), &managed).unwrap();
    let answer = f.add("proj-2", &managed);
    assert_eq!(answer["ok"], true, "{answer}");
    let id = answer["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(f.remove("proj-2", &id)["ok"], true);
    f.reload();
    assert!(managed.join(".git").is_dir());
    assert!(managed.join("README.md").is_file());
    assert!(f
        .app
        .projects
        .iter()
        .flat_map(|p| &p.sources)
        .all(|s| s.path != managed));
}

#[test]
fn repoint_reference_leaves_sibling_reference() {
    let mut f = SharedSources::new(true);
    git_in(&f.shared, &["branch", "develop"]);
    let answer = f.call("project.update_source", json!({"project_id":"proj-1","source_id":"source-2","name":"mine","base_branch":"develop","sync_base":false}));
    assert_eq!(answer["ok"], true, "{answer}");
    let sibling = f.app.sources_for("proj-2").unwrap()[1].clone();
    assert_ne!(sibling.name, "mine");
    assert_eq!(sibling.base_branch, "main");
    f.call("project.list", json!({}));
    let answer = f.call("project.update_source", json!({"project_id":"proj-1","source_id":"source-2","remote":"https://example.invalid/shared.git"}));
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(f.app.sources_for("proj-2").unwrap()[1], sibling);
    let listed = f.call("project.list", json!({}));
    let other = listed["result"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|project| project["project_id"] == "proj-2")
        .unwrap();
    assert_eq!(
        other["sources"][1]["remote"],
        "https://example.invalid/shared.git"
    );
    let answer = f.call(
        "project.update_source",
        json!({"project_id":"proj-1","source_id":"source-2","path":f.b}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    f.reload();
    assert_eq!(f.app.sources_for("proj-1").unwrap()[1].path, f.b);
    assert_eq!(f.app.sources_for("proj-2").unwrap()[1].path, f.shared);
    assert!(f.shared.join("README.md").exists());
    let answer = f.call(
        "project.update_source",
        json!({"project_id":"proj-1","source_id":"source-2","path":f.a}),
    );
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("already included in this project"),
        "{answer}"
    );
}

#[test]
fn linked_worktrees_stay_distinct_sources() {
    let mut f = SharedSources::new(false);
    let linked = f.root.path().join("linked");
    git_in(
        &f.shared,
        &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
    );
    let answer = f.add("proj-1", &linked);
    assert_eq!(answer["ok"], true, "{answer}");
    let paths = f
        .app
        .sources_for("proj-1")
        .unwrap()
        .iter()
        .map(|s| s.path.clone())
        .collect::<Vec<_>>();
    assert!(paths.contains(&f.shared));
    assert!(paths.contains(&std::fs::canonicalize(&linked).unwrap()));
    assert_ne!(f.shared, linked);
    assert_eq!(
        crate::source_sync::sync_lock_identity(&f.shared),
        crate::source_sync::sync_lock_identity(&linked)
    );
}
