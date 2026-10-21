//! Real router tool parsing, authenticated dispatch and durable effects.
use super::{router_fixture::RouterFixture, VerifiedStep};
use crate::app::tests::*;
use crate::mcp::McpSurface;
use std::collections::BTreeSet;
use std::time::Duration;

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let mut context = RouterFixture::new("mcp-router-actions");
    let question = context
        .capture("ask which isolated project should receive this work")
        .await;
    let progressing = context
        .fixture
        .call_mcp_tool(
            &question.agent_id,
            &question.token,
            McpSurface::Router,
            "post_thread_message",
            json!({"status":"Working","body":"Reading isolated branches"}),
        )
        .await;
    assert_eq!(progressing.reply["result"]["working"], true);
    let saved = context.capture_record(&question.capture_id);
    assert_eq!(saved["progress"], "Reading isolated branches");
    assert_eq!(saved["state"], "routing");
    assert!(saved["question"].is_null());
    context
        .fixture
        .verify(
            progressing,
            json!({"capture_id":question.capture_id,
        "saved_progress":"Reading isolated branches","semantic_variant":"router_message"}),
        )
        .await;

    let asked = context
        .fixture
        .call_mcp_tool(
            &question.agent_id,
            &question.token,
            McpSurface::Router,
            "ask_user",
            json!({"question":"Which project?","options":[{"label":"Bridge project",
            "project_id":context.project,"kind":"branch","branch":"build/router-option"}]}),
        )
        .await;
    let saved = context.capture_record(&question.capture_id);
    assert_eq!(saved["state"], "unrouted");
    assert_eq!(saved["question"]["text"], "Which project?");
    assert!(saved["question"]["answer"].is_null());
    assert_eq!(saved["question"]["options"][0]["id"], "option-1");
    assert_eq!(
        saved["question"]["options"][0]["project_id"],
        context.project
    );
    assert_eq!(saved["question"]["options"][0]["kind"], "branch");
    assert_eq!(
        saved["question"]["options"][0]["branch"],
        "build/router-option"
    );
    assert_eq!(
        asked.reply["result"]["options"],
        saved["question"]["options"]
    );
    context
        .fixture
        .verify(
            asked,
            json!({"capture_id":question.capture_id,
        "persisted_question":saved["question"],"awaiting_answer":true}),
        )
        .await;

    let routing = context
        .capture("dispatch this isolated branch instruction")
        .await;
    let branch = "build/rare-mcp-router-dispatch";
    let instruction = "Implement the isolated router qualification endpoint";
    assert!(!local_branch_exists(&context.repository, branch).unwrap());
    let dispatched = context
        .fixture
        .call_mcp_tool(
            &routing.agent_id,
            &routing.token,
            McpSurface::Router,
            "dispatch_branch",
            json!({"project_id":context.project,"branch":branch,"name":"Router Worker",
            "instruction":instruction,"rationale":"The isolated bridge owns this endpoint"}),
        )
        .await;
    let owner = dispatched.reply["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent = dispatched.reply["result"]["agent_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let delivered = wait_for(Duration::from_secs(2), || {
        let app = context.fixture.state().lock().unwrap();
        let run = app.runs.get(&owner)?;
        let tab = app.session_registry.test_tab(&TabKey::agent(
            &AppState::canonical_root(&run.worktree.path),
            &agent,
        ))?;
        (app.delivery_queue.is_idle()
            && tab.session_instance.is_some()
            && context
                .provider
                .log
                .turns()
                .iter()
                .any(|turn| turn.contains(instruction)))
        .then_some(())
    })
    .await;
    if delivered.is_none() {
        context.quiesce().await;
        panic!("router dispatch acknowledged without delivering its branch instruction");
    }
    let saved = context.capture_record(&routing.capture_id);
    assert_eq!(saved["state"], "routed");
    assert_eq!(saved["routing"]["project_id"], context.project);
    assert_eq!(saved["routing"]["kind"], "branch");
    assert_eq!(saved["routing"]["target_id"], branch);
    assert_eq!(
        saved["routing"]["rationale"],
        "The isolated bridge owns this endpoint"
    );
    assert!(local_branch_exists(&context.repository, branch).unwrap());
    let (checkout, durable, name) = {
        let app = context.fixture.state().lock().unwrap();
        let run = &app.runs[&owner];
        (
            run.worktree.path.clone(),
            app.store
                .as_ref()
                .unwrap()
                .load_all_runs()
                .unwrap()
                .iter()
                .any(|row| row.id == owner && row.branch == branch),
            run.agents.by_id(&agent).unwrap().name.clone(),
        )
    };
    assert!(checkout.is_dir());
    assert!(durable);
    assert_eq!(name.as_deref(), Some("Router Worker"));
    context
        .fixture
        .verify(
            dispatched,
            json!({"capture_id":routing.capture_id,
        "durable_run":owner,"created_agent":agent,"branch":branch,"checkout":checkout,
        "instruction_delivered":instruction,"saved_route":saved["routing"]}),
        )
        .await;
    context.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_router_actions_have_three_actual_effect_proofs() {
    let steps = execute().await;
    let observed: BTreeSet<_> = steps
        .iter()
        .map(|step| {
            step.mcp_action.as_ref().unwrap()["action"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(
        observed,
        BTreeSet::from(["router_message", "ask_user", "dispatch_branch"])
    );
}
