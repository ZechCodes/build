use super::*;
use crate::app::fs::service_test;
use std::time::Instant;

const SERVICE: Duration = Duration::from_millis(275);

fn timed_mutation(method: &str) -> (f64, f64) {
    let (dir, repo) = init_repo();
    let clock = FrameClock::reporting_to(Arc::new(|_| {}));
    let mut app = qa_state(&repo, dir.path());
    app.frame_clock = clock.clone();
    let project_id = app.project_at(0).id.clone();
    let sources = [crate::workspace::WorkspaceSource {
        id: "files".into(),
        name: "files".into(),
        mount: "files".into(),
        path: repo.clone(),
        is_git: false,
        base_branch: "main".into(),
    }];
    let workspace = app
        .workspaces
        .begin(&project_id, "hold timing", &sources)
        .unwrap();
    let root = workspace.directories[0].path.clone();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "before\n").unwrap();
    let scope = json!({"workspace_id": workspace.id, "source_id": "files"});
    let sender = SessionSender::detached("timing");
    let mut params = scope.clone();
    params["parent"] = json!("");
    params["name"] = json!("created");
    match method {
        "fs.mkdir" => {
            params = json!({"parent": root, "name": "created"});
        }
        "fs.write" => {
            params = scope.clone();
            params["path"] = json!("notes.txt");
            params["expected_revision"] = json!(crate::scoped_file::revision_hex(b"before\n"));
            params["content_b64"] = json!(b64encode(b"after\n"));
        }
        "fs.uploadBegin" => {
            params["size"] = json!(0);
        }
        "fs.uploadChunk" | "fs.uploadFinish" | "fs.uploadAbort" => {
            let begin = upload_call(
                &mut app,
                &sender,
                "fs.uploadBegin",
                json!({
                    "workspace_id": workspace.id, "source_id": "files",
                    "parent": "", "name": "uploaded",
                    "size": if method == "fs.uploadChunk" { 4 * 1_048_576 } else { 0 },
                }),
            );
            assert_eq!(begin["ok"], true, "{begin:?}");
            params = json!({"upload_id": begin["result"]["upload_id"]});
            if method == "fs.uploadChunk" {
                params["offset"] = json!(0);
                params["content_b64"] = json!(b64encode(&vec![0x9b; 4 * 1_048_576]));
            }
        }
        _ => (),
    }
    let mut finished = workspace.clone();
    finished.status = crate::workspace::WorkspaceStatus::Finished;
    finished.archived_at = Some("2026-10-21T00:00:00Z".into());
    std::fs::write(
        workspace.root.join(crate::workspace::MANIFEST_FILE),
        serde_json::to_vec_pretty(&finished).unwrap(),
    )
    .unwrap();
    app.workspaces.reload().unwrap();
    assert_eq!(
        app.workspaces.get(&workspace.id).unwrap().status,
        crate::workspace::WorkspaceStatus::Finished
    );
    let state = app.shared();
    let handler = AppState::handler(state);
    service_test::install(|| std::thread::sleep(SERVICE));
    let started = Instant::now();
    let response = handler.call(sender, req(method, params));
    let elapsed = started.elapsed().as_secs_f64() * 1000.;
    service_test::clear();
    assert_eq!(response["ok"], true, "{method}: {response:?}");
    let stats = clock.stats();
    (
        stats["methods"][method]["held"]["max_ms"].as_f64().unwrap(),
        elapsed,
    )
}

#[test]
fn fs_mutations_leave_app_lock_during_injected_service_time() {
    let mut failures = Vec::new();
    for method in [
        "fs.mkdir",
        "fs.write",
        "fs.createDirectory",
        "fs.createFile",
        "fs.uploadBegin",
        "fs.uploadChunk",
        "fs.uploadFinish",
        "fs.uploadAbort",
    ] {
        let (held, elapsed) = timed_mutation(method);
        eprintln!(
            "task725 {method}: held_ms={held:.3} service_ms={} elapsed_ms={elapsed:.3}",
            SERVICE.as_millis()
        );
        assert!(
            elapsed >= SERVICE.as_secs_f64() * 1000.,
            "injection did not run for {method}"
        );
        if held >= 200. {
            failures.push((method, held));
        }
    }
    assert!(
        failures.is_empty(),
        "filesystem service held the app lock: {failures:?}"
    );
}

#[test]
fn deferred_file_stages_keep_the_original_result_type_check() {
    use crate::app::runtime::deferred::DeferredNext;
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let params = json!({"project_id":app.project_at(0).id});
    let (answer, job) = app.dispatch_deferring("git.status", &params);
    answer.unwrap();
    let done = job.unwrap().run();
    app.deferred_work = Some(crate::app::DeferredWork::External(Box::new(|| {
        Ok(json!({"branch":7}))
    })));
    let DeferredNext::Again(next) = app.apply_deferred_stage("git.status", &params, done) else {
        panic!("valid first stage did not continue");
    };
    let DeferredNext::Answered(answer) =
        app.apply_deferred_stage("git.status", &params, next.run())
    else {
        panic!("final stage did not answer");
    };
    assert!(
        answer.is_err(),
        "final stage lost the verb's result check: {answer:?}"
    );
    assert!(answer
        .unwrap_err()
        .contains("does not match the type api/v1 declares"));
}

fn activity_counter(dir: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(dir.join("store/build.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE fs_activity_count (writes INTEGER NOT NULL);
        INSERT INTO fs_activity_count VALUES (0);
        CREATE TRIGGER fs_activity_insert AFTER INSERT ON meta WHEN NEW.key='user_session'
        BEGIN UPDATE fs_activity_count SET writes=writes+1; END;
        CREATE TRIGGER fs_activity_update AFTER UPDATE ON meta WHEN NEW.key='user_session'
        BEGIN UPDATE fs_activity_count SET writes=writes+1; END;",
        )
        .unwrap();
    connection
}

fn activity_writes(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row("SELECT writes FROM fs_activity_count", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn deferred_file_stages_stamp_user_activity_once_after_final_success() {
    use crate::app::runtime::deferred::DeferredNext;
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let params = json!({"project_id":app.project_at(0).id,"paths":["README.md"]});
    let status = app.handle(req(
        "git.status",
        json!({"project_id":app.project_at(0).id}),
    ))["result"]
        .clone();
    let counter = activity_counter(dir.path());
    let (answer, job) = app.dispatch_deferring("git.stage", &params);
    answer.unwrap();
    let done = job.unwrap().run();
    app.deferred_work = Some(crate::app::DeferredWork::External(Box::new(move || {
        std::thread::sleep(Duration::from_millis(10));
        Ok(status)
    })));
    let DeferredNext::Again(next) = app.apply_deferred_stage("git.stage", &params, done) else {
        panic!("valid first stage did not continue");
    };
    let DeferredNext::Answered(answer) = app.apply_deferred_stage("git.stage", &params, next.run())
    else {
        panic!("final stage did not answer");
    };
    answer.unwrap();
    assert_eq!(
        activity_writes(&counter),
        1,
        "only final success may persist user activity"
    );
}

#[test]
fn upload_begin_stamps_user_activity_once_across_file_stages() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let counter = activity_counter(dir.path());
    let sender = SessionSender::detached("activity");
    service_test::install(|| std::thread::sleep(Duration::from_millis(10)));
    let response = upload_call(
        &mut app,
        &sender,
        "fs.uploadBegin",
        json!({
            "project_id":project_id,"parent":"","name":"one-save.bin","size":0,
        }),
    );
    service_test::clear();
    assert_eq!(response["ok"], true, "{response:?}");
    assert_eq!(
        activity_writes(&counter),
        1,
        "upload admission/service must save activity once"
    );
}
