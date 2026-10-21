use super::*;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = sources().await;
    steps.extend(initialization().await);
    steps
}

async fn sources() -> Vec<VerifiedStep> {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let initial = init_repo_named(&root, "initial");
    let source = init_repo_named(&root, "source");
    let extra = init_repo_named(&root, "extra");
    let origin = root.join("origin.git");
    git_in(
        &root,
        &[
            "clone",
            "--bare",
            source.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    let mut fixture = RareFixture::new("lifecycle-project", configured(&initial, &root));
    let proof = fixture
        .call(
            "project.create",
            json!({
                "name": "qualification-project", "sources": [{"path": source}]
            }),
        )
        .await;
    let project = proof.reply["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        project_row(&fixture, &project)["sources"][0]["path"],
        source.to_str().unwrap()
    );
    assert!(source.join(".git").is_dir());
    assert!(disk_json(&root.join("config.json"))["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == project));
    fixture
        .verify(
            proof,
            json!({"registered_project": project, "source": source}),
        )
        .await;
    source_membership(&mut fixture, &project, &extra).await;
    project_policy(&mut fixture, &project, &root).await;
    remote_and_sync(&mut fixture, &project, &source, &origin, &root).await;
    delete_project(&mut fixture, &project, &source, &root).await;
    fixture.finish().await
}

async fn source_membership(fixture: &mut RareFixture, project: &str, extra: &Path) {
    let proof = fixture
        .call(
            "project.add_source",
            json!({
                "project_id": project, "path": extra, "name": "docs"
            }),
        )
        .await;
    let source_id = proof.reply["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mount = proof.reply["result"]["sources"][1]["mount"].clone();
    assert_eq!(
        project_row(fixture, project)["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(extra.join("README.md")).unwrap(),
        "# project\n"
    );
    fixture
        .verify(
            proof,
            json!({"added_source": source_id, "original_exists": true}),
        )
        .await;

    let proof = fixture
        .call(
            "project.update_source",
            json!({
                "project_id": project, "source_id": source_id, "name": "Documentation"
            }),
        )
        .await;
    let row = project_row(fixture, project);
    assert_eq!(row["sources"][1]["name"], "Documentation");
    assert_eq!(row["sources"][1]["mount"], mount);
    fixture
        .verify(
            proof,
            json!({"source": source_id, "name": "Documentation", "mount": mount}),
        )
        .await;

    let proof = fixture
        .call(
            "project.remove_source",
            json!({
                "project_id": project, "source_id": source_id
            }),
        )
        .await;
    assert_eq!(
        project_row(fixture, project)["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(extra.join(".git").is_dir());
    fixture
        .verify(
            proof,
            json!({"removed_source": source_id, "original_exists": true}),
        )
        .await;
}

async fn project_policy(fixture: &mut RareFixture, project: &str, root: &Path) {
    let proof = fixture
        .call(
            "project.set_isolation",
            json!({
                "project_id": project, "isolation": "worktree"
            }),
        )
        .await;
    assert_eq!(project_row(fixture, project)["isolation"], "worktree");
    let config = disk_json(&root.join("config.json"));
    let saved = config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == project)
        .unwrap();
    assert_eq!(saved["isolation"], "worktree");
    fixture
        .verify(proof, json!({"persisted_isolation": "worktree"}))
        .await;

    let proof = fixture
        .call(
            "project.set_compaction",
            json!({
                "project_id": project, "compaction_thresholds": {"claude": 120000}
            }),
        )
        .await;
    assert_eq!(
        project_row(fixture, project)["compaction_thresholds"]["claude"],
        120000
    );
    let config = disk_json(&root.join("config.json"));
    let saved = config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == project)
        .unwrap();
    assert_eq!(saved["compaction_thresholds"]["claude"], 120000);
    fixture
        .verify(proof, json!({"persisted_compaction_tokens": 120000}))
        .await;

    let proof = fixture
        .call(
            "project.ensure_conversation",
            json!({"project_id": project}),
        )
        .await;
    let owner = proof.reply["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let scratch = {
        let app = fixture.state().lock().unwrap();
        assert!(app.project_pin(&owner).is_some());
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .iter()
            .any(|row| row.id == owner));
        app.runs[&owner].worktree.path.clone()
    };
    assert!(scratch.starts_with(root.join("project-scratch")));
    assert!(scratch.is_dir());
    fixture
        .verify(proof, json!({"durable_owner": owner, "scratch": scratch}))
        .await;
}

async fn remote_and_sync(
    fixture: &mut RareFixture,
    project: &str,
    source: &Path,
    origin: &Path,
    root: &Path,
) {
    let proof = fixture
        .call(
            "project.set_remote",
            json!({"project_id": project, "url": origin}),
        )
        .await;
    assert_eq!(
        git2::Repository::open(source)
            .unwrap()
            .find_remote("origin")
            .unwrap()
            .url(),
        origin.to_str()
    );
    assert_eq!(
        project_row(fixture, project)["remote"],
        origin.to_str().unwrap()
    );
    fixture.verify(proof, json!({"origin": origin})).await;
    let before = head(source);
    let publisher = root.join("publisher");
    git_in(
        root,
        &[
            "clone",
            origin.to_str().unwrap(),
            publisher.to_str().unwrap(),
        ],
    );
    crate::git_fixture::configure_repo(&publisher);
    std::fs::write(publisher.join("remote.txt"), "remote advance\n").unwrap();
    git_in(&publisher, &["add", "remote.txt"]);
    git_in(
        &publisher,
        &["commit", "-m", "advance qualification source"],
    );
    git_in(&publisher, &["push", "origin", "main"]);
    let wanted = head(&publisher);
    assert_ne!(wanted, before);
    let source_id = project_row(fixture, project)["sources"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let proof = fixture
        .call(
            "project.sync_source",
            json!({
                "project_id": project, "source_id": source_id, "source_path": source
            }),
        )
        .await;
    let origin_active = fixture.origin_active(&proof).await;
    assert_eq!(proof.reply["result"]["pending"], true);
    let service = Arc::clone(fixture.state());
    tokio::task::spawn_blocking(move || {
        AppState::sync_sources(&service, crate::app::SyncPass::Requested, 1000);
    })
    .await
    .unwrap();
    assert_eq!(head(source), wanted);
    let row = project_row(fixture, project);
    assert_eq!(row["sources"][0]["sync"]["state"], "synced");
    assert_eq!(row["sources"][0]["sync"]["needs_you"], false);
    assert!(
        origin_active,
        "source sync acknowledgement lost its queued service origin"
    );
    fixture
        .verify(
            proof,
            json!({"source_head": wanted, "previous_head": before, "sync_state": "synced"}),
        )
        .await;
}

async fn delete_project(fixture: &mut RareFixture, project: &str, source: &Path, root: &Path) {
    let workspace = workspace_setup(fixture, project, "project-deletion");
    let workspace_root = PathBuf::from(workspace["root"].as_str().unwrap());
    let proof = fixture
        .call(
            "project.delete",
            json!({"project_id": project, "confirm": true}),
        )
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert!(!workspace_root.exists());
    assert!(source.join(".git").is_dir());
    assert!(!read(fixture, "project.list", json!({}))["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["project_id"] == project));
    assert!(!disk_json(&root.join("config.json"))["projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == project));
    fixture.verify(proof, json!({"deleted_project": project, "removed_workspace": workspace_root, "source_preserved": source})).await;
}

async fn initialization() -> Vec<VerifiedStep> {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let initial = init_repo_named(&root, "initial");
    let plain = root.join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "retain these notes\n").unwrap();
    let mut app = configured(&initial, &root);
    let project = setup(
        &mut app,
        "project.create",
        open_folder(json!({"path": plain})),
    )["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut fixture = RareFixture::new("lifecycle-initialization", app);
    let workspace = workspace_setup(&fixture, &project, "plain-copy");
    let workspace_id = workspace["workspace_id"].as_str().unwrap();
    let source_id = workspace["directories"][0]["source_id"].as_str().unwrap();
    let copy = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let proof = fixture
        .call(
            "workspace.init_git",
            json!({
                "workspace_id": workspace_id, "source_id": source_id, "target": "workspace"
            }),
        )
        .await;
    assert_eq!(proof.reply["result"]["results"][0]["status"], "initialized");
    assert_eq!(
        workspace_row(&fixture, workspace_id)["directories"][0]["is_git"],
        true
    );
    assert!(copy.join(".git").is_dir());
    assert!(!plain.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(copy.join("notes.txt")).unwrap(),
        "retain these notes\n"
    );
    fixture
        .verify(
            proof,
            json!({"initialized_workspace_copy": copy, "head": head(&copy), "source_plain": true}),
        )
        .await;
    let proof = fixture
        .call("project.init_git", json!({"project_id": project}))
        .await;
    assert_eq!(project_row(&fixture, &project)["is_git"], true);
    assert!(plain.join(".git").is_dir());
    assert_eq!(
        std::fs::read_to_string(plain.join("notes.txt")).unwrap(),
        "retain these notes\n"
    );
    assert!(!git2::Repository::open(&plain)
        .unwrap()
        .index()
        .unwrap()
        .has_conflicts());
    fixture
        .verify(
            proof,
            json!({"initialized_source": plain, "head": head(&plain), "sentinel_preserved": true}),
        )
        .await;
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plain_project_and_workspace_initialization_have_independent_effect_proofs() {
    let steps = initialization().await;
    let methods: BTreeSet<_> = steps.iter().map(|step| step.method.as_str()).collect();
    assert_eq!(
        methods,
        ["project.init_git", "workspace.init_git"]
            .into_iter()
            .collect()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confirmed_project_deletion_has_independent_effect_proof() {
    let (directory, repo) = init_repo();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let app = configured(&repo, &root);
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("independent-project-delete", app);
    delete_project(&mut fixture, &project, &repo, &root).await;
    let steps = fixture.finish().await;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].method, "project.delete");
}
