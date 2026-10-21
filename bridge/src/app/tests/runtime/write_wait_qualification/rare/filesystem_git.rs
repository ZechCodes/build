//! Actual temporary-file and local-Git recipe scripts.
use super::{RareFixture, VerifiedStep};
use crate::app::tests::*;
use serde_json::json;
use std::path::Path;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut steps = filesystem().await;
    steps.extend(git().await);
    steps
}

async fn filesystem() -> Vec<VerifiedStep> {
    let (home, repo) = init_repo();
    let app = qa_state(&repo, home.path());
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("filesystem", app);

    let proof = fixture
        .call(
            "fs.mkdir",
            json!({"parent": home.path(), "name":"host-child"}),
        )
        .await;
    let host_child = home.path().join("host-child");
    assert!(host_child.is_dir());
    assert_eq!(
        proof.reply["result"]["path"],
        json!(host_child.canonicalize().unwrap())
    );
    fixture.verify(proof, json!({"directory":host_child})).await;

    let proof = fixture
        .call(
            "fs.createDirectory",
            json!({"project_id":project,"parent":"","name":"assets"}),
        )
        .await;
    assert!(repo.join("assets").is_dir());
    fixture
        .verify(proof, json!({"directory":repo.join("assets")}))
        .await;

    let proof = fixture
        .call(
            "fs.createFile",
            json!({"project_id":project,"parent":"assets","name":"notes.md"}),
        )
        .await;
    let note = repo.join("assets/notes.md");
    assert_eq!(std::fs::read(&note).unwrap(), b"");
    fixture.verify(proof, json!({"file":note,"bytes":0})).await;

    let contents = b"qualification edited file\n";
    let proof = fixture.call("fs.write", json!({"project_id":project,"path":"assets/notes.md","expected_revision":sha256_hex(b""),"content_b64":b64encode(contents)})).await;
    assert_eq!(std::fs::read(&note).unwrap(), contents);
    assert_eq!(proof.reply["result"]["revision"], sha256_hex(contents));
    fixture
        .verify(proof, json!({"file":note,"sha256":sha256_hex(contents)}))
        .await;

    upload(&mut fixture, &project, &repo, "completed.bin", true).await;
    upload(&mut fixture, &project, &repo, "aborted.bin", false).await;
    fixture.finish().await
}

async fn upload(fixture: &mut RareFixture, project: &str, repo: &Path, name: &str, finish: bool) {
    let bytes = [0xff, 0, 1, 2, 3];
    let proof = fixture
        .call(
            "fs.uploadBegin",
            json!({"project_id":project,"parent":"assets","name":name,"size":bytes.len()}),
        )
        .await;
    let id = proof.reply["result"]["upload_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let stage = repo.join("assets").join(format!(".build-upload-{id}.part"));
    let target = repo.join("assets").join(name);
    assert!(stage.is_file());
    assert!(!target.exists());
    fixture
        .verify(proof, json!({"upload_id":id,"staged_path":stage}))
        .await;

    let proof = fixture
        .call(
            "fs.uploadChunk",
            json!({"upload_id":id,"offset":0,"content_b64":b64encode(&bytes)}),
        )
        .await;
    assert_eq!(proof.reply["result"]["received"], bytes.len());
    assert_eq!(std::fs::read(&stage).unwrap(), bytes);
    assert!(!target.exists());
    fixture
        .verify(
            proof,
            json!({"upload_id":id,"received":bytes.len(),"sha256":sha256_hex(&bytes)}),
        )
        .await;

    let method = if finish {
        "fs.uploadFinish"
    } else {
        "fs.uploadAbort"
    };
    let proof = fixture.call(method, json!({"upload_id":id})).await;
    assert!(!stage.exists());
    if finish {
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(proof.reply["result"]["size"], bytes.len());
    } else {
        assert!(!target.exists());
    }
    fixture
        .verify(
            proof,
            json!({"upload_id":id,"staging_removed":true,"destination_present":finish}),
        )
        .await;
}

async fn git() -> Vec<VerifiedStep> {
    let (home, repo, origin) = init_repo_with_origin();
    let app = qa_state(&repo, home.path());
    let project = app.project_at(0).id.clone();
    let mut fixture = RareFixture::new("git", app);
    let edited = "# qualification local\n";
    std::fs::write(repo.join("README.md"), edited).unwrap();
    let proof = fixture
        .call(
            "git.stage",
            json!({"project_id":project,"paths":["README.md"]}),
        )
        .await;
    assert_eq!(output(&repo, &["show", ":README.md"]), edited.trim());
    fixture.verify(proof, json!({"index_readme":edited})).await;

    let before = output(&repo, &["rev-parse", "HEAD"]);
    let proof = fixture
        .call(
            "git.commit",
            json!({"project_id":project,"message":"qualification local"}),
        )
        .await;
    let head = output(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(head, before);
    assert_eq!(proof.reply["result"]["hash"], head);
    assert_eq!(output(&repo, &["show", "HEAD:README.md"]), edited.trim());
    fixture
        .verify(
            proof,
            json!({"old_head":before,"new_head":head,"readme":edited}),
        )
        .await;

    let proof = fixture
        .call("git.push", json!({"project_id":project}))
        .await;
    assert_eq!(output(&origin, &["rev-parse", "refs/heads/main"]), head);
    fixture.verify(proof, json!({"origin_head":head})).await;
    fetch_and_pull(&mut fixture, &project, &repo, &origin, home.path()).await;
    stash_and_discard(&mut fixture, &project, &repo).await;
    checkout_and_abort(&mut fixture, &project, &repo).await;
    fixture.finish().await
}

async fn fetch_and_pull(
    fixture: &mut RareFixture,
    project: &str,
    repo: &Path,
    origin: &Path,
    root: &Path,
) {
    let other = root.join("other");
    git_in(
        root,
        &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
    );
    crate::git_fixture::configure_repo(&other);
    std::fs::write(other.join("remote.txt"), "qualification remote\n").unwrap();
    git_in(&other, &["add", "remote.txt"]);
    git_in(&other, &["commit", "-m", "qualification remote"]);
    git_in(&other, &["push", "origin", "main"]);
    let remote = output(&other, &["rev-parse", "HEAD"]);
    let old_tracking = output(repo, &["rev-parse", "refs/remotes/origin/main"]);
    assert_ne!(old_tracking, remote);
    let proof = fixture
        .call("git.fetch", json!({"project_id":project}))
        .await;
    assert_eq!(
        output(repo, &["rev-parse", "refs/remotes/origin/main"]),
        remote
    );
    assert_eq!(proof.reply["result"]["behind"], 1);
    fixture
        .verify(
            proof,
            json!({"old_tracking":old_tracking,"tracking_head":remote}),
        )
        .await;

    let proof = fixture
        .call("git.pull", json!({"project_id":project,"mode":"ff"}))
        .await;
    assert_eq!(output(repo, &["rev-parse", "HEAD"]), remote);
    assert_eq!(
        std::fs::read_to_string(repo.join("remote.txt")).unwrap(),
        "qualification remote\n"
    );
    fixture
        .verify(
            proof,
            json!({"head":remote,"remote_file":"qualification remote\n"}),
        )
        .await;
}

async fn stash_and_discard(fixture: &mut RareFixture, project: &str, repo: &Path) {
    let original = std::fs::read(repo.join("README.md")).unwrap();
    std::fs::write(repo.join("README.md"), "dirty\n").unwrap();
    std::fs::write(repo.join("new.txt"), "untracked\n").unwrap();
    let proof = fixture
        .call("git.stash", json!({"project_id":project}))
        .await;
    assert_eq!(proof.reply["result"]["stash_count"], 1);
    assert_eq!(std::fs::read(repo.join("README.md")).unwrap(), original);
    assert!(!repo.join("new.txt").exists());
    fixture
        .verify(proof, json!({"stash_count":1,"clean_tree":true}))
        .await;

    let proof = fixture
        .call("git.stash_pop", json!({"project_id":project}))
        .await;
    assert_eq!(proof.reply["result"]["stash_count"], 0);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("new.txt")).unwrap(),
        "untracked\n"
    );
    fixture
        .verify(
            proof,
            json!({"stash_count":0,"tracked":"dirty\n","untracked":"untracked\n"}),
        )
        .await;

    let proof = fixture
        .call(
            "git.discard",
            json!({"project_id":project,"paths":["README.md","new.txt"]}),
        )
        .await;
    assert_eq!(std::fs::read(repo.join("README.md")).unwrap(), original);
    assert!(!repo.join("new.txt").exists());
    fixture
        .verify(
            proof,
            json!({"restored_sha256":sha256_hex(&original),"untracked_removed":true}),
        )
        .await;
}

async fn checkout_and_abort(fixture: &mut RareFixture, project: &str, repo: &Path) {
    git_in(repo, &["branch", "qualification-topic"]);
    let proof = fixture
        .call(
            "git.checkout_ref",
            json!({"project_id":project,"full_ref":"refs/heads/qualification-topic"}),
        )
        .await;
    assert_eq!(
        output(repo, &["symbolic-ref", "HEAD"]),
        "refs/heads/qualification-topic"
    );
    fixture
        .verify(proof, json!({"branch":"refs/heads/qualification-topic"}))
        .await;

    std::fs::write(repo.join("README.md"), "topic conflict\n").unwrap();
    git_in(repo, &["commit", "-am", "topic conflict"]);
    git_in(repo, &["checkout", "main"]);
    std::fs::write(repo.join("README.md"), "main conflict\n").unwrap();
    git_in(repo, &["commit", "-am", "main conflict"]);
    let before = output(repo, &["rev-parse", "HEAD"]);
    let merge = crate::git_fixture::git_command(repo, &["merge", "qualification-topic"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    assert!(repo.join(".git/MERGE_HEAD").exists());
    let proof = fixture
        .call("git.merge_abort", json!({"project_id":project}))
        .await;
    assert_eq!(proof.reply["result"]["repo_state"], "clean");
    assert!(!repo.join(".git/MERGE_HEAD").exists());
    assert_eq!(output(repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "main conflict\n"
    );
    fixture
        .verify(
            proof,
            json!({"head":before,"merge_state_removed":true,"readme":"main conflict\n"}),
        )
        .await;
}

fn output(repo: &Path, args: &[&str]) -> String {
    let result = crate::git_fixture::git_command(repo, args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn filesystem_and_git_recipes_verify_eighteen_real_method_effects() {
    crate::git_fixture::environment::isolated_git_test!();
    let methods: std::collections::BTreeSet<_> = execute()
        .await
        .into_iter()
        .map(|step| step.method)
        .collect();
    let expected: std::collections::BTreeSet<_> = [
        "fs.mkdir",
        "fs.createDirectory",
        "fs.createFile",
        "fs.write",
        "fs.uploadBegin",
        "fs.uploadChunk",
        "fs.uploadFinish",
        "fs.uploadAbort",
        "git.stage",
        "git.commit",
        "git.push",
        "git.fetch",
        "git.pull",
        "git.stash",
        "git.stash_pop",
        "git.discard",
        "git.checkout_ref",
        "git.merge_abort",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(methods, expected);
}
