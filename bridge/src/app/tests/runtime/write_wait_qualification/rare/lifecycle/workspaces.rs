use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acknowledged_size_measurement_retains_original_until_real_service_finishes() {
    let (directory, repo) = init_repo();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let app = configured(&repo, &root);
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("lifecycle-size-origin", app);
    let workspace = workspace_setup(&fixture, &project, "size-origin");
    let id = workspace["workspace_id"].as_str().unwrap();
    measure(&mut fixture, id).await;
    let steps = fixture.finish().await;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].method, "workspace.measure_sizes");
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repo, _origin) = init_repo_with_origin();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    let app = configured(&repo, &root);
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("lifecycle-workspace", app);
    let proof = fixture
        .call(
            "workspace.create",
            json!({
                "project_id": project, "name": "qualification", "isolation": "worktree"
            }),
        )
        .await;
    let workspace = proof.reply["result"].clone();
    let id = workspace["workspace_id"].as_str().unwrap().to_string();
    let checkout = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    assert_eq!(workspace_row(&fixture, &id)["status"], "ready");
    assert_eq!(head(&checkout), head(&repo));
    assert_ne!(workspace["directories"][0]["branch"], "main");
    assert!(Path::new(workspace["root"].as_str().unwrap())
        .join(crate::workspace::MANIFEST_FILE)
        .is_file());
    fixture
        .verify(
            proof,
            json!({"workspace": id, "checkout": checkout, "head": head(&checkout)}),
        )
        .await;
    conversation(&mut fixture, &id, &workspace).await;
    labels_and_policy(&mut fixture, &id, &workspace, &root).await;
    directories(&mut fixture, &project, &id, &root).await;
    measure(&mut fixture, &id).await;
    finish_workspace(&mut fixture, &id, &workspace, &repo).await;
    delete_workspace(&mut fixture, &project, &repo).await;
    finish_branch(&mut fixture, &project, &repo).await;
    fixture.finish().await
}

async fn conversation(fixture: &mut RareFixture, id: &str, workspace: &Value) {
    let proof = fixture
        .call("workspace.ensure_conversation", json!({"workspace_id": id}))
        .await;
    let owner = proof.reply["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let row = workspace_row(fixture, id);
    assert_eq!(row["entity_id"], owner);
    assert_eq!(row["agents"], json!([]));
    let app = fixture.state().lock().unwrap();
    assert_eq!(
        app.runs[&owner].worktree.path,
        PathBuf::from(workspace["root"].as_str().unwrap())
    );
    assert!(app
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .iter()
        .any(|run| run.id == owner));
    drop(app);
    fixture
        .verify(
            proof,
            json!({"durable_owner": owner, "workspace_root": workspace["root"]}),
        )
        .await;
}

async fn labels_and_policy(fixture: &mut RareFixture, id: &str, workspace: &Value, root: &Path) {
    let manifest =
        Path::new(workspace["root"].as_str().unwrap()).join(crate::workspace::MANIFEST_FILE);
    let proof = fixture
        .call(
            "workspace.rename",
            json!({"workspace_id": id, "name": "  renamed qualification  "}),
        )
        .await;
    let row = workspace_row(fixture, id);
    assert_eq!(row["name"], "renamed qualification");
    assert_eq!(row["root"], workspace["root"]);
    assert_eq!(
        row["directories"][0]["branch"],
        workspace["directories"][0]["branch"]
    );
    assert_eq!(disk_json(&manifest)["name"], "renamed qualification");
    fixture
        .verify(
            proof,
            json!({"persisted_name": "renamed qualification", "root": row["root"]}),
        )
        .await;

    let proof = fixture
        .call(
            "workspace.set_compaction",
            json!({
                "workspace_id": id, "compaction_thresholds": {"claude": 100000}
            }),
        )
        .await;
    assert_eq!(
        workspace_row(fixture, id)["compaction_thresholds"]["claude"],
        100000
    );
    assert_eq!(
        disk_json(&root.join("config.json"))["workspace_compaction_thresholds"][id]["claude"],
        100000
    );
    fixture
        .verify(proof, json!({"persisted_compaction_tokens": 100000}))
        .await;

    let proof = fixture
        .call(
            "workspace.set_locked",
            json!({"workspace_id": id, "locked": true}),
        )
        .await;
    assert_eq!(workspace_row(fixture, id)["locked"], true);
    assert_eq!(disk_json(&manifest)["locked"], true);
    fixture
        .verify(proof, json!({"persisted_locked": true}))
        .await;
    // Cleanup permits the independently measured removal later in this script.
    read(
        fixture,
        "workspace.set_locked",
        json!({"workspace_id": id, "locked": false}),
    );
}

async fn directories(fixture: &mut RareFixture, project: &str, id: &str, root: &Path) {
    let extra = init_repo_named(root, "extra");
    let source = read(
        fixture,
        "project.add_source",
        json!({"project_id": project, "path": extra, "name": "extra"}),
    );
    let source_id = source["sources"][1]["id"].as_str().unwrap().to_string();
    let proof = fixture
        .call(
            "workspace.add_directory",
            json!({"workspace_id": id, "source_id": source_id}),
        )
        .await;
    let row = workspace_row(fixture, id);
    assert_eq!(row["directories"].as_array().unwrap().len(), 2);
    let added = &row["directories"][1];
    let path = PathBuf::from(added["path"].as_str().unwrap());
    let directory_id = added["id"].as_str().unwrap().to_string();
    let registration = crate::isolation::checkout_name(&path).unwrap();
    assert_eq!(added["status"], "ready");
    assert_eq!(head(&path), head(&extra));
    assert!(git2::Repository::open(&extra)
        .unwrap()
        .find_worktree(&registration)
        .is_ok());
    fixture
        .verify(
            proof,
            json!({"directory": directory_id, "path": path, "head": head(&path)}),
        )
        .await;

    let proof = fixture
        .call(
            "workspace.remove_directory",
            json!({"workspace_id": id, "directory_id": directory_id}),
        )
        .await;
    assert!(!path.exists());
    assert_eq!(
        workspace_row(fixture, id)["directories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(git2::Repository::open(&extra)
        .unwrap()
        .find_worktree(&registration)
        .is_err());
    assert!(extra.join(".git").is_dir());
    fixture.verify(proof, json!({"removed_directory": directory_id, "registration_removed": registration, "source_preserved": extra})).await;
    // Later removal fixtures should contain only the origin-backed source.
    read(
        fixture,
        "project.remove_source",
        json!({"project_id": project, "source_id": source_id}),
    );
}

async fn measure(fixture: &mut RareFixture, id: &str) {
    let before = i64::try_from(crate::agent::now_ms()).unwrap();
    let proof = fixture
        .call("workspace.measure_sizes", json!({"workspace_ids": [id]}))
        .await;
    let origin_active = fixture.origin_active(&proof).await;
    assert_eq!(proof.reply["result"]["queued"], json!([id]));
    let revision = fixture.state().lock().unwrap().changes.board_revision();
    let service = Arc::clone(fixture.state());
    assert!(tokio::task::spawn_blocking(move || {
        AppState::measure_next_workspace_size(&service, before + 1)
    })
    .await
    .unwrap());
    let rows = read(fixture, "workspace.list", json!({}));
    let row = rows["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["workspace_id"] == id)
        .unwrap();
    let lifecycle = row["lifecycle"].clone();
    assert!(lifecycle["size_bytes"].as_u64().unwrap() > 0);
    assert!(lifecycle["size_measured_at_ms"].as_i64().unwrap() >= before);
    assert!(fixture.state().lock().unwrap().changes.board_revision() > revision);
    assert!(
        origin_active,
        "size measurement acknowledgement lost its queued service origin"
    );
    fixture
        .verify(
            proof,
            json!({"measured_lifecycle": lifecycle, "published": true}),
        )
        .await;
}

fn assert_removed(fixture: &RareFixture, id: &str, workspace: &Value, repo: &Path) {
    assert!(!Path::new(workspace["root"].as_str().unwrap()).exists());
    assert!(repo.join(".git").is_dir());
    assert!(fixture.state().lock().unwrap().workspaces.get(id).is_none());
    let listed = read(fixture, "workspace.list", json!({}));
    assert!(!listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["workspace_id"] == id));
}

fn assert_history(fixture: &RareFixture, id: &str) {
    let archive = read(fixture, "archived.list", json!({}));
    let item = archive["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["workspace_id"] == id)
        .unwrap();
    assert_eq!(item["kind"], "workspace");
    assert!(item["finished_at"].is_string());
}

async fn finish_workspace(fixture: &mut RareFixture, id: &str, workspace: &Value, repo: &Path) {
    let checkout = Path::new(workspace["directories"][0]["path"].as_str().unwrap());
    let registration = crate::isolation::checkout_name(checkout).unwrap();
    let proof = fixture
        .call("workspace.finish", json!({"workspace_id": id}))
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert_removed(fixture, id, workspace, repo);
    assert_history(fixture, id);
    assert!(git2::Repository::open(repo)
        .unwrap()
        .find_worktree(&registration)
        .is_err());
    fixture.verify(proof, json!({"finished_workspace": id, "history_retained": true, "registration_removed": registration})).await;
}

async fn delete_workspace(fixture: &mut RareFixture, project: &str, repo: &Path) {
    let workspace = workspace_setup(fixture, project, "delete-workspace");
    let id = workspace["workspace_id"].as_str().unwrap();
    let checkout = Path::new(workspace["directories"][0]["path"].as_str().unwrap());
    let registration = crate::isolation::checkout_name(checkout).unwrap();
    let proof = fixture
        .call("workspace.delete", json!({"workspace_id": id}))
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert_removed(fixture, id, &workspace, repo);
    assert!(git2::Repository::open(repo)
        .unwrap()
        .find_worktree(&registration)
        .is_err());
    fixture
        .verify(
            proof,
            json!({"deleted_workspace": id, "registration_removed": registration}),
        )
        .await;
}

async fn finish_branch(fixture: &mut RareFixture, project: &str, repo: &Path) {
    let workspace = workspace_setup(fixture, project, "finish-branch");
    let id = workspace["workspace_id"].as_str().unwrap();
    let branch = workspace["directories"][0]["branch"].as_str().unwrap();
    assert!(local_branch(repo, branch));
    let proof = fixture
        .call(
            "branch.finish",
            json!({"project_id": project, "branch": branch, "action": "delete"}),
        )
        .await;
    assert_eq!(proof.reply["result"]["deleted"], true);
    assert_removed(fixture, id, &workspace, repo);
    assert_history(fixture, id);
    assert!(!local_branch(repo, branch));
    fixture
        .verify(
            proof,
            json!({"finished_workspace": id, "deleted_branch": branch}),
        )
        .await;
}
