use super::*;
use crate::test_support::finite_service::{self, FixtureServices, PlannedService, ServiceKind};
use crate::timing::write_wait::{IntervalCategory, WriteWaitRegistry};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

fn service_plan(id: &str, kind: ServiceKind, target_ms: u64) -> PlannedService {
    PlannedService {
        original_id: id.into(),
        kind,
        target: Duration::from_millis(target_ms),
    }
}

fn with_origin<T>(id: &str, method: &str, work: impl FnOnce() -> T) -> T {
    let registry = WriteWaitRegistry::new();
    with_registry_origin(&registry, id, method, work)
}

fn with_registry_origin<T>(
    registry: &Arc<WriteWaitRegistry>,
    id: &str,
    method: &str,
    work: impl FnOnce() -> T,
) -> T {
    let original = registry.begin(id, method, Instant::now());
    let result = {
        let _bound = original.context().bind_sync();
        work()
    };
    original.complete();
    result
}

#[test]
fn unregistered_finite_service_preserves_one_real_operation_and_error() {
    let root = tempfile::tempdir().unwrap();
    let calls = AtomicUsize::new(0);
    let answer: Result<(), &str> =
        finite_service::run(root.path(), ServiceKind::Filesystem, || {
            calls.fetch_add(1, Ordering::Relaxed);
            Err("real filesystem refusal")
        });
    assert_eq!(answer, Err("real filesystem refusal"));
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn finite_service_targets_follow_original_ids_and_exact_owned_roots() {
    let root = tempfile::tempdir().unwrap();
    let owned = root.path().join("owned");
    let other = root.path().join("owned-sibling");
    let entered = Arc::new(AtomicUsize::new(0));
    let entered_copy = Arc::clone(&entered);
    let services = FixtureServices::install(
        vec![owned.clone()],
        vec![
            service_plan("later", ServiceKind::Filesystem, 20),
            service_plan("earlier", ServiceKind::Filesystem, 5),
        ],
        Some(Arc::new(move |_, _| {
            entered_copy.fetch_add(1, Ordering::Relaxed);
        })),
    );
    // The roots deliberately do not exist. Lookup must not canonicalize them.
    for id in ["earlier", "later"] {
        with_origin(id, "fs.write", || {
            finite_service::run(&other, ServiceKind::Filesystem, || Ok::<_, ()>(())).unwrap();
            finite_service::run(&owned, ServiceKind::Filesystem, || Ok::<_, ()>(())).unwrap();
        });
    }
    let samples = services.samples();
    assert_eq!(samples.len(), 2, "only the exact fixture root is enrolled");
    assert_eq!(samples[0].original_id, "earlier");
    assert_eq!(samples[0].target, Duration::from_millis(5));
    assert_eq!(samples[1].original_id, "later");
    assert_eq!(samples[1].target, Duration::from_millis(20));
    assert!(samples.iter().all(|sample| sample.actual >= sample.target));
    assert!(samples.iter().all(|sample| !sample.exceeded_ceiling()));
    drop(services);
    finite_service::run(&owned, ServiceKind::Filesystem, || Ok::<_, ()>(())).unwrap();
    assert_eq!(
        entered.load(Ordering::Relaxed),
        2,
        "owned registration removed"
    );
}

#[test]
fn finite_filesystem_service_keeps_real_revision_effect_and_app_guard() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("notes.md"), "before\n").unwrap();
    let (state, _) = frame_locks::shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let seen_guard = Arc::new(AtomicBool::new(false));
    let seen_finite_service = Arc::new(AtomicBool::new(false));
    let registry = WriteWaitRegistry::new();
    let observed_registry = Arc::clone(&registry);
    let observed_service = Arc::clone(&seen_finite_service);
    let observed_state = Arc::clone(&state);
    let observed_guard = Arc::clone(&seen_guard);
    let services = FixtureServices::install(
        vec![repo.clone()],
        vec![
            service_plan("replace", ServiceKind::Filesystem, 20),
            service_plan("stale", ServiceKind::Filesystem, 5),
        ],
        Some(Arc::new(move |_, _| {
            observed_guard.store(observed_state.try_lock().is_err(), Ordering::Release);
            let finite = observed_registry.active_snapshot().iter().any(|write| {
                write
                    .active
                    .iter()
                    .any(|interval| interval.category == IntervalCategory::FiniteService)
            });
            observed_service.fetch_or(finite, Ordering::Release);
        })),
    );
    let params = json!({
        "project_id": project_id,
        "path": "notes.md",
        "content_b64": b64encode(b"after\n"),
        "expected_revision": sha256_hex(b"before\n"),
    });
    let replaced = with_registry_origin(&registry, "replace", "fs.write", || {
        state
            .lock()
            .unwrap()
            .handle(req("fs.write", params.clone()))
    });
    let refused = with_origin("stale", "fs.write", || {
        state.lock().unwrap().handle(req("fs.write", params))
    });
    assert_eq!(replaced["ok"], true, "{replaced}");
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.md")).unwrap(),
        "after\n"
    );
    assert!(
        seen_guard.load(Ordering::Acquire),
        "actual service retains AppState guard"
    );
    assert!(
        seen_finite_service.load(Ordering::Acquire),
        "the actual filesystem hook exposes its distinct unfinished service boundary"
    );
    let samples = services.samples();
    assert_eq!(samples.len(), 2, "both actual service outcomes are visible");
    assert!(samples[0].succeeded);
    assert!(!samples[1].succeeded);
}

struct ServiceGitScope {
    root: PathBuf,
    valid: bool,
}

impl crate::app::GitCallScope for ServiceGitScope {
    fn mutation_path(&self) -> Option<&Path> {
        Some(&self.root)
    }

    fn validate(&self, _: &AppState) -> Result<(), String> {
        self.valid
            .then_some(())
            .ok_or_else(|| "scope moved".to_string())
    }

    fn invalidate(&self, _: &mut AppState) {}
}

fn stage_service_file(scope: &ServiceGitScope, _: &Value) -> Result<Value, String> {
    crate::gitgui::stage_paths(&scope.root, &["change.txt".into()])?;
    Ok(json!({ "staged": true }))
}

fn service_git_call(
    root: &Path,
    state: &Arc<Mutex<AppState>>,
    valid: bool,
) -> crate::app::ScopedGitCall<ServiceGitScope> {
    crate::app::ScopedGitCall {
        scope: ServiceGitScope {
            root: root.into(),
            valid,
        },
        work: stage_service_file,
        serialized: true,
        state: Some(Arc::downgrade(state)),
    }
}

#[test]
fn finite_git_service_retains_real_source_exclusion_preflight_and_index_effect() {
    use crate::app::git::deferred::DeferredGitWork;
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("change.txt"), "change\n").unwrap();
    let (state, _) = frame_locks::shared_state_and_handler(&repo, dir.path());
    let real_scope = Arc::new(AtomicBool::new(false));
    let scope_seen = Arc::clone(&real_scope);
    let observed_state = Arc::clone(&state);
    let observed_root = repo.clone();
    let services = FixtureServices::install(
        vec![repo.clone()],
        vec![
            service_plan("stage", ServiceKind::Git, 20),
            service_plan("moved", ServiceKind::Git, 5),
        ],
        Some(Arc::new(move |_, _| {
            let app_free = observed_state.try_lock().is_ok();
            let source_held =
                crate::source_sync::SyncLock::acquire(&observed_root, Duration::ZERO).is_none();
            scope_seen.store(app_free && source_held, Ordering::Release);
        })),
    );
    let staged = with_origin("stage", "git.stage", || {
        service_git_call(&repo, &state, true).run(&json!({}))
    });
    let moved = with_origin("moved", "git.stage", || {
        service_git_call(&repo, &state, false).run(&json!({}))
    });
    assert_eq!(staged.unwrap(), json!({ "staged": true }));
    assert_eq!(moved.unwrap_err(), "scope moved");
    let git = git2::Repository::open(&repo).unwrap();
    assert!(git
        .index()
        .unwrap()
        .get_path(Path::new("change.txt"), 0)
        .is_some());
    assert!(
        real_scope.load(Ordering::Acquire),
        "service is off AppState but inside actual source exclusion"
    );
    let samples = services.samples();
    assert_eq!(
        samples.len(),
        1,
        "stale scope refuses before external service"
    );
    assert_eq!(samples[0].original_id, "stage");
}
