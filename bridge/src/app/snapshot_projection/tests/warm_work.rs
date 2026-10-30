//! Actual warm-loop and eager watcher-root work; no clocks or latency assertions.
use super::*;
use crate::app::board::cache::{workspace_work_summary, DiffCacheEntry};
use crate::workspace::warm_work_counts::{self as work, Counts, Phase};
use crate::workspace::{Workspace, WorkspaceRegistry};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

struct WarmFixture {
    // The sole owner cleans up this fresh TMPDIR-scoped path, including on unwind.
    _root: tempfile::TempDir,
    app: AppState,
    project: String,
    ordered: Vec<String>,
    workspace_paths: BTreeSet<PathBuf>,
}
impl WarmFixture {
    fn new(owners: usize) -> Self {
        let root = tempfile::Builder::new()
            .prefix(".task901-warm-work-")
            .tempdir()
            .unwrap();
        let project_path = root.path().join("project");
        std::fs::create_dir(&project_path).unwrap();
        let mut app = fixture(root.path());
        // Do not read persisted/user manifests or use the retained large fixture.
        app.workspaces = WorkspaceRegistry::empty(root.path().join("private-registry"));
        let project = app.add_project(project_path, "main".into());
        let mut rows = Vec::new();
        for number in 0..owners {
            let owner = format!("run-{number:02}");
            let container = root.path().join(format!("container-{number:02}"));
            std::fs::create_dir(&container).unwrap();
            let id = format!("workspace-{number:02}");
            rows.push(install_workspace(
                &mut app,
                &project,
                &id,
                &format!("Name {number:02}"),
                &container,
            ));
            let active = crate::orchestrator::ActiveRun::workspace_conversation(
                crate::run::RunId::new(&owner),
                owner.clone(),
                container,
                crate::models::ModelChoice::default(),
            );
            app.projects.bind_entity(owner.clone(), project.clone());
            app.runs.insert(owner, active);
        }
        let shared = app.runs["run-00"].worktree.path.clone();
        // Equal names require the ID tie-breaker; insert the later ID first.
        for id in ["alias-z", "alias-a"] {
            rows.push(install_workspace(
                &mut app,
                &project,
                id,
                "000 alias",
                &shared,
            ));
        }
        rows.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        let ordered = rows.iter().map(|(_, id, _)| id.clone()).collect();
        let workspace_paths = rows.into_iter().map(|(_, _, path)| path).collect();
        Self {
            _root: root,
            app,
            project,
            ordered,
            workspace_paths,
        }
    }
}

fn install_workspace(
    app: &mut AppState,
    project: &str,
    id: &str,
    name: &str,
    container: &Path,
) -> (String, String, PathBuf) {
    let code = container.join(format!("code-{id}"));
    std::fs::create_dir_all(&code).unwrap();
    // Imported typed metadata deliberately describes a Git directory whose
    // physical repository is unavailable. Compute must retain its null outcome.
    let workspace: Workspace = serde_json::from_value(json!({
        "id":id,"project_id":project,"name":name,"root":container,
        "status":"ready","directories":[{"id":"dir-1","source_id":"source-1",
            "name":"code","path":code,"is_git":true,"branch":null,
            "status":"ready","source_path":code,"base_branch":"main"}],
        "managed":false,"created_by_agent":false,"locked":false
    }))
    .unwrap();
    app.workspaces.install_prepared_root_adoption(workspace);
    (name.into(), id.into(), container.into())
}

fn require(errors: &mut Vec<String>, condition: bool, description: impl Into<String>) {
    if !condition {
        errors.push(description.into());
    }
}

fn legacy_roots(app: &AppState) -> BTreeMap<String, PathBuf> {
    // Independent legacy conversation resolver, not worktree_roots/git_subjects
    // or the proposed indexed capture. Plain containers have no .git shortcut.
    let mut expected = BTreeMap::new();
    for (owner, active) in &app.runs {
        if active.run.state == crate::run::RunState::Archived {
            continue;
        }
        let root = app
            .workspace_for_conversation(owner, None)
            .and_then(|workspace| {
                workspace
                    .directories
                    .iter()
                    .find(|directory| directory.is_git)
            })
            .map(|directory| directory.path.clone())
            .unwrap_or_else(|| active.worktree.path.clone());
        expected.insert(owner.clone(), root);
    }
    for project in app.projects.iter() {
        for source in &project.sources {
            expected.insert(
                crate::changes::project_source_entity_id(
                    &project.id,
                    &source.id,
                    &source.path.display().to_string(),
                ),
                source.path.clone(),
            );
        }
    }
    expected
}

struct Report {
    owners: usize,
    workspaces: usize,
    counts: Counts,
    errors: Vec<String>,
}
fn one_pass(owners: usize) -> Report {
    let mut f = WarmFixture::new(owners);
    let expected_roots = legacy_roots(&f.app);
    let expected_summaries = f
        .app
        .workspaces
        .list(None)
        .into_iter()
        .map(|workspace| {
            let paths = workspace
                .directories
                .iter()
                .filter(|directory| directory.is_git)
                .map(|directory| directory.path.clone())
                .collect::<Vec<_>>();
            (
                workspace.id.clone(),
                paths.clone(),
                workspace_work_summary(&paths),
            )
        })
        .collect::<Vec<_>>();
    let (_, counts) = work::measure(|| f.app.warm_snapshot_board_observations());
    let mut errors = Vec::new();
    let pass = counts.phases.get(&Phase::ComputeStore).unwrap();
    let invocations = counts
        .invocations
        .iter()
        .filter(|invocation| invocation.phase == Phase::ComputeStore)
        .collect::<Vec<_>>();
    let jobs = owners + f.ordered.len();
    require(
        &mut errors,
        pass.passes == 1,
        "one actual compute/store pass required",
    );
    require(
        &mut errors,
        pass.jobs == jobs,
        "run and summary jobs must both execute",
    );
    require(
        &mut errors,
        pass.notifications == jobs,
        "each first cache insert retains BoardChanged timing",
    );
    require(
        &mut errors,
        invocations.len() == jobs,
        "one eager roots invocation for EACH original notification",
    );
    require(
        &mut errors,
        pass.workspace_lists > 0,
        "actual list counter never fired",
    );
    require(
        &mut errors,
        !pass.canonicalizations.is_empty(),
        "actual canonicalization sites never fired",
    );
    require(
        &mut errors,
        counts
            .phases
            .get(&Phase::Membership)
            .is_some_and(|phase| phase.workspace_lists == 1),
        "membership must be counted outside compute/store",
    );
    let mut violations = Vec::new();
    for (index, invocation) in invocations.iter().enumerate() {
        if invocation.workspace_lists > 1 {
            violations.push(format!(
                "invocation{index}: {} workspace lists >1",
                invocation.workspace_lists
            ));
        }
        for path in &f.workspace_paths {
            let actual = invocation.canonicalizations.get(path).copied().unwrap_or(0);
            if actual > 1 {
                violations.push(format!("invocation{index}: {path:?} attempts={actual} >1"));
            }
        }
        require(
            &mut errors,
            invocation.returned,
            "roots invocation did not return",
        );
    }
    require(
        &mut errors,
        violations.is_empty(),
        format!("per-invocation work violations: {violations:?}"),
    );
    require(
        &mut errors,
        f.app.watchers.roots_for_warm_work_test() == expected_roots,
        "watcher roots differ from independent legacy conversation resolution",
    );
    let ordered = f
        .app
        .workspaces
        .list(None)
        .iter()
        .map(|workspace| workspace.id.clone())
        .collect::<Vec<_>>();
    require(
        &mut errors,
        ordered == f.ordered,
        "name/ID workspace ordering changed",
    );
    require(
        &mut errors,
        f.app.workspace_of_conversation("run-00").as_deref() == Some("alias-a"),
        "equal-name aliases must use first ID",
    );
    for owner in f.app.runs.keys() {
        require(
            &mut errors,
            f.app
                .board
                .diff()
                .run_stat(owner)
                .is_some_and(|cached| cached.value == &Value::Null),
            format!("wrong unavailable-repository stat for {owner}"),
        );
    }
    for (id, paths, expected) in expected_summaries {
        require(
            &mut errors,
            f.app
                .workspace_summary_of(&id, &paths)
                .is_some_and(|(_, actual)| actual == &expected),
            format!("wrong summary cache {id}"),
        );
    }
    require(
        &mut errors,
        f.app.snapshot_ownership_context().is_none(),
        "ownership guard leaked beyond invocation",
    );
    Report {
        owners,
        workspaces: f.ordered.len(),
        counts,
        errors,
    }
}

#[test]
fn warm_roots_capture_work_is_bounded_per_invocation_at_ten_and_twenty_owners() {
    let reports = [one_pass(10), one_pass(20)];
    // Print BOTH sizes and all actual maps before any reviewed bound assertion.
    for report in &reports {
        let invocations = report
            .counts
            .invocations
            .iter()
            .filter(|invocation| invocation.phase == Phase::ComputeStore)
            .count();
        let lists = report.counts.phases[&Phase::ComputeStore].workspace_lists;
        let root_lists = report
            .counts
            .invocations
            .iter()
            .filter(|invocation| invocation.phase == Phase::ComputeStore)
            .map(|invocation| invocation.workspace_lists)
            .sum::<usize>();
        eprintln!(
            "warm_roots_work owners={} workspaces={} invocations={} invocations_per_owner={} whole_pass_lists={} root_scoped_lists_sum={} root_lists_per_invocation={} counts={:?} errors={:?}",
            report.owners,
            report.workspaces,
            invocations,
            invocations as f64 / report.owners as f64,
            lists,
            root_lists,
            root_lists as f64 / invocations.max(1) as f64,
            report.counts,
            report.errors
        );
    }
    let errors = reports
        .iter()
        .flat_map(|report| {
            report
                .errors
                .iter()
                .map(move |error| format!("owners={}: {error}", report.owners))
        })
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:#?}");
    let invocations = |report: &Report| {
        report
            .counts
            .invocations
            .iter()
            .filter(|invocation| invocation.phase == Phase::ComputeStore)
            .count()
    };
    // This growth is deliberately retained: no notification coalescing repair.
    assert_eq!(invocations(&reports[1]) - invocations(&reports[0]), 20);
}

#[test]
fn warm_roots_resolution_matches_legacy_direct_terminal_project_missing_and_alias_cases() {
    let mut f = WarmFixture::new(12);
    // Positive witness for BOTH actual canonicalization entry sites: same_path
    // attempts two, AppState::canonical_root attempts one. Not a proxy counter.
    let root = f._root.path();
    let ((equal, canonical), witnesses) = work::measure(|| {
        (
            crate::app::workspaces::same_path(root, root),
            AppState::canonical_root(root),
        )
    });
    assert!(equal);
    assert_eq!(canonical, root);
    assert_eq!(witnesses.phases[&Phase::Outside].canonicalizations[root], 3);
    // A failed canonicalization must preserve raw equality and inequality.
    let absent = root.join("never-created");
    assert!(crate::app::workspaces::same_path(&absent, &absent));
    assert!(!crate::app::workspaces::same_path(
        &absent,
        &root.join("never-created/..")
    ));
    let direct = f._root.path().join("direct-container");
    std::fs::create_dir(&direct).unwrap();
    install_workspace(&mut f.app, &f.project, "run-01", "000 direct", &direct);
    f.app.runs.get_mut("run-02").unwrap().run.state = crate::run::RunState::Merged;
    let missing = f.app.runs["run-03"].worktree.path.clone();
    std::fs::remove_dir_all(&missing).unwrap();
    let project_root = f.app.projects.get(&f.project).unwrap().repo_path.clone();
    let scratch = crate::app::projects::scratch_dir(&f.app.state_root, &project_root);
    std::fs::create_dir_all(&scratch).unwrap();
    f.app.runs.get_mut("run-04").unwrap().worktree.path = scratch;
    let other_path = f._root.path().join("other-project");
    std::fs::create_dir(&other_path).unwrap();
    let other = f.app.add_project(other_path, "main".into());
    f.app.projects.bind_entity("run-05".into(), other.clone());
    f.app.runs.get_mut("run-06").unwrap().worktree.path =
        f.app.runs["run-00"].worktree.path.clone();
    #[cfg(unix)]
    {
        let alias = f._root.path().join("linked-root");
        std::os::unix::fs::symlink(&f.app.runs["run-07"].worktree.path, &alias).unwrap();
        f.app.runs.get_mut("run-07").unwrap().worktree.path = alias;
    }
    let expected = legacy_roots(&f.app);
    let scopes = [
        None,
        Some(f.project.as_str()),
        Some(other.as_str()),
        Some("unknown-project"),
    ];
    let reference = f
        .app
        .runs
        .keys()
        .flat_map(|owner| scopes.iter().map(move |scope| (owner, *scope)))
        .map(|(owner, scope)| {
            (
                (owner.clone(), scope.map(str::to_owned)),
                f.app.workspace_of_conversation_in_project(owner, scope),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let (actual, counts) = work::measure(|| f.app.worktree_roots());
    eprintln!("warm_roots_resolution counts={counts:?}");
    assert_eq!(actual, expected);
    assert!(counts.invocations.len() == 1 && counts.invocations[0].returned);
    let captured = f.app.capture_snapshot_ownership();
    for ((owner, scope), expected) in reference {
        assert_eq!(
            captured.workspace_for_owner(&owner, scope.as_deref()),
            expected.as_deref(),
            "owner={owner} scope={scope:?}"
        );
    }
    assert_eq!(captured.workspace_for_owner("unknown-owner", None), None);
    assert_eq!(
        f.app.workspace_of_conversation("run-01").as_deref(),
        Some("run-01")
    );
    assert_eq!(f.app.workspace_of_conversation("run-02"), None);
    assert_eq!(f.app.workspace_of_conversation("run-04"), None);
    assert_eq!(f.app.workspace_of_conversation("run-05"), None);
    assert!(f.app.snapshot_ownership_context().is_none());
}

#[test]
fn warm_pass_preserves_absent_equal_changed_and_ready_stat_notifications() {
    for ready in [false, true] {
        let mut f = WarmFixture::new(12);
        f.app.warm_snapshot_board_observations();
        if ready {
            f.app.warm_snapshot_observations_for_test();
        }
        let roots = f.app.watchers.roots_for_warm_work_test();
        let (_, warm) = work::measure(|| f.app.warm_snapshot_board_observations());
        assert_eq!(
            warm.phases[&Phase::ComputeStore].jobs,
            0,
            "warm cache must not recompute"
        );
        assert_eq!(warm.phases[&Phase::ComputeStore].notifications, 0);
        let (_, equal) = work::measure(|| {
            f.app.store_diff_entry(DiffCacheEntry::RunStat {
                run_id: "run-00".into(),
                stat: Value::Null,
            })
        });
        assert!(
            equal.invocations.is_empty(),
            "equal cache entry must not construct watcher roots"
        );
        let changed = json!({"changed":"exact retained stat"});
        let (_, different) = work::measure(|| {
            f.app.store_diff_entry(DiffCacheEntry::RunStat {
                run_id: "run-00".into(),
                stat: changed.clone(),
            })
        });
        assert!(
            different.invocations.is_empty(),
            "changed existing stat uses owner publication, not BoardChanged"
        );
        assert_eq!(
            f.app.board.diff().run_stat("run-00").unwrap().value,
            &changed
        );
        assert_eq!(f.app.watchers.roots_for_warm_work_test(), roots);
        assert_eq!(f.app.snapshot_projection_initialized(), ready);
        eprintln!(
            "warm_cache_controls ready={ready} warm={warm:?} equal={equal:?} different={different:?}"
        );
    }
}
