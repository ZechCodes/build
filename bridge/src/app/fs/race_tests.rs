use super::*;
use crate::app::fs::service_test;
use crate::carrier::FrameHandler;
use std::sync::mpsc;
use std::time::Instant;

/// Releases the worker even when an assertion panics, so a failed regression
/// cannot strand a test thread holding AppState.
struct BlockedService {
    release: mpsc::Sender<()>,
    worker: Option<std::thread::JoinHandle<Value>>,
}

impl BlockedService {
    fn start(handler: FrameHandler, sender: SessionSender, method: &str, params: Value) -> Self {
        let (entered, reached) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let frame = req(method, params);
        let worker = std::thread::spawn(move || {
            let mut first = true;
            service_test::install(move || {
                if !first {
                    return;
                }
                first = false;
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(10)).unwrap();
            });
            let result = handler.call(sender, frame);
            service_test::clear();
            result
        });
        let blocked = Self {
            release,
            worker: Some(worker),
        };
        reached
            .recv_timeout(Duration::from_secs(10))
            .expect("filesystem service did not start");
        blocked
    }

    fn finish(mut self) -> Value {
        self.release.send(()).unwrap();
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for BlockedService {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn filesystem_write_does_not_wait_for_a_git_sync_lock() {
    let (_dir, app, project_id, repo, _plain) = project_files_fixture();
    let held = crate::source_sync::SyncLock::acquire_many(
        std::slice::from_ref(&repo),
        Duration::from_secs(1),
    )
    .unwrap();
    let handler = AppState::handler(app.shared());
    let mut params = project_file_params(&project_id, "source-1", "notes.md");
    params["expected_revision"] = json!(sha256_hex(b"git source\n"));
    let (completed, received) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let answer = handler.call(SessionSender::detached("writer"), req("fs.write", params));
        completed.send(()).unwrap();
        answer
    });
    let prompt = received.recv_timeout(Duration::from_secs(1));
    drop(held);
    let answer = worker.join().unwrap();
    assert!(
        prompt.is_ok(),
        "filesystem write waited for Git's sync lock"
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(std::fs::read(repo.join("notes.md")).unwrap(), b"after\n");
}

#[test]
fn blocked_filesystem_service_leaves_app_state_accessible() {
    let (dir, repo) = init_repo();
    let app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let state = app.shared();
    let blocked = BlockedService::start(
        AppState::handler(state.clone()),
        SessionSender::detached("writer"),
        "fs.createFile",
        json!({"project_id": project_id, "parent": "", "name": "created"}),
    );
    assert!(
        state.try_lock().is_ok(),
        "filesystem service retained the app lock"
    );
    assert_eq!(blocked.finish()["ok"], true);
}

#[test]
fn same_root_concurrent_stale_writes_preserve_revision_conflict() {
    let (_dir, app, project_id, _repo, plain) = project_files_fixture();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let params = project_file_params(&project_id, "source-2", "notes.md");
    let first = BlockedService::start(
        handler.clone(),
        SessionSender::detached("first"),
        "fs.write",
        params.clone(),
    );
    let second = std::thread::spawn(move || {
        handler.call(SessionSender::detached("second"), req("fs.write", params))
    });
    assert_eq!(first.finish()["ok"], true);
    let refused = second.join().unwrap();
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("revision conflict"),
        "{refused}"
    );
    assert_eq!(std::fs::read(plain.join("notes.md")).unwrap(), b"after\n");
}

#[test]
fn ended_opening_cannot_publish_a_finish_waiting_in_filesystem_service() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("uploader");
    let id = begin_upload(&mut app, &sender, "ended.bin", 0, false);
    let state = app.shared();
    let blocked = BlockedService::start(
        AppState::handler(state),
        sender.clone(),
        "fs.uploadFinish",
        json!({"upload_id": id}),
    );
    sender.opening_ended();
    let refused = blocked.finish();
    assert_eq!(refused["error_code"], "not_found", "{refused}");
    assert!(!repo.join("ended.bin").exists());
    assert!(!repo.join(format!(".build-upload-{id}.part")).exists());
}

#[test]
fn abort_cancels_a_finish_blocked_in_filesystem_service() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("uploader");
    let id = begin_upload(&mut app, &sender, "aborted.bin", 0, false);
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let blocked = BlockedService::start(
        handler.clone(),
        sender.clone(),
        "fs.uploadFinish",
        json!({"upload_id": id}),
    );
    assert!(state.try_lock().is_ok(), "blocked finish retained app lock");
    let abort_id = id.clone();
    let abort = std::thread::spawn(move || {
        handler.call(
            sender,
            req("fs.uploadAbort", json!({"upload_id": abort_id})),
        )
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let cancelled = state
            .try_lock()
            .is_ok_and(|app| !app.uploads.contains_key(&id));
        if cancelled {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "abort did not cancel the blocked upload"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let refused = blocked.finish();
    assert_eq!(refused["error_code"], "not_found", "{refused}");
    let aborted = abort.join().unwrap();
    assert_eq!(aborted["ok"], true, "{aborted}");
    assert!(!repo.join("aborted.bin").exists());
    assert!(!repo.join(format!(".build-upload-{id}.part")).exists());
}

#[test]
fn upload_quota_counts_a_chunk_while_its_filesystem_service_is_blocked() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("uploader");
    let project_id = app.project_at(0).id.clone();
    let ids: Vec<_> = (0..8)
        .map(|index| begin_upload(&mut app, &sender, &format!("quota-{index}"), 1, false))
        .collect();
    let state = app.shared();
    let blocked = BlockedService::start(
        AppState::handler(state.clone()),
        sender.clone(),
        "fs.uploadChunk",
        json!({"upload_id": ids[0], "offset": 0, "content_b64": b64encode(b"x")}),
    );
    let refused = {
        let mut app = state.try_lock().expect("blocked chunk retained app lock");
        upload_call(
            &mut app,
            &sender,
            "fs.uploadBegin",
            json!({"project_id": project_id, "parent": "", "name": "overflow", "size": 0}),
        )
    };
    assert_eq!(refused["error_code"], "busy", "{refused}");
    assert_eq!(blocked.finish()["ok"], true);
    assert!(!repo.join("overflow").exists());
}

#[test]
fn source_moved_while_write_waits_for_same_root_is_rejected() {
    let (dir, app, project_id, _repo, plain) = project_files_fixture();
    let moved = dir.path().join("moved");
    std::fs::create_dir(&moved).unwrap();
    std::fs::write(moved.join("notes.md"), "plain source\n").unwrap();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let first = BlockedService::start(
        handler.clone(),
        SessionSender::detached("first"),
        "fs.createFile",
        json!({"project_id": project_id, "source_id": "source-2",
            "parent": "", "name": "occupy-root"}),
    );
    let mut params = project_file_params(&project_id, "source-2", "notes.md");
    params["source_path"] = json!(plain);
    let (completed, response) = mpsc::channel();
    let (starting, started) = mpsc::channel();
    let second = std::thread::spawn(move || {
        starting.send(()).unwrap();
        let result = handler.call(SessionSender::detached("second"), req("fs.write", params));
        completed.send(result).unwrap();
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    let early_response = response.recv_timeout(Duration::from_millis(50)).ok();
    let moved_during_service = if let Ok(mut app) = state.try_lock() {
        let mut sources = app.projects.get(&project_id).unwrap().sources.clone();
        sources[1].path = moved.clone();
        app.projects.set_sources(&project_id, sources)
    } else {
        false
    };
    let _ = first.finish();
    let serialized = early_response.is_none();
    let refused =
        early_response.unwrap_or_else(|| response.recv_timeout(Duration::from_secs(10)).unwrap());
    second.join().unwrap();
    assert!(serialized, "same-root write skipped serialization");
    assert!(moved_during_service, "root wait retained app lock");
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert_eq!(
        std::fs::read(plain.join("notes.md")).unwrap(),
        b"plain source\n"
    );
    assert_eq!(
        std::fs::read(moved.join("notes.md")).unwrap(),
        b"plain source\n"
    );
}
fn overlapping_sources_fixture() -> (tempfile::TempDir, AppState, String, PathBuf) {
    let (dir, mut app, project_id, _repo, plain) = project_files_fixture();
    let nested = plain.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("notes.md"), "plain source\n").unwrap();
    let mut sources = app.projects.get(&project_id).unwrap().sources.clone();
    sources.push(crate::app::projects::ProjectSource::added(
        "source-3".into(),
        "Nested folder".into(),
        "nested".into(),
        nested,
        false,
        "main".into(),
    ));
    assert!(app.projects.set_sources(&project_id, sources));
    (dir, app, project_id, plain)
}

fn overlapping_write_worker(
    handler: FrameHandler,
    params: Value,
) -> (mpsc::Receiver<()>, std::thread::JoinHandle<Value>) {
    let (entered, reached) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut first = true;
        service_test::install(move || {
            if first {
                first = false;
                let _ = entered.send(());
            }
        });
        let answer = handler.call(
            SessionSender::detached("overlapping-writer"),
            req("fs.write", params),
        );
        service_test::clear();
        answer
    });
    (reached, worker)
}

#[test]
fn overlapping_source_roots_serialize_revision_comparison_for_the_same_file() {
    let (_dir, app, project_id, plain) = overlapping_sources_fixture();
    let state = app.shared();
    let handler = AppState::handler(state);
    let first = BlockedService::start(
        handler.clone(),
        SessionSender::detached("outer-writer"),
        "fs.write",
        project_file_params(&project_id, "source-2", "nested/notes.md"),
    );
    let (reached, worker) = overlapping_write_worker(
        handler,
        project_file_params(&project_id, "source-3", "notes.md"),
    );
    let entered_early = reached.recv_timeout(Duration::from_millis(150)).is_ok();
    let first_answer = first.finish();
    let second_answer = worker.join().unwrap();
    assert!(
        !entered_early,
        "same destination entered service through overlapping source root"
    );
    assert_eq!(first_answer["ok"], true, "{first_answer}");
    assert_eq!(second_answer["ok"], false, "{second_answer}");
    assert!(
        second_answer["error"]
            .as_str()
            .unwrap()
            .contains("revision conflict"),
        "{second_answer}"
    );
    assert_eq!(
        std::fs::read(plain.join("nested/notes.md")).unwrap(),
        b"after\n"
    );
}

#[test]
fn upload_replacement_serializes_with_an_overlapping_source_editor_write() {
    let (_dir, mut app, project_id, plain) = overlapping_sources_fixture();
    let sender = SessionSender::detached("overlapping-uploader");
    let begin = upload_call(
        &mut app,
        &sender,
        "fs.uploadBegin",
        json!({
            "project_id": project_id, "source_id": "source-2", "parent": "nested",
            "name": "notes.md", "size": 9, "replace": true,
        }),
    );
    assert_eq!(begin["ok"], true, "{begin}");
    let id = begin["result"]["upload_id"].as_str().unwrap();
    assert_eq!(
        upload_bytes(&mut app, &sender, id, 0, b"uploaded\n")["ok"],
        true
    );
    let state = app.shared();
    let handler = AppState::handler(state);
    let first = BlockedService::start(
        handler.clone(),
        sender,
        "fs.uploadFinish",
        json!({"upload_id": id}),
    );
    let (reached, worker) = overlapping_write_worker(
        handler,
        project_file_params(&project_id, "source-3", "notes.md"),
    );
    let entered_early = reached.recv_timeout(Duration::from_millis(150)).is_ok();
    let first_answer = first.finish();
    let second_answer = worker.join().unwrap();
    assert!(
        !entered_early,
        "editor entered service while overlapping upload replacement held destination"
    );
    assert_eq!(first_answer["ok"], true, "{first_answer}");
    assert_eq!(second_answer["ok"], false, "{second_answer}");
    assert!(
        second_answer["error"]
            .as_str()
            .unwrap()
            .contains("revision conflict"),
        "{second_answer}"
    );
    assert_eq!(
        std::fs::read(plain.join("nested/notes.md")).unwrap(),
        b"uploaded\n"
    );
}
#[test]
fn upload_finish_discards_staging_after_source_root_disappears() {
    let (dir, mut app, project_id, _repo, plain) = project_files_fixture();
    let sender = SessionSender::detached("cleanup-uploader");
    let begin = upload_call(
        &mut app,
        &sender,
        "fs.uploadBegin",
        json!({
            "project_id": project_id, "source_id": "source-2", "parent": "",
            "name": "cleanup.bin", "size": 0,
        }),
    );
    assert_eq!(begin["ok"], true, "{begin}");
    let id = begin["result"]["upload_id"].as_str().unwrap();
    let moved = dir.path().join("moved-upload-root");
    std::fs::rename(&plain, &moved).unwrap();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let refused = handler.call(sender, req("fs.uploadFinish", json!({"upload_id": id})));
    assert_eq!(refused["ok"], false, "{refused}");
    assert_ne!(refused["error_code"], "busy", "{refused}");
    assert!(
        !state.lock().unwrap().uploads.contains_key(id),
        "fatal finish preparation retained upload quota"
    );
    assert!(
        !moved.join(format!(".build-upload-{id}.part")).exists(),
        "fatal finish preparation retained staging file"
    );
    assert!(!moved.join("cleanup.bin").exists());
}

fn finish_blocked_before_publish(
    handler: FrameHandler,
    sender: SessionSender,
    id: &str,
) -> BlockedService {
    let (entered, reached) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let frame = req("fs.uploadFinish", json!({"upload_id": id}));
    let worker = std::thread::spawn(move || {
        let mut first = true;
        service_test::install_before_publish(move || {
            if first {
                first = false;
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        });
        let answer = handler.call(sender, frame);
        service_test::clear_before_publish();
        answer
    });
    let blocked = BlockedService {
        release,
        worker: Some(worker),
    };
    reached
        .recv_timeout(Duration::from_secs(10))
        .expect("upload did not reach prepublication boundary");
    blocked
}

#[test]
fn opening_ended_after_finish_validation_cannot_publish_destination() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("late-ended-uploader");
    let id = begin_upload(&mut app, &sender, "late-ended.bin", 0, false);
    let state = app.shared();
    let blocked =
        finish_blocked_before_publish(AppState::handler(state.clone()), sender.clone(), &id);
    sender.opening_ended();
    let refused = blocked.finish();
    assert_eq!(refused["error_code"], "not_found", "{refused}");
    assert!(
        !repo.join("late-ended.bin").exists(),
        "ended opening published after finish validation"
    );
    assert!(!repo.join(format!(".build-upload-{id}.part")).exists());
    assert!(!state.lock().unwrap().uploads.contains_key(&id));
}

#[test]
fn finish_claim_won_before_opening_ended_returns_committed_success() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("claimed-uploader");
    let id = begin_upload(&mut app, &sender, "claimed.bin", 0, false);
    let state = app.shared();
    let (entered, reached) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let worker_sender = sender.clone();
    let handler = AppState::handler(state.clone());
    let finish_id = id.clone();
    let worker = std::thread::spawn(move || {
        service_test::install_after_publish_claim(move || {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let answer = handler.call(
            worker_sender,
            req("fs.uploadFinish", json!({"upload_id": finish_id})),
        );
        service_test::clear_after_publish_claim();
        answer
    });
    let blocked = BlockedService {
        release,
        worker: Some(worker),
    };
    reached
        .recv_timeout(Duration::from_secs(10))
        .expect("upload did not reach commit claim");
    sender.opening_ended();
    let finished = blocked.finish();
    assert_eq!(finished["ok"], true, "{finished}");
    assert!(repo.join("claimed.bin").exists());
    assert!(!repo.join(format!(".build-upload-{id}.part")).exists());
    assert!(!state.lock().unwrap().uploads.contains_key(&id));
}

#[test]
fn a_successful_replace_reopens_workspace_even_when_readback_fails() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let project_id = app.project_at(0).id.clone();
    let sources = [crate::workspace::WorkspaceSource {
        id: "files".into(),
        name: "files".into(),
        mount: "files".into(),
        path: repo,
        is_git: false,
        base_branch: "main".into(),
    }];
    let workspace = app
        .workspaces
        .begin(&project_id, "readback failure", &sources)
        .unwrap();
    let root = workspace.directories[0].path.clone();
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("notes.txt");
    std::fs::write(&file, "before\n").unwrap();
    let mut finished = workspace.clone();
    finished.status = crate::workspace::WorkspaceStatus::Finished;
    finished.archived_at = Some("2026-10-21T00:00:00Z".into());
    std::fs::write(
        workspace.root.join(crate::workspace::MANIFEST_FILE),
        serde_json::to_vec_pretty(&finished).unwrap(),
    )
    .unwrap();
    app.workspaces.reload().unwrap();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    service_test::install_after_replace(move || std::fs::remove_file(&file).unwrap());
    let answer = handler.call(
        SessionSender::detached("readback-writer"),
        req(
            "fs.write",
            json!({
                "workspace_id": workspace.id, "source_id": "files", "path": "notes.txt",
                "expected_revision": crate::scoped_file::revision_hex(b"before\n"),
                "content_b64": b64encode(b"after\n"),
            }),
        ),
    );
    service_test::clear_after_replace();
    assert_eq!(answer["ok"], false, "{answer}");
    assert_eq!(
        state
            .lock()
            .unwrap()
            .workspaces
            .get(&workspace.id)
            .unwrap()
            .status,
        crate::workspace::WorkspaceStatus::Ready,
        "successful replacement was lost when constructing its readback reply failed"
    );
}

#[test]
fn queued_finish_drops_staging_when_its_opening_ends_before_source_lock() {
    let (_dir, mut app, project_id, _repo, plain) = project_files_fixture();
    let sender = SessionSender::detached("queued-ended-uploader");
    let begin = upload_call(
        &mut app,
        &sender,
        "fs.uploadBegin",
        json!({
            "project_id": project_id, "source_id": "source-2", "parent": "",
            "name": "queued-ended.bin", "size": 0,
        }),
    );
    assert_eq!(begin["ok"], true, "{begin}");
    let id = begin["result"]["upload_id"].as_str().unwrap().to_string();
    let held = crate::source_sync::SyncLock::acquire_many(
        std::slice::from_ref(&plain),
        Duration::from_secs(1),
    )
    .unwrap();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let finish_id = id.clone();
    let worker_sender = sender.clone();
    let (starting, started) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        starting.send(()).unwrap();
        handler.call(
            worker_sender,
            req("fs.uploadFinish", json!({"upload_id": finish_id})),
        )
    });
    started.recv_timeout(Duration::from_secs(10)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    sender.opening_ended();
    drop(held);
    let refused = worker.join().unwrap();
    assert_eq!(refused["error_code"], "not_found", "{refused}");
    assert!(!plain.join("queued-ended.bin").exists());
    assert!(
        !state.lock().unwrap().uploads.contains_key(&id),
        "ended queued finish retained quota"
    );
    assert!(!plain.join(format!(".build-upload-{id}.part")).exists());
}

#[test]
fn finish_claim_won_before_abort_returns_success_and_abort_is_not_found() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let sender = SessionSender::detached("claimed-abort-uploader");
    let id = begin_upload(&mut app, &sender, "claimed-abort.bin", 0, false);
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let (entered, reached) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let finish_handler = handler.clone();
    let finish_sender = sender.clone();
    let finish_id = id.clone();
    let worker = std::thread::spawn(move || {
        service_test::install_after_publish_claim(move || {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let answer = finish_handler.call(
            finish_sender,
            req("fs.uploadFinish", json!({"upload_id": finish_id})),
        );
        service_test::clear_after_publish_claim();
        answer
    });
    let blocked = BlockedService {
        release,
        worker: Some(worker),
    };
    reached
        .recv_timeout(Duration::from_secs(10))
        .expect("upload did not reach commit claim");
    let abort_id = id.clone();
    let (done, aborted) = mpsc::channel();
    let abort_worker = std::thread::spawn(move || {
        let answer = handler.call(
            sender,
            req("fs.uploadAbort", json!({"upload_id": abort_id})),
        );
        done.send(answer).unwrap();
    });
    let early_abort = aborted.recv_timeout(Duration::from_millis(150)).ok();
    let finished = blocked.finish();
    let abort_answer =
        early_abort.unwrap_or_else(|| aborted.recv_timeout(Duration::from_secs(10)).unwrap());
    abort_worker.join().unwrap();
    assert_eq!(abort_answer["error_code"], "not_found", "{abort_answer}");
    assert_eq!(finished["ok"], true, "{finished}");
    assert!(repo.join("claimed-abort.bin").exists());
    assert!(!state.lock().unwrap().uploads.contains_key(&id));
}
