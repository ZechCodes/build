use super::*;

#[test]
fn file_and_git_observations_answer_while_the_app_mutex_is_held() {
    let (directory, repository) = init_repo();
    std::fs::write(repository.join("observation.txt"), "visible\n").unwrap();
    let app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let held = state.lock().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let replies = [
            ("fs.tree", json!({"project_id": project})),
            (
                "fs.read",
                json!({"project_id": project, "path": "observation.txt"}),
            ),
            ("git.status", json!({"project_id": project})),
        ]
        .map(|(method, params)| {
            handler.call(
                SessionSender::detached("observation-reader"),
                req(method, params),
            )
        });
        send.send(replies).unwrap();
    });
    let answers = receive.recv_timeout(Duration::from_millis(500));
    drop(held);
    worker.join().unwrap();
    let answers = answers.expect("file and Git reads must finish before releasing the app lock");
    for answer in &answers {
        assert_eq!(answer["ok"], true, "{answer}");
    }
    assert_eq!(answers[1]["result"]["content_b64"], b64encode(b"visible\n"));
}

#[cfg(unix)]
#[test]
fn observation_file_reads_keep_symlink_fences_and_byte_bounds() {
    let (directory, repository) = init_repo();
    let outside = directory.path().join("outside.txt");
    std::fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, repository.join("escape.txt")).unwrap();
    std::fs::write(repository.join("large.txt"), vec![b'x'; 1_048_577]).unwrap();
    let app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let handler = app.into_handler();
    let sender = SessionSender::detached("bounded-reader");
    for path in ["escape.txt", "../outside.txt"] {
        let answer = handler.call(
            sender.clone(),
            req(
                "fs.read",
                json!({
                    "project_id": project, "path": path
                }),
            ),
        );
        assert_eq!(answer["ok"], false, "{answer}");
    }
    let answer = handler.call(
        sender,
        req(
            "fs.read",
            json!({
                "project_id": project, "path": "large.txt"
            }),
        ),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["truncated"], true);
    let bytes =
        crate::encoding::b64decode(answer["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes.len(), 1_048_576);
}
