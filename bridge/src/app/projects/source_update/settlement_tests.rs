use super::*;
use crate::app::runtime::lifecycle::LifecycleOutcome;
use crate::app::DeferredWork;
use std::path::Path;

struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
    original: PathBuf,
    target: PathBuf,
    app: AppState,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let other = root.path().join("other");
        let original = root.path().join("original");
        let target = root.path().join("bucket/secrets/target");
        for path in [&home, &other, &original, &target] {
            std::fs::create_dir_all(path).unwrap();
        }
        let config = root.path().join("config.json");
        std::fs::write(
            &config,
            json!({"projects_dir":root.path().join("projects"),"projects":[
                {"id":"proj-1","path":home,"sources":[
                    {"id":"source-1","path":home,"name":"home","mount":"home"},
                    {"id":"source-2","path":original,"name":"original","mount":"original"}
                ]},
                {"id":"proj-2","path":other}
            ]})
            .to_string(),
        )
        .unwrap();
        let context = crate::harness::HarnessContext::resolved(
            root.path().join("mcp.sock"),
            root.path().join("state"),
        )
        .unwrap();
        let app = AppState::new_unrooted_configured(
            root.path().join("workspaces"),
            "main",
            true,
            context,
        )
        .with_config(&config)
        .unwrap();
        Self {
            _root: root,
            config,
            original,
            target,
            app,
        }
    }

    fn run_pending(&mut self) -> LifecycleOutcome {
        let Some(DeferredWork::Lifecycle(job)) = self.app.deferred_work.take() else {
            panic!("source edit must defer its filesystem work");
        };
        job.run()
    }

    fn prepared_repoint(&mut self) -> LifecycleOutcome {
        self.app
            .project_update_source(
                &json!({"project_id":"proj-1","source_id":"source-2","path":self.target}),
            )
            .unwrap();
        self.run_pending()
    }

    fn add(&mut self, project: &str, path: &Path) {
        self.app
            .project_add_source(&json!({"project_id":project,"path":path}))
            .unwrap();
        let result = self.run_pending();
        self.app.apply_lifecycle(result).unwrap();
    }

    fn assert_repoint_refused(&mut self, result: LifecycleOutcome, message: &str) {
        let before = std::fs::read(&self.config).unwrap();
        let error = self.app.apply_lifecycle(result).unwrap_err();
        assert!(error.contains(message), "{error}");
        assert_eq!(std::fs::read(&self.config).unwrap(), before);
        let sources = self.app.sources_for("proj-1").unwrap();
        assert_eq!(
            sources
                .iter()
                .find(|source| source.id == "source-2")
                .unwrap()
                .path,
            self.original
        );
    }
}

#[test]
fn repoint_settlement_refuses_concurrent_effective_duplicate() {
    let mut f = Fixture::new();
    let repoint = f.prepared_repoint();
    // Opening an addition does not take the repoint's repository lock. It can
    // publish after the edit's perform phase and before its settlement.
    let target = f.target.clone();
    f.add("proj-1", &target);
    f.assert_repoint_refused(repoint, "already included in this project");
    assert_eq!(
        f.app
            .sources_for("proj-1")
            .unwrap()
            .iter()
            .filter(|source| source.path == target)
            .count(),
        1
    );
}

#[test]
fn repoint_settlement_refuses_concurrent_source_containment() {
    for nested in [false, true] {
        let mut f = Fixture::new();
        let repoint = f.prepared_repoint();
        let other = if nested {
            f.target.join("nested")
        } else {
            f.target.parent().unwrap().to_path_buf()
        };
        std::fs::create_dir_all(&other).unwrap();
        f.add("proj-2", &other);
        f.assert_repoint_refused(repoint, "overlap");
    }
}

#[test]
fn repoint_settlement_rechecks_vault_after_preparation() {
    let mut f = Fixture::new();
    let repoint = f.prepared_repoint();
    f.app.secret_state_root = Some(f._root.path().join("bucket"));
    f.assert_repoint_refused(repoint, "vault");
}
