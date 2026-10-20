//! The shared thread fixture carries evidence on both sides of agent delivery.
use build_bridge::thread::ThreadItem;
use serde_json::Value;

#[test]
fn agent_attachment_messages_round_trip_in_the_thread_page_contract() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/api/v1/thread.page.json")).unwrap();
    let items = fixture["result"]["items"].as_array().unwrap();
    for direction in ["reply", "sent", "received"] {
        let item = items
            .iter()
            .find(|item| {
                let message = &item["data"];
                let addressed = match direction {
                    "reply" => message["role"] == "agent" && message.get("sent_to").is_none(),
                    "sent" => message["role"] == "agent" && message.get("sent_to").is_some(),
                    _ => message["role"] == "user" && message.get("from_agent").is_some(),
                };
                addressed && message["attachments"].as_array().is_some_and(|a| a.len() == 2)
            })
            .unwrap_or_else(|| panic!("missing {direction} image/log attachment example"));
        let message: ThreadItem = serde_json::from_value(item.clone()).unwrap();
        let round_trip = serde_json::to_value(message).unwrap();
        assert_eq!(round_trip["data"]["attachments"], item["data"]["attachments"]);
        let attachments = item["data"]["attachments"].as_array().unwrap();
        assert_eq!(attachments[0]["mime"], "image/png");
        assert_eq!(attachments[1]["mime"], "text/plain");
        for attachment in attachments {
            assert!(attachment["path"].as_str().unwrap().starts_with('/'));
            assert!(!attachment["name"].as_str().unwrap().is_empty());
            assert!(attachment["size"].as_u64().unwrap() > 0);
        }
    }
}
