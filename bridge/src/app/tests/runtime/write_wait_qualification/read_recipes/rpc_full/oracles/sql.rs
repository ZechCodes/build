use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "thread.page" => {
            assert_eq!(value["thread_id"], e.thread_id);
            assert_eq!(value["thread_generation_revision"], e.thread_generation);
            assert_eq!(value["agent"]["id"], i.agent);
            assert!(item_body(value, "/items", &e.message_body));
            let message = array(value, "/items")
                .iter()
                .find(|item| item["type"] == "message" && item["data"]["body"] == e.message_body)
                .unwrap();
            assert_eq!(message["data"]["operation_id"], i.operation);
            assert!(array(value, "/items").len() <= 20);
        }
        "thread.activity" => {
            let activity = array(value, "/items")
                .iter()
                .find(|item| {
                    item["type"] == "event" && item["data"]["summary"] == e.activity_summary
                })
                .expect("real reserved activity event");
            let sequence = activity["data"]["sequence"].as_u64().unwrap();
            assert!((i.activity_from..=i.activity_through).contains(&sequence));
        }
        "thread.operation" => assert_eq!(value, &e.operation),
        "tasks.list" => {
            assert_eq!(value["project_id"], i.project);
            let task = row(value, "/tasks", "id", &i.task);
            assert_eq!(task["title"], e.task_title);
            assert_eq!(task["project_id"], i.project);
            assert!(array(value, "/tasks").len() <= 20);
            assert!(task["labels"].as_array().unwrap().contains(&json!(i.label)));
        }
        "tasks.get" => {
            assert_eq!(value["task"]["id"], i.task);
            assert_eq!(value["task"]["title"], e.task_title);
            assert_eq!(value["task"]["project_id"], i.project);
            let comment = row(value, "/timeline", "id", &e.comment);
            assert_eq!(comment["body"], e.comment_body);
            assert!(comment["attachments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["path"] == i.task_attachment));
        }
        "tasks.waiting_on.blocks" => {
            row(value, "/blocks", "task_id", &i.dependent);
            assert!(array(value, "/blocks").len() <= 20);
        }
        _ => return false,
    }
    true
}
