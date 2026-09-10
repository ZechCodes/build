use super::*;

// ---- thread.attach: files sent with a conversation message -------------

pub(in crate::app::tests) const ONE_PIXEL_PNG: &[u8] =
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR one pixel";

/// The reviewer's file has to become something the AGENT can open. It lands
/// in the worktree the agent already works in, the message carries the path,
/// and the agent's mail hands it that path verbatim.
#[test]
fn an_attached_file_lands_in_the_worktree_and_rides_the_message() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "attach a screenshot");
    let worktree = state.runs[&run_id].worktree.path.clone();

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");
    let attachment = attached["result"].clone();
    assert_eq!(attachment["name"], "screenshot.png");
    assert_eq!(attachment["mime"], "image/png");
    assert_eq!(attachment["size"], ONE_PIXEL_PNG.len());
    let path = attachment["path"].as_str().unwrap().to_string();
    assert!(
        path.starts_with(".build/attachments/"),
        "an attachment lives in its own fenced folder: {path}"
    );
    assert_eq!(
        std::fs::read(worktree.join(&path)).unwrap(),
        ONE_PIXEL_PNG,
        "the bytes must be on disk before the message references them"
    );

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "body": "the button is misaligned here",
            "attachments": [attachment],
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let unread = state
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    let message = &unread["messages"][0];
    assert_eq!(message["body"], "the button is misaligned here");
    assert_eq!(message["attachments"][0]["path"], path);
    assert_eq!(message["attachments"][0]["name"], "screenshot.png");
}

/// Attachments are conversation, not work. They must never show up as an
/// uncommitted change in the diff the human is reviewing, nor ride the
/// agent's own `git add -A`.
#[test]
fn an_attachment_is_invisible_to_git() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "attachments stay out of the diff");
    let worktree = state.runs[&run_id].worktree.path.clone();

    state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));

    // -uall, because the default collapses an untracked directory to one
    // line and would pass whether or not the ignore rule exists.
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(&worktree)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status.stdout).to_string();
    assert!(
        !status.contains("attachments"),
        "git must not see the attachment: {status}"
    );
}

/// A filename comes straight from the reviewer's machine, so it is hostile
/// input: traversal, separators and control characters all get flattened
/// into one leaf that cannot leave the attachments folder.
#[test]
fn a_hostile_filename_cannot_escape_the_attachments_folder() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "hostile names");
    let worktree = state.runs[&run_id].worktree.path.clone();

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "../../../../etc/passwd",
            "content_b64": b64encode(b"root:x:0:0"),
        }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");
    let path = attached["result"]["path"].as_str().unwrap().to_string();
    assert!(path.starts_with(".build/attachments/"), "{path}");
    assert!(!path.contains(".."), "{path}");
    let written = std::fs::canonicalize(worktree.join(&path)).unwrap();
    assert!(
        written.starts_with(std::fs::canonicalize(&worktree).unwrap()),
        "the file must land inside the worktree: {written:?}"
    );
}

/// The relay carries one frame per attachment, so an upload bigger than the
/// frame is refused with a limit the client can show — not a dropped socket.
#[test]
fn an_oversized_attachment_is_refused_with_its_limit() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "too big");

    let refused = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "huge.bin",
            "content_b64": b64encode(&vec![0u8; ATTACHMENT_MAX_BYTES as usize + 1]),
        }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains(&ATTACHMENT_MAX_BYTES.to_string()),
        "the refusal must name the limit: {refused:?}"
    );
}

/// A message may only reference bytes this bridge wrote. Otherwise a post
/// is an arbitrary-path read primitive dressed up as a conversation.
#[test]
fn posting_cannot_reference_a_path_the_bridge_did_not_write() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "no smuggling");

    for path in [
        ".build/mcp.json",
        "../../etc/hosts",
        ".build/attachments/ghost.png",
    ] {
        let refused = state.handle(req(
            "thread.post",
            json!({
                "entity_id": run_id,
                "body": "look",
                "attachments": [{ "name": "x", "path": path, "mime": "image/png", "size": 1 }],
            }),
        ));
        assert_eq!(refused["ok"], false, "{path} must be refused: {refused:?}");
    }
}

/// The reviewer's own view has to render what they sent, and the browser
/// cannot reach the disk — so the bytes come back through the same entity
/// that took them, with no worktree scope for the caller to get wrong.
#[test]
fn an_attachment_reads_back_for_the_surface_that_sent_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "read it back");

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    let path = attached["result"]["path"].as_str().unwrap().to_string();

    let read = state.handle(req(
        "thread.attachment",
        json!({ "entity_id": run_id, "path": path }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
    assert_eq!(read["result"]["mime"], "image/png");
    assert_eq!(
        b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap(),
        ONE_PIXEL_PNG
    );

    let escaped = state.handle(req(
        "thread.attachment",
        json!({ "entity_id": run_id, "path": ".build/mcp.json" }),
    ));
    assert_eq!(
        escaped["ok"], false,
        "only attachments read back: {escaped:?}"
    );
}

/// A screenshot on its own IS the message. Demanding a caption for a file
/// the reviewer already chose to send would just produce "see attached".
#[test]
fn a_file_can_be_sent_with_no_words_at_all() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "wordless send");

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "body": "",
            "attachments": [attached["result"].clone()],
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let unread = state
        .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(unread["messages"][0]["body"], "");
    assert_eq!(
        unread["messages"][0]["attachments"][0]["name"],
        "screenshot.png"
    );

    // Only an attachment earns the exemption: an empty send with nothing on
    // it is still a mistake, and posting it would look like a dropped edit.
    let empty = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "   " }),
    ));
    assert_eq!(empty["ok"], false, "{empty:?}");
}

/// A conversation outlives its checkouts — implementations get archived —
/// so a screenshot sent last week must not render as a broken image once
/// its tree is gone.
#[test]
fn an_attachment_outlives_the_worktree_it_was_written_into() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "outlive the checkout");
    let worktree = state.runs[&run_id].worktree.path.clone();

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": run_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    let path = attached["result"]["path"].as_str().unwrap().to_string();
    state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "body": "here is the mock",
            "attachments": [attached["result"].clone()],
        }),
    ));

    std::fs::remove_dir_all(worktree.join(".build").join("attachments")).unwrap();
    let read = state.handle(req(
        "thread.attachment",
        json!({ "entity_id": run_id, "path": path }),
    ));
    assert_eq!(read["ok"], true, "the durable copy answers: {read:?}");
    assert_eq!(
        b64decode(read["result"]["content_b64"].as_str().unwrap()).unwrap(),
        ONE_PIXEL_PNG
    );
}

/// An entity with no checkout of its own still takes files — that window is
/// exactly when a mock is most useful — and is handed an absolute path,
/// since there is no tree for a relative one to mean anything against.
#[test]
fn an_entity_without_a_checkout_still_takes_an_attachment() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req(
        "plan.create",
        json!({ "goal": "attach before dispatch" }),
    ));
    let plan_id = plan_id_of(&plan);
    // plan.create gives the issue a workspace; drop it to stand in for
    // every entity whose checkout does not exist yet or no longer does.
    state.plans.get_mut(&plan_id).unwrap().workspace = None;

    let attached = state.handle(req(
        "thread.attach",
        json!({
            "entity_id": plan_id,
            "filename": "screenshot.png",
            "content_b64": b64encode(ONE_PIXEL_PNG),
        }),
    ));
    assert_eq!(attached["ok"], true, "{attached:?}");
    let path = attached["result"]["path"].as_str().unwrap().to_string();
    assert!(
        std::path::Path::new(&path).is_absolute(),
        "with no checkout to be relative to, the path must be openable as-is: {path}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), ONE_PIXEL_PNG);

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": plan_id,
            "body": "here is the mock",
            "attachments": [attached["result"].clone()],
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let read = state.handle(req(
        "thread.attachment",
        json!({ "entity_id": plan_id, "path": path }),
    ));
    assert_eq!(read["ok"], true, "{read:?}");
}
