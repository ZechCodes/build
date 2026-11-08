//! Archive and session side effects must serialize at their existing reservation lock.
use super::*;
use crate::api::ApiError;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    handler: FrameHandler,
    project: String,
    spawns: Arc<std::sync::atomic::AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let (repo_home, repo) = super::super::project_coordinator::a_repo();
        let (root_home, root) = super::super::project_coordinator::a_state_root();
        let mut app = super::super::project_coordinator::booted(&root);
        let origin = repo_home.path().join("origin.git");
        git_in(
            repo_home.path(),
            &[
                "clone",
                "--bare",
                repo.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&repo, &["fetch", "origin"]);
        git_in(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
        let project = app.add_project(repo, "main".into());
        app.persist();
        let shell = root.join("finite-shell");
        std::fs::write(&shell, "#!/bin/sh\nwhile IFS= read -r line; do :; done\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        app.term_shell = shell.display().to_string();
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        {
            let mut app = state.lock().unwrap();
            // Snapshot initialization adopts the primary checkout. Finish it
            // normally so only the session under test can block archive.
            for mut workspace in app
                .workspaces
                .list(Some(&project))
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
            {
                app.workspaces.record_finished(&mut workspace).unwrap();
                app.workspaces.apply_local_capabilities(workspace).unwrap();
            }
        }
        Self {
            _repo: repo_home,
            _root: root_home,
            state,
            handler,
            project,
            spawns: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    fn create(&self) -> std::thread::JoinHandle<Value> {
        self.create_at(json!({"project_id":self.project}))
    }

    fn create_at(&self, params: Value) -> std::thread::JoinHandle<Value> {
        let handler = self.handler.clone();
        let spawns = Arc::clone(&self.spawns);
        std::thread::spawn(move || {
            crate::timing::observe_test_terminal_spawns(spawns);
            call(&handler, "term.create", params)
        })
    }

    fn archive(&self) -> Value {
        call(
            &self.handler,
            "project.archive",
            json!({"project_id":self.project}),
        )
    }

    fn close(&self, created: &Value) {
        if created["ok"] != true {
            return;
        }
        let child = {
            let app = self.state.lock().unwrap();
            let key = app
                .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
                .unwrap();
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session)
        };
        call(
            &self.handler,
            "term.close",
            json!({"term_id":created["result"]["term_id"]}),
        );
        child.end();
        assert!(child.exited_within(Duration::from_secs(2)));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_session_shell_create_refused_when_archive_lands_before_reservation() {
    let f = Fixture::new();
    let (gate, held) = OffLockGate::new();
    f.state.lock().unwrap().terminal_create_admission_gate = Some(gate);
    let creating = f.create();
    held.wait_for_arrival();
    let archived = f.archive();
    held.release();
    let created = creating.join().unwrap();
    let count = f.state.lock().unwrap().shell_tab_count();
    f.close(&created); // Retire a reproduced orphan before failing on the old code.
    assert_eq!(archived["ok"], true, "{archived}");
    assert_eq!(created["error_code"], "project_archived", "{created}");
    assert_eq!(count, 0);
    assert_eq!(f.spawns.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        f.state
            .lock()
            .unwrap()
            .session_registry
            .pending_shell_count(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_session_pending_shell_blocks_archive_until_settlement() {
    let f = Fixture::new();
    let (gate, held) = OffLockGate::new();
    f.state.lock().unwrap().terminal_create_spawn_gate = Some(gate);
    let creating = f.create();
    held.wait_for_arrival();
    let pending = f
        .state
        .lock()
        .unwrap()
        .session_registry
        .pending_shell_for_project(&f.project);
    let archived = f.archive();
    held.release();
    let created = creating.join().unwrap();
    f.close(&created);
    assert!(pending);
    assert_eq!(archived["error_code"], "conflict", "{archived}");
    assert!(
        archived["error"]
            .as_str()
            .unwrap()
            .contains("terminal creation"),
        "{archived}"
    );
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(f.spawns.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!f
        .state
        .lock()
        .unwrap()
        .session_registry
        .pending_shell_for_project(&f.project));
    assert_eq!(f.archive()["ok"], true);
}

fn run_bytes(store: &crate::store::Store, owner: &str) -> Vec<u8> {
    let record = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|record| record.id == owner)
        .unwrap();
    serde_json::to_vec(&record).unwrap()
}

#[test]
fn archive_session_refused_agent_start_keeps_changed_choice_record_byte_unchanged() {
    let f = Fixture::new();
    let (owner, pin, store, clock) = {
        let mut app = f.state.lock().unwrap();
        let (owner, pin) = super::super::project_agent::project_agent(&mut app, &f.project);
        (
            owner,
            pin,
            app.store.clone().unwrap(),
            Arc::clone(&app.frame_clock),
        )
    };
    assert_eq!(f.archive()["ok"], true);
    let before = run_bytes(&store, &owner);
    // Enter the endpoint after the unlocked preflight boundary, as in the race.
    let result = crate::app::agent_start(
        &f.state,
        &SessionSender::detached("late-start"),
        &json!({"id":owner,"agent_id":pin,"model":"archive-race-model"}),
        &clock.frame("agent.start"),
    );
    let error = match result {
        Err(crate::app::runtime::agents::endpoints::ScopedEndpointError::Domain(error)) => {
            ApiError::classify(error)
        }
        other => panic!("expected an archived-owner refusal: {other:?}"),
    };
    assert_eq!(error.code(), "project_archived");
    assert_eq!(
        run_bytes(&store, &owner),
        before,
        "a refused start must not persist its requested model"
    );
    assert!(f.state.lock().unwrap().delivery_queue.is_idle());
}

fn assert_live_shell_blocks_archive(f: &Fixture, params: Value) {
    let created = f.create_at(params).join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    assert_eq!(
        f.state
            .lock()
            .unwrap()
            .session_registry
            .pending_shell_count(),
        0
    );
    let archived = f.archive();
    f.close(&created); // Also retire the live shell when reproducing the old gap.
    assert_eq!(archived["error_code"], "conflict", "{archived}");
    assert!(
        archived["error"]
            .as_str()
            .unwrap()
            .contains("close the open terminal in this project"),
        "{archived}"
    );
    assert_eq!(f.archive()["ok"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_session_live_primary_shell_blocks_archive_until_closed() {
    let f = Fixture::new();
    assert_live_shell_blocks_archive(&f, json!({"project_id":f.project}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_session_live_source_shell_blocks_archive_until_closed() {
    let f = Fixture::new();
    let params = {
        let app = f.state.lock().unwrap();
        let source = app.project(&f.project).unwrap().sources.first().unwrap();
        json!({"project_id":f.project,"source_id":source.id,"source_path":source.path})
    };
    assert_live_shell_blocks_archive(&f, params);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archive_session_exited_but_listed_shell_does_not_block_archive() {
    let f = Fixture::new();
    let created = f.create().join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    // Hold the guard across process exit and admission so its EOF pump cannot
    // remove the listed shell before archive observes it. No production wait
    // or lock scope changes are needed to exercise this short-lived state.
    let mut app = f.state.lock().unwrap();
    let key = app
        .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
        .unwrap();
    let tab = app.session_registry.test_tab(&key).unwrap();
    let child = Arc::clone(&tab.session);
    child.end();
    assert!(child.exited_within(Duration::from_secs(2)));
    assert!(tab.live, "the EOF pump has not settled the listed tab");
    assert!(!tab.session_is_live());
    assert_eq!(app.shell_tab_count(), 1);
    assert!(app.project_archive(&f.project).is_ok());
}
