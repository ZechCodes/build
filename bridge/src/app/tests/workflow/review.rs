use super::*;

#[tokio::test]
async fn list_surfaces_carry_thread_digests_without_message_bodies() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let plan = call(
        &handler,
        "plan.create",
        json!({ "goal": "digest the board" }),
    );
    let plan_id = plan_id_of(&plan);
    // A plan whose only event would be its agent's session start carries
    // whatever that background spawn managed; this one is driven here.
    let approved = call(
        &handler,
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    );
    assert!(approved["error"].is_null(), "{approved:?}");
    let (_, run_id) = planned_run_in_review_delivered(&handler, "a run to digest");
    // Seed a real user message into each conversation so the assertions
    // below prove bodies are omitted, not merely absent.
    call(
        &handler,
        "plan.send_notes",
        json!({
            "plan_id": plan_id,
            "messages": [{ "body": "plan-only-body-marker", "anchor": null }]
        }),
    );
    call(
        &handler,
        "run.request_changes",
        json!({
            "run_id": run_id,
            "messages": [{ "body": "run-only-body-marker", "anchor": null }]
        }),
    );

    let board = call(&handler, "board.list", json!({}));
    let listed_plan = row_with(&board["result"]["plans"], "plan_id", &plan_id);
    let listed_run = row_with(&board["result"]["runs"], "run_id", &run_id);
    for thread in [&listed_plan["thread"], &listed_run["thread"]] {
        assert!(thread.get("items").is_none(), "{thread:?}");
        assert!(thread["item_count"].as_u64().unwrap() > 0, "{thread:?}");
        assert!(thread["last_sequence"].as_u64().unwrap() > 0, "{thread:?}");
        assert!(thread["last_event"]["event"].is_string(), "{thread:?}");
    }
    let serialized_board = board.to_string();
    assert!(!serialized_board.contains("plan-only-body-marker"));
    assert!(!serialized_board.contains("run-only-body-marker"));

    let listed = call(&handler, "plan.list", json!({}));
    let listed_plan = row_with(&listed["result"]["plans"], "plan_id", &plan_id);
    assert!(listed_plan["thread"].get("items").is_none(), "{listed:?}");

    // The detail surfaces must not regress: full threads, bodies intact.
    let plan_view = call(&handler, "plan.get", json!({ "plan_id": plan_id }));
    assert!(plan_view.to_string().contains("plan-only-body-marker"));
    let run_view = call(&handler, "run.get", json!({ "run_id": run_id }));
    assert!(run_view.to_string().contains("run-only-body-marker"));
}

#[tokio::test]
async fn detail_gets_with_a_cursor_ship_only_newer_thread_items() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let (_, run_id) = planned_run_in_review_delivered(&handler, "cursor the thread");
    call(
        &handler,
        "run.request_changes",
        json!({
            "run_id": run_id,
            "messages": [{ "body": "tighten the loop", "anchor": null }]
        }),
    );
    // The delivered fixture has a real harness whose exit can append
    // lifecycle items. Hold the state across this paging snapshot so every
    // response describes the same immutable conversation revision.
    let mut state = state.lock().unwrap();

    // The first load a paging client makes: the newest page, which for a
    // conversation this short is every item it holds.
    let full = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_limit": crate::thread::DEFAULT_THREAD_PAGE }),
    ));
    let full_items = full["result"]["thread"]["items"].as_array().unwrap();
    assert!(full_items.len() >= 2, "{full:?}");
    assert_eq!(full["result"]["thread"]["has_more"], false, "{full:?}");
    let total = full_items.len() as u64;
    assert_eq!(full["result"]["thread"]["thread_total"], total);
    let last_sequence = full_items.last().unwrap()["data"]["sequence"]
        .as_u64()
        .unwrap();
    let cursor = full_items[full_items.len() - 2]["data"]["sequence"]
        .as_u64()
        .unwrap();

    let delta = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_after_sequence": cursor }),
    ));
    let delta_thread = &delta["result"]["thread"];
    let delta_items = delta_thread["items"].as_array().unwrap();
    assert!(!delta_items.is_empty(), "{delta:?}");
    assert!(delta_items
        .iter()
        .all(|item| item["data"]["sequence"].as_u64().unwrap() > cursor));
    assert_eq!(delta_thread["thread_total"], total);
    assert_eq!(delta_thread["thread_last_sequence"], last_sequence);

    // A cursor past the end is an empty delta, never an error.
    let drained = state.handle(req(
        "run.get",
        json!({ "run_id": run_id, "thread_after_sequence": last_sequence + 100 }),
    ));
    assert_eq!(drained["ok"], true, "{drained:?}");
    assert_eq!(
        drained["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(drained["result"]["thread"]["thread_total"], total);

    // A garbage cursor is treated as absent: the page the poll asked for,
    // not an error.
    let garbage = state.handle(req(
        "run.get",
        json!({
            "run_id": run_id,
            "thread_after_sequence": "junk",
            "thread_limit": crate::thread::DEFAULT_THREAD_PAGE,
        }),
    ));
    assert_eq!(garbage["ok"], true, "{garbage:?}");
    assert_eq!(
        garbage["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .len(),
        full_items.len()
    );
}

/// The wire is bounded on a steady-state poll; the daemon has to be too.
///
/// Every poll after a client's first load carries a cursor and no limit, so
/// the delta it gets back is a handful of items at most. The view underneath
/// it used to build the conversation whole anyway and then have it replaced
/// unread — every item of it through serde, on every poll, for every open
/// browser. That is the cost paging exists to remove, and only counting it
/// can tell it apart from the answer, which was correct all along.
#[test]
fn a_cursored_detail_poll_serializes_the_delta_and_not_the_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "poll a long conversation");
    let now = crate::store::now_rfc3339();
    for n in 0..200 {
        primary_thread_mut(&mut state.plans.get_mut(&issue_id).unwrap().agents).post_user(
            format!("message {n}"),
            None,
            now.clone(),
        );
    }
    let conversation = &state.plans[&issue_id].agents.sole_thread();
    let total = conversation.total_item_count();
    assert!(total > 200, "the conversation is long enough to matter");
    let last_sequence = conversation.last_sequence();

    // The branch surface polls by project and branch rather than by id.
    let branch = state.runs[&run_id].worktree.branch();
    let project_id = state.project_at(0).id.clone();
    let polls = [
        ("run.get", json!({ "run_id": run_id })),
        ("plan.get", json!({ "plan_id": issue_id })),
        (
            "branch.get",
            json!({ "project_id": project_id, "branch": branch }),
        ),
    ];
    for (method, base) in polls {
        let mut params = base;
        params["thread_after_sequence"] = json!(last_sequence);
        let before = crate::thread::items_serialized();
        let answer = state.handle(req(method, params));
        let serialized = crate::thread::items_serialized() - before;

        assert_eq!(answer["ok"], true, "{answer:?}");
        let thread = match method {
            "branch.get" => &answer["result"]["run"]["thread"],
            _ => &answer["result"]["thread"],
        };
        assert_eq!(thread["items"].as_array().unwrap().len(), 0, "{answer:?}");
        assert_eq!(thread["thread_total"], total, "{answer:?}");
        assert!(
            serialized < 8,
            "{method} put {serialized} conversation items through serde \
             to answer with a delta of none"
        );
    }
}

#[tokio::test]
async fn plan_get_honors_the_thread_cursor() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let plan = call(
        &handler,
        "plan.create",
        json!({ "goal": "cursor the plan" }),
    );
    let plan_id = plan_id_of(&plan);

    let full = call(&handler, "plan.get", json!({ "plan_id": plan_id }));
    let full_items = full["result"]["thread"]["items"].as_array().unwrap();
    assert!(!full_items.is_empty(), "{full:?}");
    let last_sequence = full_items.last().unwrap()["data"]["sequence"]
        .as_u64()
        .unwrap();

    let delta = call(
        &handler,
        "plan.get",
        json!({ "plan_id": plan_id, "thread_after_sequence": last_sequence }),
    );
    let delta_thread = &delta["result"]["thread"];
    assert_eq!(delta_thread["items"].as_array().unwrap().len(), 0);
    assert_eq!(delta_thread["thread_total"], full_items.len() as u64);
    assert_eq!(delta_thread["thread_last_sequence"], last_sequence);
}

#[test]
fn state_changed_at_moves_on_transitions_but_not_same_state_mutations() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "changed change");
    let entry = |res: &Value| {
        res["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["run_id"] == json!(run_id.clone()))
            .unwrap()
            .clone()
    };
    let at_review = entry(&state.handle(req("board.list", json!({}))));
    assert_eq!(at_review["state"], "review", "{at_review:?}");
    let review_stamp = at_review["state_changed_at"].as_str().unwrap().to_string();
    let review_updated = at_review["updated_at"].as_str().unwrap().to_string();

    // A same-state git mutation advances updated_at but never the stamp.
    // Every built stage commits its own work, so dirty the worktree first.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let worktree = state.runs[&run_id].worktree.path.clone();
    std::fs::write(worktree.join("scratch.txt"), "reviewer edit\n").unwrap();
    let staged = state.handle(req(
        "git.stage",
        json!({ "run_id": run_id, "paths": ["scratch.txt"] }),
    ));
    assert_eq!(staged["ok"], true, "{staged:?}");
    let committed = state.handle(req(
        "git.commit",
        json!({ "run_id": run_id, "message": "keep" }),
    ));
    assert_eq!(committed["ok"], true, "{committed:?}");
    let after_commit = entry(&state.handle(req("board.list", json!({}))));
    assert_eq!(after_commit["state"], "review", "{after_commit:?}");
    assert_eq!(after_commit["state_changed_at"], json!(review_stamp));
    assert_ne!(after_commit["updated_at"], json!(review_updated));

    // Merging is a real transition: the stamp moves with it.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge" }),
    ));
    assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
    assert_ne!(merged["result"]["state_changed_at"], json!(review_stamp));
}

#[test]
fn state_changed_at_survives_a_daemon_restart() {
    let (dir, repo) = init_repo();
    let run_id;
    let stamp;
    {
        let mut state = qa_state(&repo, dir.path());
        run_id = planned_run_in_review(&mut state, "restartable").1;
        let got = state.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(got["result"]["state"], "review", "{got:?}");
        stamp = got["result"]["state_changed_at"]
            .as_str()
            .unwrap()
            .to_string();
    } // daemon dies

    let mut reloaded = qa_state(&repo, dir.path());
    let got = reloaded.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(got["result"]["state"], "review", "{got:?}");
    assert_eq!(got["result"]["state_changed_at"], json!(stamp));

    // A restored entity's first same-state mutation must not false-stamp:
    // the last-observed state is seeded from the record on boot. A commit
    // runs the full mutation tail but keeps the run in review.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let committed = reloaded.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "commit" }),
    ));
    assert_eq!(committed["result"]["state"], "review", "{committed:?}");
    assert_eq!(committed["result"]["state_changed_at"], json!(stamp));
}

#[test]
fn deleting_a_run_worktree_removes_it_from_the_board_and_plan_docs_survive() {
    let (dir, repo) = init_repo();
    let plan_id;
    let run_id;
    let doc_before;
    {
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "doomed run" })));
        plan_id = plan_id_of(&plan);
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        let doc = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        doc_before = doc["result"]["contents"].as_str().unwrap().to_string();

        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        run_id = run_id_of(&run);
        let worktree = state.runs.get(&run_id).unwrap().worktree.path.clone();
        std::fs::remove_dir_all(&worktree).unwrap();

        // The next board poll retires the run to internal archived history.
        let board = state.handle(req("board.list", json!({})));
        assert!(board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["run_id"] != run_id));
        assert_eq!(state.runs[&run_id].run.state, RunState::Archived);

        // The plan and its docs are untouched — they were never in the run.
        let doc = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(doc["result"]["contents"].as_str().unwrap(), doc_before);
    } // daemon dies

    let mut reloaded = qa_state(&repo, dir.path());
    let got = reloaded.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(got["result"]["state"], "archived", "{got:?}");
    let doc = reloaded.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert_eq!(doc["result"]["contents"].as_str().unwrap(), doc_before);
    // An archived run can be cleared off the board.
    let deleted = reloaded.handle(req("run.delete", json!({ "run_id": run_id })));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert_eq!(deleted["result"]["retained_as_issue_lineage"], true);
    assert!(reloaded.runs.contains_key(&run_id));
}

#[test]
fn boot_recreates_a_missing_issue_worktree_from_its_original_branch() {
    let (dir, repo) = init_repo();
    let issue_id;
    let run_id;
    let worktree;
    {
        let mut state = qa_state(&repo, dir.path());
        (issue_id, run_id) = planned_run_in_review(&mut state, "restore lineage");
        worktree = state.runs[&run_id].worktree.path.clone();
        assert!(Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.to_str().unwrap()
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        assert!(!worktree.exists());
    }

    let mut restored = qa_state(&repo, dir.path());
    assert!(worktree.exists(), "the original checkout path is recreated");
    let run = restored.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(run["result"]["state"], "review", "{run:?}");
    let issue = restored.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert!(
        issue["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "event"
                && item["data"]["event"] == "worktree_recreated"
                && item["data"]["links"][0]["implementation_id"] == run_id),
        "{issue:?}"
    );
}

#[test]
fn single_active_writer_rejects_a_second_run_of_one_plan() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "one writer" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    let first = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    assert_eq!(first["ok"], true, "{first:?}");
    let second = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    assert_eq!(second["ok"], false, "{second:?}");
    assert!(
        second["error"]
            .as_str()
            .unwrap()
            .contains("single-active-writer"),
        "{second:?}"
    );
}

#[test]
fn plan_comments_crud_and_stage_send_notes_resolve() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "comment me" })));
    let plan_id = plan_id_of(&plan);

    let general = state.handle(req(
        "plan.comment_add",
        json!({ "plan_id": plan_id, "stage_id": "first-half", "body": "split further" }),
    ));
    assert_eq!(
        general["result"]["comment"]["anchor"],
        Value::Null,
        "{general:?}"
    );
    let anchored = state.handle(req(
        "plan.comment_add",
        json!({
            "plan_id": plan_id, "stage_id": "first-half", "body": "use a timestamp",
            "anchor": { "heading_path": ["Stage: First half"], "snippet": "the first half" },
        }),
    ));
    let anchored_id = anchored["result"]["comment"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    assert_eq!(stages["result"]["stages"][0]["open_comments"], 2);

    let upd = state.handle(req(
        "plan.stage_send_notes",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert_eq!(upd["result"]["state"], "plan_review", "{upd:?}");
    let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    let first = stages["result"]["stages"][0].clone();
    assert_eq!(first["state"], "planned");
    assert_eq!(first["open_comments"], 0);
    assert!(first["comments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["state"] == "addressed" && c["agent_reply"] == "QA: addressed."));
    let doc = state.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert!(doc["result"]["contents"]
        .as_str()
        .unwrap()
        .contains("(revised)"));

    // No open comments left → sending notes again errors.
    let bad = state.handle(req(
        "plan.stage_send_notes",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    assert!(
        bad["error"].as_str().unwrap().contains("no open comments"),
        "{bad:?}"
    );
    // Deleting an already-addressed comment is rejected.
    let del = state.handle(req(
        "plan.comment_delete",
        json!({ "plan_id": plan_id, "comment_id": anchored_id }),
    ));
    assert_eq!(del["error"], "only open comments can be deleted");
}

/// A plan-doc comment is a post on the Issue agent's conversation, anchored
/// to the passage it is about — and the stage viewer reads its comments
/// back off that conversation. There is no second record.
#[test]
fn a_stage_comment_is_a_doc_anchored_post_the_stage_view_reads_back() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "comment as a post" })));
    let plan_id = plan_id_of(&plan);

    let added = state.handle(req(
        "plan.comment_add",
        json!({
            "plan_id": plan_id, "stage_id": "first-half", "body": "use a timestamp",
            "anchor": {
                "heading_path": ["Stage: First half"],
                "snippet": "the first half",
                "line_start": 12,
                "line_end": 14,
            },
        }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let comment = added["result"]["comment"].clone();
    let comment_id = comment["id"].as_str().unwrap().to_string();
    assert_eq!(comment["state"], "open", "{comment:?}");
    assert_eq!(comment["anchor"]["line_start"], 12, "{comment:?}");
    assert_eq!(
        comment["anchor"]["snippet"], "the first half",
        "{comment:?}"
    );
    assert_eq!(
        comment["path"], ".build/plan/01-first-half.md",
        "{comment:?}"
    );

    // The post itself: one anchored user message on the Issue conversation.
    let thread = &state.plans[&plan_id].agents.sole_thread();
    let posted = thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message) if message.id == comment_id => {
                Some(message)
            }
            _ => None,
        })
        .next()
        .unwrap_or_else(|| panic!("the comment is the post: {:?}", thread.items));
    let anchor = posted.anchor.as_ref().expect("an anchored post");
    assert_eq!(anchor.artifact, crate::thread::ArtifactKind::Doc);
    assert_eq!(anchor.path.as_deref(), Some(".build/plan/01-first-half.md"));
    assert_eq!(posted.body, "use a timestamp");

    // The stage viewer reads it back off the conversation.
    let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    let first = stages["result"]["stages"][0].clone();
    assert_eq!(first["open_comments"], 1, "{first:?}");
    assert_eq!(first["comments"][0]["id"], json!(comment_id), "{first:?}");

    // Deleting the comment deletes the post: there is nowhere else it is.
    let deleted = state.handle(req(
        "plan.comment_delete",
        json!({ "plan_id": plan_id, "comment_id": comment_id }),
    ));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert!(
        primary_thread(&state.plans[&plan_id].agents)
            .doc_comments()
            .is_empty(),
        "{:?}",
        primary_thread(&state.plans[&plan_id].agents).items
    );
    let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    assert_eq!(stages["result"]["stages"][0]["open_comments"], 0);
}

/// The comments survive a restart because the conversation does — and a
/// record written before comments were posts brings its comments across.
#[test]
fn stage_comments_survive_a_restart_as_posts() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "durable comments" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req(
        "plan.comment_add",
        json!({ "plan_id": plan_id, "stage_id": "first-half", "body": "split further" }),
    ));

    let mut restarted = qa_state(&repo, dir.path());
    let comments = primary_thread(&restarted.plans[&plan_id].agents).doc_comments();
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert_eq!(comments[0].body, "split further");
    let stages = restarted.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    assert_eq!(stages["result"]["stages"][0]["open_comments"], 1);
}

#[test]
fn multi_stage_run_gate_rejections() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "gate this" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate");

    // Unknown stage id.
    let unknown = state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "no-such" }),
    ));
    assert!(
        unknown["error"].as_str().unwrap().contains("no-such"),
        "{unknown:?}"
    );

    // Stage 2 not yet approved → rejected.
    let unapproved = state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert!(
        unapproved["error"]
            .as_str()
            .unwrap()
            .contains("not approved"),
        "{unapproved:?}"
    );
}

#[test]
fn mid_run_stage_send_notes_revises_the_plan_doc_from_the_stage_gate() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "revise me mid-run" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "second-half" }),
    ));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate");

    // Comment on the upcoming stage, then send the notes through the run.
    state.handle(req(
        "plan.comment_add",
        json!({ "plan_id": plan_id, "stage_id": "second-half", "body": "tighten this" }),
    ));
    let revised = state.handle(req(
        "run.stage_send_notes",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert_eq!(revised["ok"], true, "{revised:?}");
    assert_eq!(revised["result"]["state"], "stage_gate", "{revised:?}");

    // The doc revision landed in the canonical store and the approval
    // is stale again (planned), with the comment addressed.
    let doc = state.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": plan_id, "stage_id": "second-half" }),
    ));
    assert!(
        doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("(revised mid-run)"),
        "{doc:?}"
    );
    let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
    let second = stages["result"]["stages"][1].clone();
    assert_eq!(second["state"], "planned", "{second:?}");
    assert_eq!(second["open_comments"], 0, "{second:?}");

    // An adopted run implements no plan, so it has nothing to revise.
    let adopted_id = adopted_run(&mut state, &repo, dir.path(), "adopted-branch");
    let refused = state.handle(req(
        "run.stage_send_notes",
        json!({ "run_id": adopted_id, "stage_id": "second-half" }),
    ));
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("implements no plan"),
        "{refused:?}"
    );
}

#[test]
fn set_auto_advance_runs_every_stage_to_review() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "run all" })));
    let plan_id = plan_id_of(&plan);
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "second-half" }),
    ));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate");

    let armed = state.handle(req(
        "run.set_auto_advance",
        json!({ "run_id": run_id, "enabled": true }),
    ));
    assert_eq!(armed["result"]["state"], "review", "{armed:?}");
    assert_eq!(armed["result"]["auto_advance"], true);
    assert!(armed["result"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["state"] == "validated_passed"));
    let first = &armed["result"]["stages"][0];
    assert!(first["start_sha"].is_string(), "{first:?}");
    assert!(first["built_sha"].is_string(), "{first:?}");
    assert_eq!(first["completion_sha"], first["built_sha"], "{first:?}");
    assert_eq!(first["publication"], "local", "{first:?}");

    let stable = state.handle(req(
        "run.stage_diff",
        json!({ "run_id": run_id, "stage_id": "first-half" }),
    ));
    assert_eq!(stable["ok"], true, "{stable:?}");
    assert_eq!(stable["result"]["status"], "available");
    assert_eq!(stable["result"]["start_sha"], first["start_sha"]);
    assert_eq!(stable["result"]["completion_sha"], first["completion_sha"]);
    assert!(stable["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("result-first-half.txt"));
    assert!(!stable["result"]["patch"]
        .as_str()
        .unwrap()
        .contains("result-second-half.txt"));
}

/// Requesting changes talks to the worktree's agent instead of killing it
/// and spawning a replacement: the comments land on the durable thread, a
/// turn is queued for the worktree's one agent, and the run's phase-session
/// slot is never touched.
#[test]
fn run_request_changes_delivers_to_the_agent_instead_of_respawning() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "do work");
    let worktree_root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    let rc = state.handle(req(
        "run.request_changes",
        json!({ "run_id": run_id, "comments": "rename the symbol" }),
    ));
    assert_eq!(rc["result"]["state"], "review", "{rc:?}");
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "a verb queues a turn; only delivery — off the state lock — spawns"
    );
    let queued = state
        .pending_agent_turns
        .last()
        .expect("a change request is a turn for the worktree's agent");
    assert_eq!(queued.owner, run_id);
    assert_eq!(
        queued.root, worktree_root,
        "the turn is addressed to the worktree, not to the run"
    );
    assert_eq!(
        queued.said().warm,
        NEW_THREAD_MESSAGES_PROMPT,
        "an agent already in the conversation is only told to read the thread"
    );
    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains(NEW_THREAD_MESSAGES_PROMPT)
            && delivered.contains("rename the symbol")
            && delivered.contains("Ordered Issue stage-plan catalog")
            && delivered.find("\n- first-half").unwrap()
                < delivered.find("\n- second-half").unwrap(),
        "a cold agent gets the run context, ordered stage catalog, AND the reviewer's              words: {delivered}"
    );
    let structured = state.handle(req(
        "run.request_changes",
        json!({
            "run_id": run_id,
            "messages": [{
                "body": "Use the public name",
                "anchor": {
                    "artifact": "diff",
                    "path": "src/lib.rs",
                    "side": "new",
                    "line_start": 12,
                    "line_end": 12,
                    "heading_path": [],
                    "snippet": "fn old_name()"
                }
            }]
        }),
    ));
    assert_eq!(structured["ok"], true, "{structured:?}");
    let messages = structured["result"]["thread"]["items"].as_array().unwrap();
    assert!(messages.iter().any(|item| {
        item["type"] == "message"
            && item["data"]["body"] == "Use the public name"
            && item["data"]["anchor"]["path"] == "src/lib.rs"
    }));
    let bad = state.handle(req("run.request_changes", json!({ "run_id": run_id })));
    assert!(
        bad["error"].as_str().unwrap().contains("comments"),
        "{bad:?}"
    );
}

/// A freeform message to a working run's agent talks to the process the
/// reviewer is already in conversation with. The words land on the durable
/// thread, the turn is addressed to the WORKTREE, and the run's phase
/// session is never ended or replaced.
#[test]
fn run_message_delivers_to_the_agent_instead_of_respawning() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-message",
        RunState::Building,
    );

    let sent = state.handle(req(
        "run.message",
        json!({ "run_id": "run-message", "message": "prefer the smaller helper" }),
    ));
    assert_eq!(sent["ok"], true, "{sent:?}");
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "messaging the agent must not spawn a harness under the state lock"
    );

    let queued = state
        .pending_agent_turns
        .last()
        .expect("a message is a turn for the worktree's agent");
    assert_eq!(queued.owner, "run-message");
    assert_eq!(
        queued.root, root,
        "the turn is addressed to the worktree, not to the run"
    );
    assert_eq!(
        queued.said().warm,
        NEW_THREAD_MESSAGES_PROMPT,
        "an agent already in the conversation is only told to read the thread"
    );
    assert!(
        queued.said().cold.contains(NEW_THREAD_MESSAGES_PROMPT)
            && queued.said().cold.contains("Build conversation protocol"),
        "a cold agent gets the run context and the conversation protocol: {}",
        queued.said().cold
    );
    let posted = primary_thread(&state.runs["run-message"].agents)
        .items
        .iter()
        .any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
            if m.body == "prefer the smaller helper")
        });
    assert!(posted, "the reviewer's words stay durable on the thread");
}

/// Dispatching a stage is a turn, not a new process. The stage prompt is
/// queued for the worktree's one agent; a warm agent hears the stage
/// instruction alone (it lived the conversation), a cold one hears the same
/// instruction wrapped in the conversation protocol and catch-up packet.
#[test]
fn dispatching_a_stage_queues_its_prompt_for_the_worktrees_one_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "two stages" })));
    let plan_id = plan_id_of(&plan);
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    state.pending_agent_turns.clear();

    let dispatched = state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "dispatching a stage must not spawn a harness under the state lock"
    );

    let queued = state
        .pending_agent_turns
        .first()
        .expect("a stage dispatch is a turn for the worktree's agent");
    assert_eq!(queued.owner, run_id);
    assert_eq!(queued.root, root);
    assert!(
        queued.said().warm.contains("Second half"),
        "the stage instruction travels whether the agent is warm or cold: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "a warm agent is not re-taught the protocol it is already following: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.starts_with(&queued.said().warm)
            && queued.said().cold.contains("Build conversation protocol"),
        "a cold agent gets the same instruction plus the conversation it missed: {}",
        queued.said().cold
    );
}

/// A multi-stage run parked at its stage gate after a REAL first-stage build
/// and validation verdict — the shape `run.stage_fix` and run-all act on,
/// reached without the scripted agent playing both sides of the stage.
/// Returns `(run_id, worktree root)`.
fn run_at_the_stage_gate_after_a_real_first_stage(
    state: &mut AppState,
    goal: &str,
    first_stage_passed: bool,
) -> (String, std::path::PathBuf) {
    let (run_id, root) = run_awaiting_a_real_stage_build(state, goal);
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "the first stage is built".into(),
            outputs: DoneOutputs::default(),
        },
    );
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Validate,
            status: DoneStatus::Completed,
            summary: "the first stage is validated".into(),
            outputs: DoneOutputs {
                validation: Some(ValidationReport {
                    passed: first_stage_passed,
                    findings: if first_stage_passed {
                        String::new()
                    } else {
                        "- the migration is missing".into()
                    },
                    notes_for_next_stage: String::new(),
                }),
                ..DoneOutputs::default()
            },
        },
    );
    assert_eq!(
        state.runs[&run_id].run.state,
        RunState::StageGate,
        "a verdict on a non-final stage parks the run at the gate"
    );
    (run_id, root)
}

/// Fixing a failed stage is a turn, not a new process. `run.stage_fix` moves
/// the stage back to `Building` — and unless the fix prompt is QUEUED for the
/// worktree's one agent, the run sits in `Building` forever with nobody ever
/// told to fix anything. Moving the state is not dispatching the work.
#[test]
fn fixing_a_failed_stage_queues_its_fix_prompt_for_the_worktrees_one_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, root) =
        run_at_the_stage_gate_after_a_real_first_stage(&mut state, "fix the stage", false);
    assert_eq!(
        state.runs[&run_id].stages[0].state,
        StageProgressState::Validated { passed: false },
        "the fix verb only applies to a stage whose validation failed"
    );
    state.pending_agent_turns.clear();

    let fixed = state.handle(req(
        "run.stage_fix",
        json!({
            "run_id": run_id,
            "stage_id": "first-half",
            "note": "add the migration",
        }),
    ));
    assert_eq!(fixed["ok"], true, "{fixed:?}");
    assert_eq!(
        state.runs[&run_id].stages[0].state,
        StageProgressState::Building,
        "the stage went back to work"
    );
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "a verb queues a turn; only delivery — off the state lock — spawns"
    );

    let queued = state
        .pending_agent_turns
        .last()
        .expect("a stage fix is a turn for the worktree's agent");
    assert_eq!(queued.owner, run_id);
    assert_eq!(
        queued.root, root,
        "the stage is fixed in the worktree it was built in"
    );
    assert_eq!(queued.phase, "build");
    assert!(
        queued.said().warm.contains("add the migration")
            && queued.said().warm.contains("the migration is missing"),
        "the reviewer's note and the failed findings both travel: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "the agent that just failed validation is not re-taught the protocol: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.starts_with(&queued.said().warm)
            && queued.said().cold.contains("Build conversation protocol"),
        "a replacement agent gets the same fix plus the conversation it missed: {}",
        queued.said().cold
    );
}

/// Sending a stage's open comments mid-run is a turn for the RUN's worktree
/// agent (the plan doc is revised where the run can see it). The comments
/// are already posts on the Issue's conversation — the one that agent reads
/// its messages from — but unless the revision turn is queued, nothing ever
/// asks the agent to revise the doc and the run stalls at the gate with
/// `revising_stage_id` set and no agent working.
#[test]
fn sending_stage_notes_mid_run_queues_a_revision_turn_for_the_worktrees_one_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "revise mid-run" })));
    let plan_id = plan_id_of(&plan);
    for stage_id in ["first-half", "second-half"] {
        let approved = state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": stage_id }),
        ));
        assert_eq!(approved["ok"], true, "{approved:?}");
    }
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
    let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
    let comment = state.handle(req(
        "plan.comment_add",
        json!({ "plan_id": plan_id, "stage_id": "second-half", "body": "tighten this" }),
    ));
    assert_eq!(comment["ok"], true, "{comment:?}");
    // The scripted agent answers the revision itself, which would swallow
    // the very dispatch under test.
    state.qa_agent = false;
    state.pending_agent_turns.clear();

    let sent = state.handle(req(
        "run.stage_send_notes",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert_eq!(sent["ok"], true, "{sent:?}");
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "a verb queues a turn; only delivery — off the state lock — spawns"
    );

    let queued = state
        .pending_agent_turns
        .last()
        .expect("stage notes are a turn for the run worktree's agent");
    assert_eq!(queued.owner, run_id);
    assert_eq!(
        queued.root, root,
        "the doc is revised in the run's worktree, not the plan's"
    );
    assert_eq!(queued.phase, "revise");
    assert!(
        queued.said().warm.contains("read_unread_messages"),
        "the comments travel through MCP; the turn only points at them: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "an agent already in the run is not re-taught the protocol: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.contains("02-second-half.md")
            && queued.said().cold.contains("Build conversation protocol")
            && queued
                .said()
                .cold
                .contains("Ordered Issue stage-plan catalog")
            && queued.said().cold.find("\n- first-half").unwrap()
                < queued.said().cold.find("\n- second-half").unwrap(),
        "a cold agent is primed with the ordered catalog and stage doc it must revise: {}",
        queued.said().cold
    );
    let comments = primary_thread(&state.plans[&plan_id].agents).doc_comments();
    assert_eq!(
        comments
            .iter()
            .filter(|comment| comment.body == "tighten this")
            .count(),
        1,
        "the comment is one post on the conversation this agent reads: {comments:?}"
    );
}

/// Run-all is the one dispatcher with no human behind each hop: arming it at
/// a stage gate must QUEUE the next stage's turn, not merely walk the run's
/// state forward. Drop the queueing and every stage advances while no agent
/// is ever asked to build one — the failure is silent by construction.
#[test]
fn auto_advance_queues_the_next_stages_turn_for_the_worktrees_one_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, root) =
        run_at_the_stage_gate_after_a_real_first_stage(&mut state, "run them all", true);
    assert_eq!(
        state.runs[&run_id].stages[0].state,
        StageProgressState::Validated { passed: true },
        "the first stage passed, so the next one is dispatchable"
    );
    state.pending_agent_turns.clear();

    let armed = state.handle(req(
        "run.set_auto_advance",
        json!({ "run_id": run_id, "enabled": true }),
    ));
    assert_eq!(armed["ok"], true, "{armed:?}");
    assert_eq!(
        state.runs[&run_id].run.state,
        RunState::Building,
        "run-all dispatched the next stage"
    );
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "auto-advance queues a turn; only delivery — off the state lock — spawns"
    );

    let queued = state
        .pending_agent_turns
        .last()
        .expect("run-all dispatches the next stage as a turn for the worktree's agent");
    assert_eq!(queued.owner, run_id);
    assert_eq!(
        queued.root, root,
        "the next stage is built in the run's one worktree"
    );
    assert_eq!(queued.phase, "build");
    assert!(
        queued.said().warm.contains("Second half"),
        "the next stage's instruction travels warm or cold: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "the agent that built stage one is not re-taught the protocol: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.starts_with(&queued.said().warm)
            && queued.said().cold.contains("Build conversation protocol"),
        "a replacement agent gets the same instruction plus the conversation: {}",
        queued.said().cold
    );
}

/// Run-all end to end: no human types anything between stages, so the whole
/// path — queue under the state lock, drain off it — has to carry the next
/// stage's prompt into the SAME agent process. A break anywhere along it is
/// invisible from the run's state, which advances either way.
#[tokio::test]
async fn run_all_delivers_the_next_stages_prompt_to_the_same_agent_process() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    let (run_id, root) = {
        let mut s = state.lock().unwrap();
        run_at_the_stage_gate_after_a_real_first_stage(&mut s, "run them all", true)
    };
    // The turns the fixture queued were queued on the state directly; the
    // handler is the thing that delivers, and delivering opens the agent.
    let opened = call(&handler, "run.get", json!({ "run_id": run_id }));
    assert_eq!(opened["ok"], true, "{opened:?}");
    wait_for_deliveries(&state).await;
    let key = derived_agent_key(&root, &run_id);
    let first_stage_pid = {
        let s = state.lock().unwrap();
        agent_pid(
            s.session_registry
                .test_tab(&key)
                .expect("the first stage's turns opened the worktree's agent"),
        )
        .expect("a live harness has a pid")
    };

    let armed = call(
        &handler,
        "run.set_auto_advance",
        json!({ "run_id": run_id, "enabled": true }),
    );
    assert_eq!(armed["ok"], true, "{armed:?}");

    let screen = wait_for_agent_screen(&state, &root, "Second half").await;
    assert!(
        screen.contains("Second half"),
        "run-all's next stage must reach the agent's PTY: {screen:?}"
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.runs[&run_id].run.state,
        RunState::Building,
        "the run is building the stage run-all dispatched"
    );
    assert_eq!(
        s.session_registry.test_tab(&key).and_then(agent_pid),
        Some(first_stage_pid),
        "the agent that built stage one is the one asked to build stage two"
    );
    assert_eq!(
        s.session_registry.test_counts().tabs,
        1,
        "one worktree, one agent"
    );
}

/// Every phase of a multi-stage run — the first stage's build, the
/// validation hand-off its `done` triggers, and the next stage's build —
/// reaches ONE process in the run's worktree. The phase boundary stopped
/// being a process boundary: that is the whole point of the tab.
#[tokio::test]
async fn a_multi_stage_run_drives_one_agent_process_through_every_phase() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    let plan = call(&handler, "plan.create", json!({ "goal": "one agent" }));
    let plan_id = plan_id_of(&plan);
    for stage_id in ["first-half", "second-half"] {
        call(
            &handler,
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": stage_id }),
        );
    }
    call(&handler, "plan.approve", json!({ "plan_id": plan_id }));
    let run = call(&handler, "run.create", json!({ "plan_id": plan_id }));
    assert_eq!(run["ok"], true, "{run:?}");
    let run_id = run_id_of(&run);
    let key = {
        let s = state.lock().unwrap();
        derived_agent_key(
            &AppState::canonical_root(&s.runs[&run_id].worktree.path),
            &run_id,
        )
    };
    wait_for_deliveries(&state).await;
    let first_pid = {
        let s = state.lock().unwrap();
        let tab = s
            .session_registry
            .test_tab(&key)
            .expect("dispatching a run opens the worktree's agent");
        assert!(tab.session_is_live(), "the agent is running");
        agent_pid(tab).expect("a live harness has a pid")
    };

    let next = call(
        &handler,
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    );
    assert_eq!(next["ok"], true, "{next:?}");
    wait_for_deliveries(&state).await;
    let s = state.lock().unwrap();
    assert_eq!(
        s.session_registry.test_tab(&key).and_then(agent_pid),
        Some(first_pid),
        "every phase must reach the process the dispatch woke"
    );
    assert_eq!(
        s.session_registry
            .test_tabs()
            .map(|(key, _)| key)
            .filter(|k| k.is_agent() && k.root == key.root)
            .count(),
        1,
        "every phase of one run reaches one agent — a second agent on the \
         branch is the human's to add, never a phase's"
    );
    assert_eq!(
        s.session_registry.test_counts().tabs,
        1,
        "no harness may be spawned beside the tab's agent {:?}",
        s.session_registry
            .test_tabs()
            .map(|(k, t)| (k.clone(), t.role.clone()))
            .collect::<Vec<_>>()
    );
}

/// The agent Build talks to is one process for the worktree's life. A
/// second round of comments reaches the SAME harness — same pid, one tab —
/// because a warm tab is delivered to, never replaced.
#[tokio::test]
async fn a_second_request_changes_reaches_the_same_agent_process() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    // The fixture only needs state, so it goes through the synchronous
    // path; the change requests below go through the frame handler, which
    // is what actually delivers a queued turn.
    let (_, run_id) = planned_run_in_review(&mut state.lock().unwrap(), "keep the agent");
    let root = {
        let s = state.lock().unwrap();
        AppState::canonical_root(&s.runs[&run_id].worktree.path)
    };
    let key = derived_agent_key(&root, &run_id);

    let first = call(
        &handler,
        "run.request_changes",
        json!({
            "run_id": run_id, "comments": "rename the symbol"
        }),
    );
    assert_eq!(first["ok"], true, "{first:?}");
    wait_for_deliveries(&state).await;
    let first_pid = {
        let s = state.lock().unwrap();
        let tab = s
            .session_registry
            .test_tab(&key)
            .expect("a change request opens the worktree's agent");
        assert!(tab.session_is_live(), "the agent is running");
        agent_pid(tab).expect("a live harness has a pid")
    };

    let second = call(
        &handler,
        "run.request_changes",
        json!({
            "run_id": run_id, "comments": "and inline the helper"
        }),
    );
    assert_eq!(second["ok"], true, "{second:?}");
    wait_for_deliveries(&state).await;
    let s = state.lock().unwrap();
    assert_eq!(
        s.session_registry.test_counts().tabs,
        1,
        "one worktree, one agent"
    );
    assert_eq!(
        s.session_registry.test_tab(&key).and_then(agent_pid),
        Some(first_pid),
        "the second request must reach the process the first one woke"
    );
}

/// A persistent agent outlives the phase it was dispatched for: talk to it
/// at a review gate and it reports `done` from a state the run machine does
/// not accept. Enforcement is by observation, not permission — the report
/// is recorded on the conversation and moves nothing, rather than landing
/// as a failure the human never caused.
#[test]
fn an_out_of_phase_done_is_recorded_and_moves_nothing() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "already reviewed");

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "Tidied the imports you mentioned".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let got = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(
        got["result"]["state"], "review",
        "an out-of-phase report moves nothing: {got:?}"
    );
    let items = got["result"]["thread"]["items"].as_array().unwrap();
    assert!(
        items.iter().any(|item| {
            item["type"] == "message"
                && item["data"]["outcome"] == "completed"
                && item["data"]["body"] == "Tidied the imports you mentioned"
        }),
        "the report is recorded: {items:?}"
    );
    assert!(
        !items.iter().any(
            |item| item["data"]["event"] == "run_failed" || item["data"]["outcome"] == "failed"
        ),
        "a report Build cannot apply is not a failure: {items:?}"
    );
}

/// The report the branch's state does not accept still needs the user: it
/// is an agent saying it finished. So it lands as an attention event and
/// the entry says why, while the branch stays exactly where it was.
#[test]
fn a_dispatched_agents_report_at_a_review_gate_asks_for_the_user() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "already reviewed");
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "Tidied the imports you mentioned".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let got = state.handle(req("run.get", json!({ "run_id": run_id })));
    assert_eq!(got["result"]["state"], "review", "{got:?}");
    assert_eq!(
        got["result"]["unread"], true,
        "an agent reporting it finished is something the user is told: {got:?}"
    );
    assert_eq!(got["result"]["unread_reason"], "done", "{got:?}");
}

/// And the daemon says so in its own voice. Branch state belongs to the
/// branch, so a report that moves nothing is the design working — the line
/// it writes must not read like a transition somebody has to go and fix.
#[test]
fn the_out_of_phase_line_says_the_branch_is_unchanged_not_that_something_failed() {
    let line = out_of_phase_log(
        "run-1",
        &crate::run::IllegalRunTransition {
            from: crate::run::RunState::Review,
            event: crate::run::RunEvent::BuildReady,
        },
    );

    assert!(
        line.contains("dispatched-agent report recorded; branch state unchanged"),
        "{line}"
    );
    assert!(line.contains("run-1") && line.contains("Review"), "{line}");
    assert!(
        !line.contains("illegal") && !line.contains("not valid"),
        "the words of a rejected transition have no business here: {line}"
    );
}

/// A multi-stage run parked mid-build, waiting on a real `done` — the shape
/// production has and the scripted agent never reaches, because it plays
/// both sides of a stage itself. Returns `(state, run_id, worktree root)`.
pub(in crate::app::tests) fn run_awaiting_a_real_stage_build(
    state: &mut AppState,
    goal: &str,
) -> (String, std::path::PathBuf) {
    let plan = state.handle(req("plan.create", json!({ "goal": goal })));
    let plan_id = plan_id_of(&plan);
    for stage_id in ["first-half", "second-half"] {
        let approved = state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": stage_id }),
        ));
        assert_eq!(approved["ok"], true, "{approved:?}");
    }
    let approved = state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    assert_eq!(approved["ok"], true, "{approved:?}");
    // From here the scripted agent must stop answering for the harness:
    // `qa_simulate_stage_build` consumes the build AND the validation in one
    // call, so it would swallow the very hand-off under test.
    state.qa_agent = false;
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    assert_eq!(run["ok"], true, "{run:?}");
    let run_id = run_id_of(&run);
    let active = &state.runs[&run_id];
    assert_eq!(
        active.stages[0].state,
        StageProgressState::Building,
        "the run is waiting on its stage-build agent"
    );
    let root = AppState::canonical_root(&active.worktree.path);
    (run_id, root)
}

/// A stage that reports its build complete hands ITSELF to validation. The
/// orchestrator returns that hand-off as a turn and `on_run_agent_done` is
/// the only thing that queues it — a `done` is the one input to the daemon
/// that starts a phase without a human verb behind it. Drop the queueing and
/// a built stage is simply never asked to validate itself.
#[test]
fn a_built_stage_queues_its_validation_turn_for_the_worktrees_agent() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (run_id, root) = run_awaiting_a_real_stage_build(&mut state, "hand off to validation");
    state.pending_agent_turns.clear();

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "stage one is built".into(),
            outputs: DoneOutputs::default(),
        },
    );

    assert_eq!(
        state.runs[&run_id].stages[0].state,
        StageProgressState::Validating,
        "the report moved the stage to its validation gate"
    );
    let queued = state
        .pending_agent_turns
        .last()
        .expect("a built stage hands itself to validation as a turn");
    assert_eq!(queued.phase, "validate");
    assert_eq!(queued.owner, run_id);
    assert_eq!(
        queued.root, root,
        "the stage is validated in the worktree it was built in"
    );
    assert!(
        queued.said().warm.contains("VALIDATION agent"),
        "the validation instruction travels warm or cold: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "the agent that just reported is not re-taught the protocol: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.starts_with(&queued.said().warm)
            && queued.said().cold.contains("Build conversation protocol"),
        "a replacement agent gets the same instruction plus the conversation: {}",
        queued.said().cold
    );
}
