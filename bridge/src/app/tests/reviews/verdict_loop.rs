//! A complete author/reviewer loop through the public adapters and the
//! client-independent verdict service, with the real persisted author inbox.
use super::*;
use crate::mcp::DoneServer;

fn tool(state: &mut AppState, owner: &str, agent: &str, name: &str, arguments: Value) -> Value {
    let action = DoneServer::for_owner(agent)
        .handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}})
            .to_string(),
        )
        .action
        .unwrap_or_else(|| panic!("{name} must parse"));
    state.agent_action(owner, agent, action).unwrap()
}

fn inbox_verdict(
    state: &mut AppState,
    owner: &str,
    agent: &str,
    verdict_comment: &str,
    instruction: &str,
) -> String {
    state.drain_review_verdict_deliveries().unwrap();
    let page = review_call(
        state,
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":100}),
    );
    let messages = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["data"]["body"].as_str())
        .filter(|body| body.contains(verdict_comment) && body.contains(instruction))
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 1, "{page}");
    messages[0].to_string()
}

fn assert_changes_batch(
    state: &AppState,
    changes: &Value,
    first_comment: &Value,
    second_comment: &Value,
    first: &Value,
    received_changes: &str,
) {
    let directory = &first["directories"][0];
    let journal = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(changes["comment"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(journal.revision, 1);
    assert_eq!(journal.batch.len(), 2);
    assert_eq!(journal.batch[0].comment_id, first_comment["comment"]["id"]);
    assert_eq!(journal.batch[0].body, "Keep the original heading.");
    assert_eq!(journal.batch[0].anchor.line, 1);
    assert_eq!(journal.batch[1].comment_id, second_comment["comment"]["id"]);
    assert_eq!(journal.batch[1].body, "Describe the behavior in this line.");
    assert_eq!(journal.batch[1].anchor.line, 2);
    assert_eq!(
        journal.batch[1].anchor.commit.as_deref(),
        directory["head"].as_str()
    );
    for expected in [
        first["id"].as_str().unwrap(),
        first_comment["comment"]["id"].as_str().unwrap(),
        second_comment["comment"]["id"].as_str().unwrap(),
        "README.md:1",
        "README.md:2",
        "old",
        "new",
        directory["head"].as_str().unwrap(),
    ] {
        assert!(received_changes.contains(expected), "{received_changes}");
    }
}

fn push_addressed_revision(
    state: &mut AppState,
    owner: &str,
    author: &str,
    working: &std::path::Path,
    opened: &Value,
    current: &Value,
) -> (Value, String) {
    let task_id = opened["task"]["id"].as_str().unwrap();
    let directory = &opened["review"]["snapshots"][0]["directories"][0];
    std::fs::write(
        working.join("README.md"),
        "# project\nThe revised behavior.\nsecond change\n",
    )
    .unwrap();
    git_in(working, &["add", "README.md"]);
    git_in(working, &["commit", "-m", "Address both review comments"]);
    let head = git2::Repository::open(working)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    let pushed = tool(
        state,
        owner,
        author,
        "push_review",
        json!({"task_id":task_id,"expected_version":current["review"]["version"],
            "sources":[{"directory_id":directory["id"],"expected_head":head,
                "expected_received_head":opened["review"]["bindings"][0]["last_received_head"]}]}),
    );
    assert_eq!(pushed["sources"][0]["status"], "published", "{pushed}");
    assert_eq!(pushed["review"]["snapshots"][1]["number"], 2);
    let rereview = review_call(state, "tasks.get", json!({"task_id":task_id}));
    assert_eq!(rereview["task"]["status"], "in_review");
    assert!(
        rereview["timeline"].as_array().unwrap().iter().any(|item| {
            item["mentions_user"] == true
                && item["body"] == "Revision 2 is ready; read since from revision 1."
        }),
        "{rereview}"
    );
    (pushed, head)
}

#[test]
fn author_receives_two_anchored_changes_then_approval_on_revision_two() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let (owner, author) = project_agent(&mut state, &project);
    let workspace_id = workspace(&mut state, &project, "verdict loop");
    let working = state.workspaces.get(&workspace_id).unwrap().directories[0]
        .path
        .clone();
    std::fs::write(
        working.join("README.md"),
        "# proposed\nfirst change\nsecond change\n",
    )
    .unwrap();
    git_in(&working, &["add", "README.md"]);
    git_in(&working, &["commit", "-m", "Propose review changes"]);
    let opened = tool(
        &mut state,
        &owner,
        &author,
        "open_review",
        json!({"workspace_id":workspace_id,"request_id":"verdict-loop",
            "reviewer":{"kind":"user"},
            "title":"Complete verdict loop","description":"Two anchored changes, then approval"}),
    );
    let task_id = opened["task"]["id"].as_str().unwrap();
    let first = &opened["review"]["snapshots"][0];
    let directory = &first["directories"][0];
    assert_eq!(opened["review"]["pull_request"]["author"], author);
    assert!(opened["task"]["trackers"]
        .as_array()
        .unwrap()
        .contains(&json!(author)));
    let first_comment = review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":task_id,"body":"Keep the original heading.",
            "anchor":{"snapshot_id":first["id"],"directory_id":directory["id"],
                "path":"README.md","line":1,"side":"old"}}),
    );
    let second_comment = review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":task_id,"body":"Describe the behavior in this line.",
            "anchor":{"snapshot_id":first["id"],"directory_id":directory["id"],
                "path":"README.md","line":2,"side":"new","commit":directory["head"]}}),
    );
    let changes = review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":task_id,"body":"Address both anchored comments.",
            "opinion":{"snapshot_id":first["id"],"verdict":"request_changes"}}),
    );
    let change_instruction =
        "Fix each comment, commit, `push_review`; reply on comments you disagree with.";
    let received_changes = inbox_verdict(
        &mut state,
        &owner,
        &author,
        changes["comment"]["id"].as_str().unwrap(),
        change_instruction,
    );
    assert_changes_batch(
        &state,
        &changes,
        &first_comment,
        &second_comment,
        first,
        &received_changes,
    );
    let current = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(
        current["review"]["pull_request"]["status"],
        "changes_requested"
    );
    let (pushed, head) =
        push_addressed_revision(&mut state, &owner, &author, &working, &opened, &current);
    let second = &pushed["review"]["snapshots"][1];
    let approved = review_call(
        &mut state,
        "tasks.comment",
        json!({"task_id":task_id,"body":"Both comments are addressed.",
            "opinion":{"snapshot_id":second["id"],"verdict":"approve"}}),
    );
    let approval_instruction = "Approved. Take this forward as the work requires";
    let received_approval = inbox_verdict(
        &mut state,
        &owner,
        &author,
        approved["comment"]["id"].as_str().unwrap(),
        approval_instruction,
    );
    assert!(received_approval.contains(second["id"].as_str().unwrap()));
    assert!(!received_approval.contains(first_comment["comment"]["id"].as_str().unwrap()));
    let complete = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(complete["review"]["pull_request"]["status"], "approved");
    println!("END-TO-END VERDICT LOOP\nAgent {author} open_review -> review #{}, revision 1 {}\nUser anchored comments: {} README.md:1 old; {} README.md:2 new commit {}\nUser request_changes: {}\nAuthor durable inbox:\n{received_changes}\nAuthor commit {head}; push_review -> revision 2 {}\nUser approve: {}\nAuthor durable inbox:\n{received_approval}\nEND TRANSCRIPT",
        opened["task"]["number"], first["id"], first_comment["comment"]["id"],
        second_comment["comment"]["id"], directory["head"], changes["comment"]["id"],
        second["id"], approved["comment"]["id"]);
}

#[test]
fn assigning_a_prior_advisory_approver_delivers_that_saved_batch_once() {
    let home = tempfile::tempdir().unwrap();
    let (_repository, mut state, project) = tracked(home.path());
    let (owner, author) = project_agent(&mut state, &project);
    let (reviewer_owner, reviewer) =
        super::super::tracker_tools::coding_agent(&mut state, &project, "advisory reviewer");
    let workspace_id = workspace(&mut state, &project, "advisory approval");
    let opened = tool(
        &mut state,
        &owner,
        &author,
        "open_review",
        json!({"workspace_id":workspace_id,"request_id":"advisory-approval",
            "title":"Promote an advisory approval","description":"The deciding reviewer changes"}),
    );
    let task_id = opened["task"]["id"].as_str().unwrap();
    let first = &opened["review"]["snapshots"][0];
    let anchored = tool(
        &mut state,
        &reviewer_owner,
        &reviewer,
        "comment_task",
        json!({"task_id":task_id,"body":"This heading is clear.",
            "anchor":{"snapshot_id":first["id"],"directory_id":first["directories"][0]["id"],
                "path":"README.md","line":1,"side":"new"}}),
    );
    let advisory = tool(
        &mut state,
        &reviewer_owner,
        &reviewer,
        "comment_task",
        json!({"task_id":task_id,"body":"Advisory approval.",
            "opinion":{"snapshot_id":first["id"],"verdict":"approve"}}),
    );
    let comment_id = advisory["comment"]["id"].as_str().unwrap();
    let current = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(current["review"]["pull_request"]["status"], "open");
    assert!(state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(comment_id)
        .unwrap()
        .is_none());
    let assignment = json!({"task_id":task_id,
        "assignee":{"kind":"agent","agent_id":reviewer}});
    review_call(&mut state, "tasks.assign", assignment.clone());
    let approved = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(approved["review"]["pull_request"]["status"], "approved");
    let received = inbox_verdict(
        &mut state,
        &owner,
        &author,
        comment_id,
        "Approved. Take this forward as the work requires",
    );
    assert!(received.contains(anchored["comment"]["id"].as_str().unwrap()));
    let journal = state
        .store
        .as_ref()
        .unwrap()
        .load_review_verdict_delivery(comment_id)
        .unwrap()
        .unwrap();
    assert_eq!(journal.batch.len(), 1);
    assert_eq!(journal.batch[0].comment_id, anchored["comment"]["id"]);
    review_call(
        &mut state,
        "tasks.assign",
        json!({"task_id":task_id,"assignee":{"kind":"user"}}),
    );
    let advisory_again = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(advisory_again["review"]["pull_request"]["status"], "open");
    review_call(&mut state, "tasks.assign", assignment);
    let reapproved = review_call(&mut state, "tasks.review.get", json!({"task_id":task_id}));
    assert_eq!(reapproved["review"]["pull_request"]["status"], "approved");
    let repeated = inbox_verdict(
        &mut state,
        &owner,
        &author,
        comment_id,
        "Approved. Take this forward as the work requires",
    );
    assert_eq!(repeated, received);
    assert_eq!(
        state
            .store
            .as_ref()
            .unwrap()
            .load_review_verdict_delivery(comment_id)
            .unwrap()
            .unwrap()
            .operation_id,
        journal.operation_id
    );
}
