//! Separate functional control; existing work RED assertions/counters stay intact.
use super::*;

fn observe_roots(fixture: &WarmFixture, phase: &str) -> BTreeMap<String, PathBuf> {
    assert!(fixture.app.snapshot_ownership_context().is_none());
    let expected = legacy_roots(&fixture.app);
    let (actual, counts) = work::measure(|| fixture.app.worktree_roots());
    eprintln!("warm_roots_freshness phase={phase} counts={counts:?}");
    assert_eq!(actual, expected, "phase={phase}");
    assert!(
        fixture.app.snapshot_ownership_context().is_none(),
        "phase={phase}"
    );
    actual
}

fn replace_workspace(fixture: &mut WarmFixture, id: &str, edit: impl FnOnce(&mut Workspace)) {
    let mut row = fixture.app.workspaces.get(id).unwrap().clone();
    edit(&mut row);
    fixture.app.workspaces.forget(id);
    fixture.app.workspaces.install_prepared_root_adoption(row);
}

#[test]
fn successive_roots_invocations_refresh_registry_and_keep_first_no_git_match() {
    for ready in [false, true] {
        let mut f = WarmFixture::new(12);
        if ready {
            f.app.initialize_snapshot_projection();
        }
        let checkout = f.app.runs["run-00"].worktree.path.clone();
        let first = observe_roots(&f, "initial");
        assert_eq!(first["run-00"], checkout.join("code-alias-a"));

        // Select workspace FIRST, then inspect its directory. A later alias
        // with Git must not win when the first matching alias has no Git.
        replace_workspace(&mut f, "alias-a", |row| row.directories[0].is_git = false);
        let no_git = observe_roots(&f, "first-match-no-git");
        assert_eq!(no_git["run-00"], checkout);
        replace_workspace(&mut f, "alias-a", |row| row.directories[0].is_git = true);

        // Typed registry replacement models an imported rename without trying
        // the public rename API, which deliberately refuses adopted rows.
        replace_workspace(&mut f, "alias-z", |row| row.name = "000 !first".into());
        let renamed = observe_roots(&f, "renamed-first-alias");
        assert_eq!(renamed["run-00"], checkout.join("code-alias-z"));

        let moved_root = f._root.path().join("moved-alias-root");
        std::fs::create_dir(&moved_root).unwrap();
        replace_workspace(&mut f, "alias-z", |row| row.root = moved_root);
        let moved = observe_roots(&f, "workspace-root-updated");
        assert_eq!(moved["run-00"], checkout.join("code-alias-a"));

        let new_directory = checkout.join("fresh-source-directory");
        std::fs::create_dir(&new_directory).unwrap();
        replace_workspace(&mut f, "alias-a", |row| {
            row.directories[0].path = new_directory.clone()
        });
        let directory = observe_roots(&f, "selected-directory-updated");
        assert_eq!(directory["run-00"], new_directory);

        f.app.workspaces.forget("alias-a");
        let forgotten = observe_roots(&f, "first-alias-forgotten");
        assert_eq!(forgotten["run-00"], checkout.join("code-workspace-00"));
        let project = f.project.clone();
        install_workspace(&mut f.app, &project, "alias-new", "000 !new", &checkout);
        let inserted = observe_roots(&f, "new-first-alias-inserted");
        assert_eq!(inserted["run-00"], checkout.join("code-alias-new"));

        // Missing physical paths retain exact stored/raw fallback behavior.
        std::fs::remove_dir_all(&checkout).unwrap();
        assert_eq!(observe_roots(&f, "physical-root-missing"), inserted);

        // A local roots call must neither replace nor clear an unrelated outer
        // publication context. The borrowed guard ends before any next edit.
        let expected = legacy_roots(&f.app);
        {
            let outer = f.app.begin_snapshot_ownership();
            let before = f.app.snapshot_ownership_context().unwrap();
            assert_eq!(f.app.worktree_roots(), expected);
            let after = f.app.snapshot_ownership_context().unwrap();
            assert!(Arc::ptr_eq(&before, &after));
            drop(outer);
        }
        assert!(f.app.snapshot_ownership_context().is_none());
    }
}

#[cfg(unix)]
#[test]
fn held_outer_ownership_survives_symlink_retarget_until_guard_drops() {
    let mut f = WarmFixture::new(12);
    let initial_container = f.app.runs["run-00"].worktree.path.clone();
    let later_container = f.app.runs["run-01"].worktree.path.clone();
    let alias = f._root.path().join("retargetable-run-root");
    std::os::unix::fs::symlink(&initial_container, &alias).unwrap();
    f.app.runs.get_mut("run-00").unwrap().worktree.path = alias.clone();
    let initial_expected = legacy_roots(&f.app);
    assert_eq!(
        initial_expected["run-00"],
        initial_container.join("code-alias-a")
    );
    {
        let outer = f.app.begin_snapshot_ownership();
        let captured = f.app.snapshot_ownership_context().unwrap();
        assert_eq!(f.app.worktree_roots(), initial_expected);
        // App inputs stay immutable; only this private physical symlink changes.
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&later_container, &alias).unwrap();
        let (held, counts) = work::measure(|| f.app.worktree_roots());
        eprintln!("warm_roots_held_context_retarget counts={counts:?}");
        assert_eq!(
            held, initial_expected,
            "must honor outer captured resolution"
        );
        assert_eq!(held["run-00"], initial_container.join("code-alias-a"));
        let current = f.app.snapshot_ownership_context().unwrap();
        assert!(Arc::ptr_eq(&captured, &current));
        assert_eq!(counts.invocations.len(), 1);
        assert_eq!(
            counts.invocations[0].workspace_lists, 0,
            "an existing context must not be replaced by a fresh registry list"
        );
        assert!(counts.invocations[0].canonicalizations.is_empty());
        drop(outer);
    }
    assert!(f.app.snapshot_ownership_context().is_none());
    let fresh = observe_roots(&f, "after-held-guard-symlink-retarget");
    assert_ne!(fresh["run-00"], initial_expected["run-00"]);
    // Depending on the original HashMap traversal, either run00 or run01 is
    // the first live owner of the new root. The legacy oracle pins that choice.
    assert_eq!(fresh, legacy_roots(&f.app));
}
