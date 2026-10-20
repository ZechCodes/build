//! Agent-authored chat files use the same intake policy as task evidence.
use super::project_agent::{added_project, items, project_agent, rooted};
use super::tracker_tools::coding_agent;
use super::*;
use crate::mcp::{DoneServer, McpSurface};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nchat screenshot";

struct Scene {
    _root: tempfile::TempDir,
    _home: tempfile::TempDir,
    state: AppState,
    project: String,
    who: (String, String),
    scratch: tempfile::TempDir,
}

fn scene() -> Scene {
    let root = tempfile::tempdir().unwrap();
    let (home, repo) = init_repo();
    let mut state = boot(root.path());
    let project = added_project(&mut state, &std::fs::canonicalize(repo).unwrap());
    let who = coding_agent(&mut state, &project, "chat");
    Scene {
        _root: root,
        _home: home,
        state,
        project,
        who,
        scratch: tempfile::tempdir().unwrap(),
    }
}

fn boot(root: &Path) -> AppState {
    rooted(root)
        .with_config(root.join("config.json"))
        .unwrap()
        .with_task_store(root.join("store"))
        .unwrap()
}

impl Scene {
    fn made(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.scratch.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        let server = DoneServer::launched(
            &self.who.1,
            Some(
                self.state
                    .agent_mcp_surface(&self.who.0, &self.who.1)
                    .as_str(),
            ),
        );
        let parsed = server.handle_message(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":tool,"arguments":arguments}}).to_string());
        let action = parsed
            .action
            .ok_or_else(|| parsed.reply.unwrap_or_default())?;
        self.state.agent_action(&self.who.0, &self.who.1, action)
    }

    fn post(&mut self, attachments: Value) -> Result<Value, String> {
        self.call(
            "post_thread_message",
            json!({"status":"Working", "body":"Screenshot and log.", "attachments":attachments}),
        )
    }

    fn read(&mut self, who: &(String, String), path: &Value) -> Vec<u8> {
        let read = self.state.handle(req(
            "thread.attachment",
            json!({"entity_id":who.0,"agent_id":who.1,"path":path,"offset":0,"length":1024}),
        ));
        assert_eq!(read["ok"], true, "{read:?}");
        crate::encoding::b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap()
    }
}

#[test]
fn chat_attachment_schema_matches_task_files_on_both_working_surfaces() {
    for surface in [McpSurface::Coding, McpSurface::Project] {
        let server = DoneServer::for_owner(if surface == McpSurface::Project {
            "project-attachment"
        } else {
            "agent-attachment"
        });
        let list = server.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let reply: Value = serde_json::from_str(&list.reply.unwrap()).unwrap();
        for tool in ["post_thread_message", "message_agent"] {
            let schema = &reply["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"] == tool)
                .unwrap()["inputSchema"]["properties"]["attachments"];
            assert_eq!(schema["type"], "array", "{surface:?} {tool}: {schema}");
            assert_eq!(schema["maxItems"], 10);
            assert_eq!(schema["items"]["required"], json!(["path"]));
            assert_eq!(schema["items"]["properties"]["name"]["type"], "string");
            assert!(schema["description"].as_str().unwrap().contains("50 MB"));
        }
    }
}

#[test]
fn chat_files_are_copied_typed_persisted_and_read_after_source_deletion() {
    let mut scene = scene();
    let shot = scene.made("screen.png", PNG);
    let log = scene.made("tests.log", b"passed\n");
    scene
        .post(json!([{ "path": shot, "name":"../Review.png" }, { "path":log }]))
        .unwrap();
    std::fs::remove_file(shot).unwrap();
    let outgoing = items(&mut scene.state, &scene.who.0, &scene.who.1)
        .into_iter()
        .find(|item| item["data"]["body"] == "Screenshot and log.")
        .unwrap();
    let files = &outgoing["data"]["attachments"];
    assert_eq!(files[0]["name"], "Review.png", "{outgoing}");
    assert_eq!(files[0]["mime"], "image/png");
    assert_eq!(files[0]["size"], PNG.len());
    assert_eq!(files[1]["name"], "tests.log");
    assert_eq!(files[1]["mime"], "text/plain");
    assert_eq!(scene.read(&scene.who.clone(), &files[0]["path"]), PNG);
    scene.state = boot(scene._root.path());
    let reloaded = items(&mut scene.state, &scene.who.0, &scene.who.1);
    assert!(
        reloaded
            .iter()
            .any(|item| item["data"]["attachments"] == *files),
        "{reloaded:?}"
    );
    assert_eq!(scene.read(&scene.who.clone(), &files[0]["path"]), PNG);
    let persisted = scene
        .state
        .store
        .as_ref()
        .unwrap()
        .thread_items(&scene.who.1)
        .unwrap();
    let wire = serde_json::to_value(persisted).unwrap();
    assert!(wire.to_string().contains("Review.png"), "{wire}");
}

#[test]
fn project_agent_can_attach_files_in_its_own_chat() {
    let mut scene = scene();
    scene.who = project_agent(&mut scene.state, &scene.project);
    let shot = scene.made("project.png", PNG);
    scene.post(json!([{ "path":shot }])).unwrap();
    let messages = items(&mut scene.state, &scene.who.0, &scene.who.1);
    let file = &messages
        .iter()
        .find(|item| item["data"]["body"] == "Screenshot and log.")
        .unwrap()["data"]["attachments"][0];
    assert_eq!(file["mime"], "image/png");
    assert_eq!(scene.read(&scene.who.clone(), &file["path"]), PNG);
}

#[test]
fn sending_files_records_both_sides_and_freezes_them_in_the_delivery_payload() {
    let mut scene = scene();
    let target = coding_agent(&mut scene.state, &scene.project, "recipient");
    let shot = scene.made("handoff.png", PNG);
    let sent = scene.call("message_agent", json!({"agent_id":target.1,"body":"Inspect the screenshot.","attachments":[{"path":shot}]})).unwrap();
    std::fs::remove_file(shot).unwrap();
    for who in [scene.who.clone(), target.clone()] {
        let messages = items(&mut scene.state, &who.0, &who.1);
        let file = &messages
            .iter()
            .find(|item| item["data"]["body"] == "Inspect the screenshot.")
            .unwrap()["data"]["attachments"][0];
        assert_eq!(file["name"], "handoff.png", "{messages:?}");
        assert_eq!(scene.read(&who, &file["path"]), PNG);
    }
    let receipt = scene
        .state
        .operation_receipt(sent["operation_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let payload = receipt.delivery.unwrap().payload.unwrap();
    assert_eq!(payload.messages[0].attachments.len(), 1);
    let path = &payload.messages[0].attachments[0].path;
    assert_eq!(
        std::fs::read(scene.state.runs[&target.0].worktree.path.join(path)).unwrap(),
        PNG
    );
    scene.state = boot(scene._root.path());
    let replay = items(&mut scene.state, &target.0, &target.1);
    assert!(replay
        .iter()
        .any(|item| item["data"]["attachments"][0]["name"] == "handoff.png"));
    let stored = scene
        .state
        .store
        .as_ref()
        .unwrap()
        .operation(sent["operation_id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.delivery.unwrap().payload.unwrap().messages[0].attachments,
        payload.messages[0].attachments
    );
}

#[test]
fn chat_attachment_intake_refuses_bad_paths_types_counts_and_large_files_without_a_message() {
    let mut scene = scene();
    let binary = scene.made("core.bin", b"\x00\xffELF");
    let fake = scene.made("fake.png", b"plain text");
    let oversized = scene.scratch.path().join("huge.webm");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(crate::app::AGENT_ATTACHMENT_MAX_BYTES + 1)
        .unwrap();
    for (files, expected) in [
        (json!([{ "path":"relative.png" }]), "full path"),
        (
            json!([{ "path":scene.scratch.path().join("missing.png") }]),
            "no file there",
        ),
        (json!([{ "path":scene.scratch.path() }]), "folder"),
        (json!([{ "path":binary }]), "only images"),
        (json!([{ "path":fake }]), "not a PNG"),
        (json!([{ "path":oversized }]), "50 MB"),
        (json!(vec![json!({"path":"x"}); 11]), "10 files"),
        (json!({"path":"x"}), "array"),
        (json!([{}]), "path"),
    ] {
        let error = scene.post(files).unwrap_err();
        assert!(error.contains(expected), "{error}: expected {expected}");
    }
    assert!(items(&mut scene.state, &scene.who.0, &scene.who.1)
        .iter()
        .all(|item| item["data"]["body"] != "Screenshot and log."));
}

#[test]
fn chat_attachment_intake_refuses_links_and_does_not_copy_partial_batches() {
    let mut scene = scene();
    let shot = scene.made("good.png", PNG);
    let link = scene.scratch.path().join("link.png");
    std::os::unix::fs::symlink(&shot, &link).unwrap();
    let bad = scene.made("bad.png", b"not png");
    let store = scene.state.local_attachments_dir();
    let before = std::fs::read_dir(&store)
        .map(|files| files.count())
        .unwrap_or(0);
    for files in [
        json!([{ "path":link }]),
        json!([{ "path":shot }, { "path":bad }]),
    ] {
        assert!(scene.post(files).is_err());
    }
    assert_eq!(
        std::fs::read_dir(store)
            .map(|files| files.count())
            .unwrap_or(0),
        before
    );
}

#[test]
fn forwarding_a_stored_chat_attachment_keeps_shared_bytes_when_sender_is_cleared() {
    let mut scene = scene();
    let shot = scene.made("forward.png", PNG);
    scene.post(json!([{ "path":shot }])).unwrap();
    let outgoing = items(&mut scene.state, &scene.who.0, &scene.who.1)
        .into_iter()
        .find(|item| item["data"]["body"] == "Screenshot and log.")
        .unwrap();
    let file = outgoing["data"]["attachments"][0].clone();
    assert!(
        file.is_object(),
        "the original chat stores its file: {outgoing}"
    );
    let target = coding_agent(&mut scene.state, &scene.project, "forwarded");
    scene
        .call(
            "message_agent",
            json!({"agent_id":target.1,"body":"Forwarded image.","attachments":[file]}),
        )
        .unwrap();
    let retained = scene
        .state
        .store
        .as_ref()
        .unwrap()
        .reset_attachment_leaves(&scene.who.1)
        .unwrap();
    let leaf = Path::new(file["path"].as_str().unwrap())
        .file_name()
        .unwrap()
        .to_string_lossy();
    assert!(
        !retained.contains(leaf.as_ref()),
        "sender reset must retain shared bytes"
    );
    let source_copy = scene.state.runs[&scene.who.0]
        .worktree
        .path
        .join(file["path"].as_str().unwrap());
    std::fs::remove_file(source_copy).unwrap();
    let inbound = items(&mut scene.state, &target.0, &target.1)
        .into_iter()
        .find(|item| item["data"]["body"] == "Forwarded image.")
        .unwrap();
    let path = &inbound["data"]["attachments"][0]["path"];
    assert_eq!(scene.read(&target, path), PNG);
    assert_eq!(
        std::fs::read(
            scene.state.runs[&target.0]
                .worktree
                .path
                .join(path.as_str().unwrap())
        )
        .unwrap(),
        PNG
    );
}

#[test]
fn chat_attachment_actions_still_decode_older_text_only_messages() {
    for value in [
        json!({"action":"post_thread_message", "still_working":true, "body":"Working", "options":[]}),
        json!({"action":"message_agent", "agent_id":"other", "body":"Hello"}),
    ] {
        let action: crate::mcp::BridgeAction = serde_json::from_value(value).unwrap();
        assert!(serde_json::to_value(action).unwrap()["attachments"].is_null());
    }
}

#[test]
fn a_refused_chat_file_does_not_emit_a_complete_report() {
    let mut scene = scene();
    let source = scene.made("bad.png", b"not a picture");
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"Finished with evidence.","attachments":[{"path":source}]}}});
    let mut output = Vec::new();
    let mut reports = Vec::new();
    DoneServer::new(&scene.who.1)
        .run_stdio(
            std::io::Cursor::new(format!("{frame}\n")),
            &mut output,
            |report| reports.push(report),
            |action| scene.state.agent_action(&scene.who.0, &scene.who.1, action),
        )
        .unwrap();
    assert!(
        reports.is_empty(),
        "the failed post must not finish the turn"
    );
    let reply: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert!(items(&mut scene.state, &scene.who.0, &scene.who.1)
        .iter()
        .all(|item| item["data"]["body"] != "Finished with evidence."));
}

#[test]
fn a_refused_agent_target_does_not_copy_its_files() {
    let mut scene = scene();
    let source = scene.made("unsent.png", PNG);
    for target in [scene.who.1.clone(), "agent-missing".to_string()] {
        assert!(scene
            .call(
                "message_agent",
                json!({"agent_id":target,"body":"Unsent.","attachments":[{"path":source}]})
            )
            .is_err());
    }
    assert!(!scene.state.local_attachments_dir().exists());
}

#[test]
fn a_large_agent_chat_recording_reads_in_transport_sized_pieces() {
    let mut scene = scene();
    let mut bytes = b"\x1a\x45\xdf\xa3webm".to_vec();
    bytes.resize(ATTACHMENT_MAX_BYTES as usize + 37, 42);
    let source = scene.made("recording.webm", &bytes);
    scene.post(json!([{ "path":source }])).unwrap();
    let outgoing = items(&mut scene.state, &scene.who.0, &scene.who.1)
        .into_iter()
        .find(|item| item["data"]["body"] == "Screenshot and log.")
        .unwrap();
    let path = &outgoing["data"]["attachments"][0]["path"];
    let read = scene.state.handle(req(
        "thread.attachment",
        json!({"entity_id":scene.who.0,"agent_id":scene.who.1,"path":path}),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(read["result"]["size"], bytes.len());
    let head = crate::encoding::b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(head.len() as u64, ATTACHMENT_MAX_BYTES);
    let rest = scene.state.handle(req(
        "thread.attachment",
        json!({"entity_id":scene.who.0,"agent_id":scene.who.1,"path":path,"offset":head.len()}),
    ));
    let tail = crate::encoding::b64decode(rest["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!([head, tail].concat(), bytes);
}
