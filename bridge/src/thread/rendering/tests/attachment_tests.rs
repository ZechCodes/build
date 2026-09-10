use super::*;

fn image() -> MessageAttachment {
    MessageAttachment {
        name: "screenshot.png".to_string(),
        path: ".build/attachments/ab12cd34-screenshot.png".to_string(),
        mime: "image/png".to_string(),
        size: 4096,
    }
}

/// The agent reads its mail as JSON, so a file the reviewer attached only
/// exists for it if the path rides the message it was sent with.
#[test]
fn an_attached_file_rides_the_message_the_agent_reads() {
    let mut thread = Thread::new("run-1");
    thread.post_user_with_attachments("look at this", None, vec![image()], "2026-08-09T13:00:00Z");

    let unread = thread.read_unread("2026-08-09T13:00:01Z");
    assert_eq!(unread.len(), 1);
    assert_eq!(unread[0].attachments, vec![image()]);
    let wire = serde_json::to_value(&unread[0]).expect("a message serializes");
    assert_eq!(
        wire["attachments"][0]["path"],
        ".build/attachments/ab12cd34-screenshot.png"
    );
}

/// A resumed agent rebuilds the conversation from the catch-up packet, not
/// from its mailbox — so a file sent to a previous session has to be named
/// there too, or it silently stops existing across a restart.
#[test]
fn the_catch_up_packet_still_names_the_files_a_message_carried() {
    let mut thread = Thread::new("run-1");
    thread.post_user_with_attachments("look at this", None, vec![image()], "2026-08-09T13:00:00Z");
    let catch_up = thread.catch_up_markdown(40);
    assert!(
        catch_up.contains(".build/attachments/ab12cd34-screenshot.png"),
        "{catch_up}"
    );
}

/// Every message written before attachments existed must still load, and a
/// message without them must not pay for the field on the wire.
#[test]
fn a_message_without_attachments_carries_no_attachment_field() {
    let mut thread = Thread::new("run-1");
    thread.post_user("no files here", None, "2026-08-09T13:00:00Z");
    let wire = serde_json::to_value(&thread).expect("a thread serializes");
    assert!(!wire.to_string().contains("attachments"), "{wire:?}",);
    let reloaded: Thread = serde_json::from_value(wire).expect("a thread reloads");
    assert_eq!(reloaded, thread);
}
