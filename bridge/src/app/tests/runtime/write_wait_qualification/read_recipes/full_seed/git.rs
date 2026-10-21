//! Seed at the actual existing inactive checkout. No new workspace.
use crate::app::tests::runtime::write_wait_qualification::fixture::WorkspaceContext;
use crate::app::AppState;
use std::path::{Path, PathBuf};

pub(super) struct GitSeed {
    pub base: String,
    pub head: String,
    pub branch_ref: String,
    pub upstream_ref: String,
    pub worktree: String,
    pub committed_file: String,
    pub diff_file: String,
    pub diff_needle: String,
    pub external_path: PathBuf,
    pub external_name: String,
}

fn commit(repo: &git2::Repository, paths: &[&str], subject: &str) -> String {
    let mut index = repo.index().unwrap();
    for path in paths {
        index.add_path(Path::new(path)).unwrap();
    }
    index.write().unwrap();
    let oid = index.write_tree().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let signature = git2::Signature::now("Qualification", "fixture@qualification.invalid").unwrap();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        subject,
        &tree,
        &[&parent],
    )
    .unwrap()
    .to_string()
}

pub(super) fn seed(
    app: &mut AppState,
    scope: &WorkspaceContext,
    project: &str,
    staged_plan: &str,
    stage: &str,
    fixture_root: &Path,
) -> GitSeed {
    let repo = git2::Repository::open(&scope.directory).unwrap();
    let base = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let committed_file = "qualification-read-commit.txt".to_owned();
    let diff_file = "qualification-read-diff.txt".to_owned();
    std::fs::write(
        scope.directory.join(&committed_file),
        b"Qualification immutable committed witness\n",
    )
    .unwrap();
    std::fs::write(
        scope.directory.join(&diff_file),
        b"Qualification old diff witness\n",
    )
    .unwrap();
    let head = commit(
        &repo,
        &[&committed_file, &diff_file],
        "Qualification immutable read boundary",
    );
    let diff_needle = "Qualification changed diff witness".to_owned();
    std::fs::write(scope.directory.join(&diff_file), format!("{diff_needle}\n")).unwrap();
    std::fs::create_dir_all(scope.directory.join("qualification-child")).unwrap();
    let private_bare = fixture_root.join("read-upstream.git");
    git2::Repository::init_bare(&private_bare).unwrap();
    // Reference observation only: neither fetch nor push nor network execution.
    repo.remote("qualification", private_bare.to_str().unwrap())
        .unwrap();
    repo.reference(
        "refs/remotes/qualification/main",
        git2::Oid::from_str(&base).unwrap(),
        true,
        "qualification immutable base",
    )
    .unwrap();
    let branch = repo.head().unwrap().shorthand().unwrap().to_owned();
    repo.find_branch(&branch, git2::BranchType::Local)
        .unwrap()
        .set_upstream(Some("qualification/main"))
        .unwrap();
    let branch_ref = format!("refs/heads/{branch}");
    let mut active = app.take_run(&scope.owner).unwrap();
    active.run.plan_id = Some(crate::plan::PlanId::new(staged_plan));
    active.run.state = crate::run::RunState::Review;
    active.base_sha = Some(base.clone());
    active.stages = vec![crate::run::StageProgress {
        stage_id: stage.into(),
        state: crate::run::StageProgressState::Completed,
        start_sha: Some(base.clone()),
        built_sha: Some(head.clone()),
        completion_sha: Some(head.clone()),
        publication: crate::run::StagePublication::Local,
        invalidation_reason: None,
    }];
    active.current_stage_id = Some(stage.into());
    app.finish_run_mutation(scope.owner.clone(), active)
        .unwrap();
    // Actual external discovery excludes ALL bound run worktrees. This one
    // explicitly ledgered support checkout is not a registry resident, owner,
    // auth endpoint or canonical history. It is never adopted into Build.
    let external_name = "qualification-read-external".to_owned();
    let external_path = fixture_root.join(&external_name);
    let commit = repo
        .find_commit(git2::Oid::from_str(&head).unwrap())
        .unwrap();
    let branch = repo.branch(&external_name, &commit, false).unwrap();
    let mut options = git2::WorktreeAddOptions::new();
    options.reference(Some(branch.get()));
    repo.worktree(&external_name, &external_path, Some(&options))
        .unwrap();
    std::fs::write(
        external_path.join(&diff_file),
        format!("{diff_needle} external\n"),
    )
    .unwrap();
    let canonical = std::fs::canonicalize(&external_path).unwrap();
    let worktree = app
        .scan_external_worktrees_now(project)
        .unwrap()
        .into_iter()
        .find(|w| std::fs::canonicalize(&w.path).ok().as_ref() == Some(&canonical))
        .expect("actual discovered ID of nonresident support checkout")
        .id;
    GitSeed {
        base,
        head,
        branch_ref,
        upstream_ref: "refs/remotes/qualification/main".into(),
        worktree,
        committed_file,
        diff_file,
        diff_needle,
        external_path,
        external_name,
    }
}
