use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    let i = &e.inputs;
    match method {
        "tasks.review.get" => row(value, "/review/snapshots", "id", &i.snapshot)
            .as_object()
            .map(|_| ())
            .unwrap(),
        "tasks.review.diff" => {
            assert_eq!(value["path"], i.stable_file);
            bytes(value, &e.review_bytes);
            assert_eq!(value["editable"], false);
        }
        "tasks.attachment" => {
            assert_eq!(value["path"], i.task_attachment);
            assert_eq!(value["offset"], 0);
            bytes(value, &e.attachment_bytes);
        }
        "thread.attachment" => {
            assert_eq!(value["path"], i.thread_attachment);
            assert_eq!(value["offset"], 0);
            bytes(value, &e.attachment_bytes);
        }
        _ => return false,
    }
    true
}
