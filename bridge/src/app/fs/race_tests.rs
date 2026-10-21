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
