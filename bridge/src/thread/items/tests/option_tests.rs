use super::*;

fn option(id: &str, label: &str, message: Option<&str>) -> MessageOption {
    MessageOption {
        id: id.to_string(),
        label: label.to_string(),
        message: message.map(str::to_string),
    }
}

fn offered() -> Thread {
    let mut thread = Thread::new("run-1");
    thread.post_user("the tests are red", None, "2026-08-19T09:00:00Z");
    thread.post_agent_offering(
        "Two ways out. Which?",
        None,
        Vec::new(),
        vec![
            option(
                "option-1",
                "Revert it",
                Some("Revert the commit that turned the tests red."),
            ),
            option("option-2", "Fix forward", None),
        ],
        "2026-08-19T09:01:00Z",
        false,
    );
    thread
}

#[test]
fn an_answer_sends_the_options_longer_text_and_falls_back_to_the_label() {
    let mut thread = offered();
    let choice = OptionChoice {
        message_id: "message-2".to_string(),
        option_ids: vec!["option-1".to_string(), "option-2".to_string()],
    };

    assert_eq!(
        thread.option_reply_text(&choice).unwrap(),
        "Revert the commit that turned the tests red.\n\nFix forward"
    );
    thread
        .post_option_reply(&choice, "2026-08-19T09:02:00Z")
        .unwrap();
    let ThreadItem::Message(reply) = thread.items.last().unwrap() else {
        panic!("the reply is a message");
    };
    assert_eq!(reply.role, MessageRole::User);
    assert_eq!(reply.answers_options_of.as_deref(), Some("message-2"));
    assert!(reply.body.contains("Fix forward"));
}

/// The chat's only record of what was chosen, so it has to be on the
/// message that offered it rather than on the reply.
#[test]
fn the_choice_is_recorded_on_the_message_that_offered_it() {
    let mut thread = offered();
    thread
        .post_option_reply(
            &OptionChoice {
                message_id: "message-2".to_string(),
                option_ids: vec!["option-2".to_string()],
            },
            "2026-08-19T09:02:00Z",
        )
        .unwrap();

    let ThreadItem::Message(offer) = &thread.items[1] else {
        panic!("the offer is a message");
    };
    assert_eq!(offer.selected_options, vec!["option-2".to_string()]);
    // An in-place mutation of an already-sequenced item, so a cursored poll
    // re-ships it with the selection on it.
    assert!(offer.updated_sequence > offer.sequence, "{offer:?}");
}

#[test]
fn an_option_nobody_offered_is_refused_and_leaves_the_thread_alone() {
    let mut thread = offered();
    let choice = OptionChoice {
        message_id: "message-2".to_string(),
        option_ids: vec!["option-9".to_string()],
    };

    assert!(thread.option_reply_text(&choice).is_err());
    assert!(thread
        .post_option_reply(&choice, "2026-08-19T09:02:00Z")
        .is_err());
    assert_eq!(thread.items.len(), 2);
}

#[test]
fn answering_an_empty_set_is_refused() {
    let thread = offered();
    assert!(thread
        .option_reply_text(&OptionChoice {
            message_id: "message-2".to_string(),
            option_ids: Vec::new(),
        })
        .is_err());
}

#[test]
fn options_are_answered_once() {
    let mut thread = offered();
    let choice = OptionChoice {
        message_id: "message-2".to_string(),
        option_ids: vec!["option-1".to_string()],
    };
    thread
        .post_option_reply(&choice, "2026-08-19T09:02:00Z")
        .unwrap();

    assert!(thread
        .post_option_reply(&choice, "2026-08-19T09:03:00Z")
        .is_err());
}

/// What the reviewer sees disabled, the daemon refuses: a newer message —
/// from either side — closes the offer, and the race where one lands
/// between the render and the press must not send a stale answer.
#[test]
fn a_newer_message_closes_the_offer_and_an_event_does_not() {
    let mut thread = offered();
    let choice = OptionChoice {
        message_id: "message-2".to_string(),
        option_ids: vec!["option-1".to_string()],
    };
    thread.push_event(
        ThreadEventKind::Committed,
        Some("Changes committed".to_string()),
        None,
        None,
        "2026-08-19T09:02:00Z",
    );
    assert!(thread.option_reply_text(&choice).is_ok());

    thread.post_agent(
        "actually, I found a third way",
        None,
        "2026-08-19T09:03:00Z",
    );
    assert!(thread.option_reply_text(&choice).is_err());
}

#[test]
fn a_message_with_no_options_cannot_be_answered() {
    let thread = offered();
    assert!(thread
        .option_reply_text(&OptionChoice {
            message_id: "message-1".to_string(),
            option_ids: vec!["option-1".to_string()],
        })
        .is_err());
}
