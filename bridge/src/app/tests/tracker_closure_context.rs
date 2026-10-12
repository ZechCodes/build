use super::project_agent::project_agent;
use super::tracker::{filed, tracked};
use super::*;
use crate::tracker::Actor;

fn id(task: &Value) -> &str {
    task["id"].as_str().unwrap()
}

#[test]
fn closed_blockers_delivery_context() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let (owner, agent) = project_agent(&mut app, &project);
    let dependent = filed(&mut app, &project, "dependent work");
    let closed = filed(&mut app, &project, "old prerequisite title");
    let also_closed = filed(&mut app, &project, "other abandoned prerequisite");
    let done = filed(&mut app, &project, "completed prerequisite");
    let path = app.tracker_project_path(&project).unwrap();
    app.tracker_store()
        .unwrap()
        .edit_waiting_on(
            &path,
            id(&dependent),
            &[id(&closed).into(), id(&also_closed).into(), id(&done).into()],
            &[],
            &Actor::User,
            &crate::store::now_rfc3339(),
        )
        .unwrap();

    for (task, update) in [
        (&closed, json!({"state":"closed", "title":"current prerequisite title"})),
        (&also_closed, json!({"state":"closed"})),
        (&done, json!({"state":"closed", "status":"done"})),
    ] {
        let mut params = update;
        params["task_id"] = json!(id(task));
        let answer = app.handle(req("tasks.update", params));
        assert_eq!(answer["ok"], true, "{answer}");
    }
    while app
        .tracker_store()
        .unwrap()
        .reconcile_waiting_on(2)
        .unwrap()
        .pending
    {}

    // Both closures precede admission. Done takes precedence over closed,
    // and the retained graph supplies the current title for every closure.
    let answer = app.handle(req(
        "tasks.assign",
        json!({"task_id":id(&dependent), "assignee":{"kind":"agent", "agent_id":agent}}),
    ));
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["dispatch"]["agent_id"], agent);
    let page = app.handle(req(
        "thread.page",
        json!({"entity_id":owner, "agent_id":agent, "limit":50}),
    ));
    assert_eq!(page["ok"], true, "{page}");
    let messages: Vec<_> = page["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .map(|item| &item["data"])
        .filter(|message| message["from_task"]["task_id"] == id(&dependent))
        .filter(|message| {
            message["body"]
                .as_str()
                .is_some_and(|body| body.starts_with("The user assigned you"))
        })
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let body = messages[0]["body"].as_str().unwrap();
    for (task, title) in [
        (&closed, "current prerequisite title"),
        (&also_closed, "other abandoned prerequisite"),
    ] {
        assert!(body.contains(id(task)), "{body}");
        assert!(body.contains(title), "{body}");
        assert!(body.contains(&format!("#{}", task["number"])), "{body}");
    }
    assert!(!body.contains("old prerequisite title"), "{body}");
    assert!(!body.contains(id(&done)), "{body}");
    assert!(!body.contains("completed prerequisite"), "{body}");
    for instruction in [
        "closed without Done",
        "Read",
        "assess",
        "proceed",
        "comment_task",
        "mention_user: true",
        "uncertain prerequisites",
        "decision needed",
        id(&dependent),
    ] {
        assert!(body.contains(instruction), "missing {instruction}: {body}");
    }
}
