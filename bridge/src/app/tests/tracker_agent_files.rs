//! Files an agent made itself, filed on an issue (#116).
//!
//! A reviewer's evidence used to be a list of `/tmp/...png` paths in a
//! comment, which nobody reading the issue from the app could open. An agent's
//! issue tools now take a file it produced by its full path; the bridge copies
//! it into the attachment store at intake, so the issue holds the bytes and not
//! a pointer to a file somebody may delete.

use super::tracker::{attached, filed, refused, tracked};
use super::tracker_tools::{call, coding_agent};
use super::*;
use crate::mcp::{BridgeAction, DoneServer};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nscreenshot";
const MP4: &[u8] = b"\x00\x00\x00\x18ftypmp42recording";

/// A workspace agent of a tracked project, an issue it can comment on, and a
/// scratch folder outside every store to make files in.
struct Scene {
    _root: tempfile::TempDir,
    _home: tempfile::TempDir,
    state: AppState,
    project_id: String,
    who: (String, String),
    issue_id: String,
    scratch: tempfile::TempDir,
}

fn scene() -> Scene {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, mut state, project_id) = tracked(&state_root);
    let who = coding_agent(&mut state, &project_id, "here");
    let issue = filed(&mut state, &project_id, "The board forgets a drag");
    Scene {
        _root: tmp,
        _home: home,
        state,
        project_id,
        who,
        issue_id: issue["id"].as_str().unwrap().to_string(),
        scratch: tempfile::tempdir().unwrap(),
    }
}

impl Scene {
    fn made(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.scratch.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.display().to_string()
    }

    fn comment(&mut self, attachments: Vec<Value>) -> Result<Value, String> {
        call(
            &mut self.state,
            &self.who,
            BridgeAction::TrackerCommentIssue {
                issue_id: self.issue_id.clone(),
                body: "Before and after.".into(),
                attachments,
                refs: Vec::new(),
                track: None,
                notify_user: None,
                mention_user: None,
            },
        )
    }

    fn bytes_of(&mut self, path: &Value) -> Vec<u8> {
        let read = self.state.handle(req(
            "issues.attachment",
            json!({ "issue_id": self.issue_id, "path": path }),
        ));
        assert_eq!(read["ok"], true, "{read:?}");
        crate::encoding::b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap()
    }
}

/// A screenshot and a recording the agent made are copied into the store and
/// typed from what they are, named after the file the agent pointed at.
#[test]
fn an_agent_attaches_a_screenshot_and_a_recording_it_made() {
    let mut scene = scene();
    let shot = scene.made("board-after.png", PNG);
    let recording = scene.made("drag.mp4", MP4);

    let said = scene
        .comment(vec![json!({ "path": shot }), json!({ "path": recording })])
        .expect("an agent attaches files it made");
    let files = said["comment"]["attachments"].as_array().unwrap().clone();
    assert_eq!(files.len(), 2, "{said:?}");
    assert_eq!(files[0]["name"], "board-after.png");
    assert_eq!(files[0]["mime"], "image/png");
    assert_eq!(files[0]["size"], PNG.len());
    assert_eq!(files[1]["name"], "drag.mp4");
    assert_eq!(files[1]["mime"], "video/mp4");
    for file in &files {
        let stored = file["path"].as_str().unwrap();
        assert!(
            !stored.starts_with(&scene.scratch.path().display().to_string()),
            "the issue names the store's copy, not the agent's file: {stored}"
        );
    }
    assert_eq!(scene.bytes_of(&files[1]["path"]), MP4.to_vec());
}

/// The copy is taken at intake: the agent cleaning up its scratch folder does
/// not take the picture off the issue.
#[test]
fn the_stored_copy_survives_the_source_being_deleted() {
    let mut scene = scene();
    let shot = scene.made("board.png", PNG);
    let said = scene.comment(vec![json!({ "path": shot })]).unwrap();
    std::fs::remove_file(&shot).unwrap();

    let path = said["comment"]["attachments"][0]["path"].clone();
    assert_eq!(scene.bytes_of(&path), PNG.to_vec());
}

/// Over the cap is refused in a sentence, before a byte is copied. A sparse
/// file, so the test costs nothing to make.
#[test]
fn a_file_over_the_cap_is_refused_in_a_sentence() {
    let mut scene = scene();
    let path = scene.scratch.path().join("long.webm");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(crate::app::AGENT_ATTACHMENT_MAX_BYTES + 1)
        .unwrap();

    let refusal = scene
        .comment(vec![json!({ "path": path.display().to_string() })])
        .unwrap_err();
    assert_eq!(refusal, "Build cannot attach a file larger than 50 MB.");
}

/// A folder, a path with nothing at it, and a file the bridge may not read are
/// each refused with a sentence that names the path.
#[test]
fn a_folder_a_missing_path_or_an_unreadable_file_is_refused() {
    let mut scene = scene();
    let folder = scene.scratch.path().display().to_string();
    let missing = scene.scratch.path().join("gone.png").display().to_string();
    let locked = scene.made("locked.log", b"secret");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    for (path, sentence) in [
        (
            &folder,
            format!("Build cannot attach {folder}: it is a folder, not a file."),
        ),
        (
            &missing,
            format!("Build cannot attach {missing}: there is no file there."),
        ),
        (
            &locked,
            format!("Build cannot attach {locked}: it cannot be read."),
        ),
    ] {
        let refusal = scene.comment(vec![json!({ "path": path })]).unwrap_err();
        assert_eq!(refusal, sentence);
    }
}

/// A relative path that is not an attachment the agent was sent is refused:
/// the bridge does not guess which folder the agent meant.
#[test]
fn a_relative_path_is_refused_unless_it_names_an_attachment() {
    let mut scene = scene();
    let refusal = scene
        .comment(vec![json!({ "path": "shots/board.png" })])
        .unwrap_err();
    assert_eq!(
        refusal,
        "Build cannot attach shots/board.png: give the file's full path."
    );
}

/// Images, videos, and plain text or logs. A binary that is none of them is
/// refused, and a log comes through as text.
#[test]
fn only_media_and_plain_text_are_taken() {
    let mut scene = scene();
    let binary = scene.made("core.bin", b"\x7fELF\x00\x01");
    let refusal = scene.comment(vec![json!({ "path": binary })]).unwrap_err();
    assert_eq!(
        refusal,
        format!("Build cannot attach {binary}: only images (png, jpg, webp, gif), videos (mp4, webm) and plain text or logs can be attached.")
    );

    let log = scene.made("vitest.log", b"Test Files 1 failed\n");
    let said = scene.comment(vec![json!({ "path": log })]).unwrap();
    assert_eq!(said["comment"]["attachments"][0]["mime"], "text/plain");
    assert_eq!(said["comment"]["attachments"][0]["name"], "vitest.log");
}

/// What the user sent the agent is still handed on by its path, unchanged and
/// without a second copy.
#[test]
fn passing_through_a_users_attachment_still_works() {
    let mut scene = scene();
    let project_id = scene.project_id.clone();
    let sent = attached(&mut scene.state, &project_id, "board.png", PNG);

    let said = scene
        .comment(vec![json!({ "path": sent["path"], "name": "board.png" })])
        .unwrap();
    assert_eq!(said["comment"]["attachments"][0]["path"], sent["path"]);
    assert_eq!(said["comment"]["attachments"][0]["name"], "board.png");
}

/// `create_issue` takes them too, so an issue can be filed with its evidence.
#[test]
fn an_agent_files_an_issue_with_a_file_it_made() {
    let mut scene = scene();
    let shot = scene.made("broken.png", PNG);
    let created = call(
        &mut scene.state,
        &scene.who,
        BridgeAction::TrackerCreateIssue {
            title: "The rail overlaps the composer".into(),
            body: None,
            status: None,
            labels: Vec::new(),
            priority: None,
            attachments: vec![json!({ "path": shot })],
            track: None,
            notify_user: None,
        },
    )
    .unwrap();
    assert_eq!(created["issue"]["attachments"][0]["name"], "broken.png");
    assert_eq!(created["issue"]["attachments"][0]["mime"], "image/png");
}

/// The client verbs are unchanged: a path from a browser is hostile input, and
/// only an agent's tool copies a file in from the disk.
#[test]
fn the_client_still_cannot_file_a_path_outside_the_store() {
    let mut scene = scene();
    let shot = scene.made("board.png", PNG);
    let refusal = refused(
        &mut scene.state,
        "issues.comment",
        json!({ "issue_id": scene.issue_id, "body": "x", "attachments": [{ "path": shot }] }),
    );
    assert!(refusal.contains("not an attachment"), "{refusal}");
}

/// The whole path, unmocked: a real MCP `comment_issue` frame naming a local
/// file, through the tool's parse, the agent's action and the store, to the
/// issue the client reads.
#[test]
fn a_real_comment_issue_frame_files_a_local_file_the_client_reads() {
    let mut scene = scene();
    let shot = scene.made("lightbox-desktop.png", PNG);
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "comment_issue", "arguments": {
            "issue_id": scene.issue_id, "body": "Desktop screenshot.",
            "attachments": [{ "path": shot }]
        }}
    });
    let action = DoneServer::new(&scene.who.1)
        .handle_message(&frame.to_string())
        .action
        .expect("the frame emits a comment action");
    call(&mut scene.state, &scene.who.clone(), action).unwrap();
    std::fs::remove_file(&shot).unwrap();

    let read = scene
        .state
        .handle(req("issues.get", json!({ "issue_id": scene.issue_id })));
    let comment = read["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "comment")
        .expect("the comment is on the timeline")
        .clone();
    let file = &comment["attachments"][0];
    assert_eq!(file["name"], "lightbox-desktop.png", "{comment:?}");
    assert_eq!(file["mime"], "image/png");
    assert_eq!(file["size"], PNG.len());
    assert_eq!(scene.bytes_of(&file["path"]), PNG.to_vec());
}

/// A recording is bigger than one DataChannel message, so the bytes come back
/// in pieces: `offset` and `length` read a range, and the answer says where it
/// starts and how big the whole file is.
#[test]
fn an_attachment_reads_back_in_ranges() {
    let mut scene = scene();
    let body: Vec<u8> = (0..(crate::app::ATTACHMENT_READ_CHUNK_BYTES + 10))
        .map(|at| (at % 251) as u8)
        .collect();
    let recording = scene.made("drag.webm", &body);
    let said = scene.comment(vec![json!({ "path": recording })]).unwrap();
    let path = said["comment"]["attachments"][0]["path"].clone();

    let first = scene.state.handle(req(
        "issues.attachment",
        json!({ "issue_id": scene.issue_id, "path": path }),
    ));
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(first["result"]["size"], body.len());
    assert_eq!(first["result"]["offset"], 0);
    let head =
        crate::encoding::b64decode(first["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(
        head.len() as u64,
        crate::app::ATTACHMENT_READ_CHUNK_BYTES,
        "an unranged read answers one piece, never more"
    );

    let rest = scene.state.handle(req(
        "issues.attachment",
        json!({ "issue_id": scene.issue_id, "path": path, "offset": head.len() }),
    ));
    assert_eq!(rest["result"]["offset"], head.len());
    let tail = crate::encoding::b64decode(rest["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!([head, tail].concat(), body);

    let slice = scene.state.handle(req(
        "issues.attachment",
        json!({ "issue_id": scene.issue_id, "path": path, "offset": 3, "length": 4 }),
    ));
    let piece =
        crate::encoding::b64decode(slice["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(piece, body[3..7].to_vec());
}
