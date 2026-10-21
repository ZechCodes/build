//! OFFSOURCE exact shared recipe form audit. Not a generic production read
//! classifier and not proof of successful native replies. The driver still
//! requires issued auth, semantic oracle, final value and outcome classification.
use super::{mcp_cases, FullReadForm, McpReadInputs};
use crate::mcp::BridgeAction;

/// Test-first adapter for the old harness's implicit all-ready assumption.
/// Not used by production admission or by the normal qualification backend.
pub(in crate::app::tests::runtime::write_wait_qualification) fn mcp_recipe_form(
    action: &BridgeAction,
) -> Option<FullReadForm> {
    match action {
        BridgeAction::TrackerGetTask { .. }
        | BridgeAction::TrackerListTasks { .. }
        | BridgeAction::TrackerTaskDependents { .. }
        | BridgeAction::ListHarnesses
        | BridgeAction::ListWorkspaceAgents { .. }
        | BridgeAction::ListProjectAgents
        | BridgeAction::ListWorkspaces
        | BridgeAction::ListProjects
        | BridgeAction::ListWork
        | BridgeAction::SearchConversation { .. }
        | BridgeAction::ReadConversation { .. }
        | BridgeAction::TrackerReadComment { .. }
        | BridgeAction::TrackerGetReview { .. }
        | BridgeAction::TrackerReadReview { .. } => Some(FullReadForm::Ready),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::tests::runtime::write_wait_qualification) struct LoadedReadPolicy {
    pub p99_limit_us: u64,
    pub minimum_successes: usize,
}
pub(in crate::app::tests::runtime::write_wait_qualification) fn mcp_recipe_latency_policy(
    action: &BridgeAction,
) -> Option<LoadedReadPolicy> {
    // Preserve the existing form-coupled loaded gate until its intended RED.
    (mcp_recipe_form(action) == Some(FullReadForm::Ready)).then_some(LoadedReadPolicy {
        p99_limit_us: 100_000,
        minimum_successes: 1_000,
    })
}

#[test]
fn shared_mcp_forms_keep_all_fourteen_loaded_latency_contracts() {
    use super::super::trial_complete as trial;
    let ids = McpReadInputs {
        owner: "owner".into(),
        agent: "agent".into(),
        workspace: "workspace".into(),
        task: "task".into(),
        comment: "comment".into(),
        snapshot: "snapshot".into(),
        directory: "directory".into(),
        path: "README.md".into(),
        search_text: "saved marker".into(),
    };
    let actions = mcp_cases(&ids);
    let actual = actions
        .iter()
        .map(|(_, action)| {
            (
                action.tool_name(),
                mcp_recipe_form(action),
                mcp_recipe_latency_policy(action),
            )
        })
        .collect::<Vec<_>>();
    use FullReadForm::{LegacyFull, Ready};
    let policy = Some(LoadedReadPolicy {
        p99_limit_us: 100_000,
        minimum_successes: 1_000,
    });
    let expected = [
        ("get_task", Ready),
        ("list_tasks", LegacyFull),
        ("list_task_dependents", Ready),
        ("list_harnesses", Ready),
        ("list_workspace_agents", Ready),
        ("list_project_agents", Ready),
        ("list_workspaces", Ready),
        ("list_projects", Ready),
        ("list_work", Ready),
        ("search_conversation", Ready),
        ("read_conversation", Ready),
        ("read_comment", Ready),
        ("get_review", Ready),
        ("read_review", Ready),
    ]
    .map(|(method, form)| (method, Some(form), policy));
    // No readiness inference from cardinality: use actual typed recipe names.
    let catalog = trial::ReadCatalog {
        rpc: super::rpc::registered_methods()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        mcp: actions
            .iter()
            .map(|(_, a)| a.tool_name().to_owned())
            .collect(),
        router_only_mcp: actions
            .iter()
            .filter(|(caller, _)| *caller == super::McpCaller::Router)
            .map(|(_, a)| a.tool_name().to_owned())
            .collect(),
    };
    let plan = trial::make_plan(
        trial::TrialName {
            envelope: trial::Envelope::Small,
            number: 1,
        },
        &catalog,
    )
    .unwrap();
    let reads = plan
        .iter()
        .filter(|s| s.traffic == trial::Traffic::Read)
        .collect::<Vec<_>>();
    assert_eq!(
        reads.len(),
        59_000,
        "the full completion denominator never shrinks for legacy MCP"
    );
    for method in &catalog.mcp {
        assert_eq!(
            reads
                .iter()
                .filter(|s| s.route == trial::Route::AgentMcp && s.method.as_ref() == Some(method))
                .count(),
            1_000,
            "{method}"
        );
    }
    assert_eq!(
        super::rpc::classify(
            "task.doc",
            &serde_json::json!({"ok":true,"result":{"contents":"whole legacy doc"}})
        ),
        super::ReadOutcome::LegacyFull
    );
    assert_eq!(
        actual, expected,
        "form classification is separate from mandatory100ms/1000-success MCP latency policy"
    );
}
