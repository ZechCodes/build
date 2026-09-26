use super::*;
use crate::git_fixture::{git_in, init_repo};

fn fake_rift(dir: &std::path::Path) -> std::path::PathBuf {
    let executable = dir.join("fake-rift");
    super::super::test_fixture::write_executable(
        &executable,
        r#"#!/bin/sh
set -eu
[ "$1" = "--database" ]
database=$2
shift 2
command=$1
shift
{
  printf '%s' "$command"
  for argument in "$@"; do printf '\t%s' "$argument"; done
  printf '\n'
} >> "${database}.log"
case "$command" in
  init)
    project=$1
    if [ -f "$project/.rift" ] && [ "$(cat "$project/.rift")" != "$database" ]; then
      printf 'workspace belongs to a different registry\n' >&2
      exit 73
    fi
    if ! mkdir "${database}.initializing" 2>/dev/null; then
      printf 'overlapping Rift init\n' >&2
      exit 72
    fi
    sleep 0.1
    printf '%s\n' "$database" > "$project/.rift"
    if [ -d "$project/.git" ]; then
      mkdir -p "$project/.git/info"
      printf '/.rift\n' >> "$project/.git/info/exclude"
    fi
    touch "$database"
    rmdir "${database}.initializing"
    ;;
  create)
    project=$1
    shift
    parent=
    name=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --into) parent=$2; shift 2 ;;
        --name) name=$2; shift 2 ;;
        *) shift ;;
      esac
    done
    checkout="$parent/$name"
    cp -a "$project" "$checkout"
    if [ -d "$checkout/.git" ]; then
      git -C "$checkout" checkout --detach --quiet
      touch "$checkout/.git/index.lock"
    fi
    printf 'fake Rift marker\n' > "$checkout/.rift"
    printf '%s\n' "$checkout" >> "$database"
    printf '%s\n' "$checkout"
    ;;
  list)
    [ ! -f "$database" ] || cat "$database"
    ;;
  remove)
    [ "$1" = "--no-hooks" ]
    checkout=$2
    rm -rf -- "$checkout"
    temporary="${database}.tmp"
    grep -Fvx "$checkout" "$database" > "$temporary" || true
    mv "$temporary" "$database"
    ;;
  gc)
    :
    ;;
  *)
    printf 'unexpected fake Rift command: %s\n' "$command" >&2
    exit 64
    ;;
esac
"#,
    );
    executable
}

fn commit(path: &std::path::Path, name: &str) -> git2::Oid {
    std::fs::write(path.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
    git_in(path, &["add", "."]);
    git_in(path, &["commit", "-m", name]);
    git2::Repository::open(path)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
}

#[test]
fn fake_cli_covers_the_rift_checkout_lifecycle() {
    let (dir, project) = init_repo();
    std::fs::write(project.join(".gitignore"), ".cache/\n").unwrap();
    git_in(&project, &["add", ".gitignore"]);
    git_in(&project, &["commit", "-m", "ignore cache"]);
    std::fs::create_dir_all(project.join(".cache")).unwrap();
    std::fs::write(project.join(".cache/tool.bin"), "warm cache\n").unwrap();
    std::fs::write(project.join("dirty.txt"), "must be cleaned\n").unwrap();
    let repo = git2::Repository::open(&project).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("feature", &head, false).unwrap();

    let root = dir.path().join("worktrees");
    let backend = RiftBackend::with_executable(&root, fake_rift(dir.path()));
    let checkout = root.join("feature-checkout");

    backend.materialize(&project, "feature", &checkout).unwrap();

    assert_eq!(Isolation::of(&checkout), Some(Isolation::Rift));
    backend.verify(&project, &checkout, "feature").unwrap();
    assert!(
        checkout.join(".cache/tool.bin").exists(),
        "Rift copied caches"
    );
    assert!(
        !checkout.join("dirty.txt").exists(),
        "Build cleaned copied dirt"
    );
    assert!(!checkout.join(".git/index.lock").exists());
    assert_eq!(
        git2::Repository::open(&checkout)
            .unwrap()
            .head()
            .unwrap()
            .shorthand(),
        Some("feature")
    );
    assert!(backend.holds_record(&project, "feature-checkout").unwrap());
    assert_eq!(
        backend.discover(&project, &root).unwrap(),
        vec![checkout.clone()]
    );

    let feature_tip = commit(&checkout, "feature-work");
    backend.publish(&project, &checkout, "feature").unwrap();
    assert_eq!(
        git2::Repository::open(&project)
            .unwrap()
            .find_reference("refs/heads/feature")
            .unwrap()
            .target(),
        Some(feature_tip)
    );

    let main_tip = commit(&project, "base-work");
    backend.sync_base(&project, &checkout, "main").unwrap();
    assert_eq!(
        git2::Repository::open(&checkout)
            .unwrap()
            .find_reference("refs/heads/main")
            .unwrap()
            .target(),
        Some(main_tip)
    );

    backend.remove(&project, &checkout).unwrap();
    assert!(!checkout.exists());
    assert!(!backend.holds_record(&project, "feature-checkout").unwrap());
    assert_cli_lifecycle(&backend, &project, &root, &checkout);
}

#[test]
fn configured_cli_materializes_an_ordinary_directory() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let root = dir.path().join("workspaces");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("notes.txt"), "preserved\n").unwrap();
    let backend = RiftBackend::with_executable(&root, fake_rift(dir.path()));
    let destination = root.join("notes");

    backend
        .materialize_directory(&source, &destination)
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(destination.join("notes.txt")).unwrap(),
        "preserved\n"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("notes.txt")).unwrap(),
        "preserved\n"
    );
    let log =
        std::fs::read_to_string(backend.database_path().with_extension("sqlite.log")).unwrap();
    assert!(
        log.lines().any(|line| line
            == format!(
                "create\t{}\t--into\t{}\t--name\tnotes\t--copy-all\t--no-hooks",
                source.display(),
                root.display()
            )),
        "the configured CLI did not receive the directory create: {log}"
    );
}

#[test]
fn stable_registry_reuses_a_plain_source_across_workspace_roots() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let registry_root = dir.path().join("project-worktrees");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("notes.txt"), "preserved\n").unwrap();
    let executable = fake_rift(dir.path());

    for name in ["first", "second"] {
        let workspace_root = dir.path().join(name);
        std::fs::create_dir(&workspace_root).unwrap();
        let backend = RiftBackend::with_registry_root(&workspace_root, &registry_root, &executable);
        backend
            .materialize_directory(&source, &workspace_root.join("notes"))
            .unwrap();
    }

    assert_eq!(
        std::fs::read_to_string(dir.path().join("second/notes/notes.txt")).unwrap(),
        "preserved\n"
    );
}

#[test]
fn stable_registry_reuses_a_git_source_across_workspace_roots() {
    let (dir, project) = init_repo();
    let registry_root = dir.path().join("project-worktrees");
    let executable = fake_rift(dir.path());

    for name in ["first", "second"] {
        let workspace_root = dir.path().join(name);
        std::fs::create_dir(&workspace_root).unwrap();
        let backend = RiftBackend::with_registry_root(&workspace_root, &registry_root, &executable);
        backend
            .materialize(&project, "main", &workspace_root.join("repository"))
            .unwrap();
    }

    assert_eq!(
        Isolation::of(&dir.path().join("second/repository")),
        Some(Isolation::Rift)
    );
}

fn assert_cli_lifecycle(backend: &RiftBackend, project: &Path, root: &Path, checkout: &Path) {
    let log =
        std::fs::read_to_string(backend.database_path().with_extension("sqlite.log")).unwrap();
    let expected = [
        format!("init\t{}\t--here", project.display()),
        format!(
            "create\t{}\t--into\t{}\t--name\tfeature-checkout\t--copy-all\t--no-hooks",
            project.display(),
            root.display(),
        ),
        format!("remove\t--no-hooks\t{}", checkout.display()),
        "gc".to_string(),
    ];
    for command in expected {
        assert!(
            log.lines().any(|line| line == command),
            "missing {command:?}: {log}"
        );
    }
}

#[test]
fn a_failing_cli_reports_rifts_stderr() {
    let (dir, project) = init_repo();
    let executable = dir.path().join("failing-rift");
    super::super::test_fixture::write_executable(
        &executable,
        "#!/bin/sh\nprintf 'fixture failure' >&2\nexit 23\n",
    );
    let backend = RiftBackend::with_executable(dir.path().join("worktrees"), executable);

    let error = backend
        .materialize(&project, "main", &dir.path().join("worktrees/task"))
        .unwrap_err()
        .to_string();

    assert!(error.contains("rift"), "{error}");
    assert!(error.contains("fixture failure"), "{error}");
}

#[test]
fn a_copied_core_worktree_cannot_send_git_cleanup_into_the_source() {
    let (dir, project) = init_repo();
    git_in(
        &project,
        &["config", "core.worktree", project.to_str().unwrap()],
    );
    let source_file = project.join("source-only.txt");
    std::fs::write(&source_file, "must survive\n").unwrap();
    let root = dir.path().join("worktrees");
    let backend = RiftBackend::with_executable(&root, fake_rift(dir.path()));
    let checkout = root.join("unsafe-worktree");

    let error = backend
        .materialize(&project, "main", &checkout)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("Git worktree outside the checkout"),
        "{error}"
    );
    assert!(source_file.exists(), "source working tree was cleaned");
    assert!(!checkout.exists(), "the rejected Rift checkout was removed");
}

#[test]
fn cloned_managers_serialize_first_rift_initialization() {
    use crate::worktree::WorktreeManager;
    use std::sync::{Arc, Barrier};

    let (dir, project) = init_repo();
    let root = dir.path().join("worktrees");
    let manager = WorktreeManager::new(&project, &root).with_rift_executable(fake_rift(dir.path()));
    let barrier = Arc::new(Barrier::new(3));

    let results = std::thread::scope(|scope| {
        let first_manager = manager.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = scope.spawn(move || {
            first_barrier.wait();
            first_manager.create("parallel-one", "main", Isolation::Rift)
        });
        let second_manager = manager.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            second_manager.create("parallel-two", "main", Isolation::Rift)
        });
        barrier.wait();
        [first.join().unwrap(), second.join().unwrap()]
    });

    for result in results {
        result.unwrap();
    }
    let repository = git2::Repository::open(&project).unwrap();
    assert!(repository
        .find_reference("refs/heads/build/parallel-one")
        .is_ok());
    assert!(repository
        .find_reference("refs/heads/build/parallel-two")
        .is_ok());
}
