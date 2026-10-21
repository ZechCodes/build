//! The three coding completion statuses consume real Building run records.
use super::super::finite_provider::FiniteProvider;
use super::*;
use crate::app::tests::*;
use crate::run::RunState;

const TOKEN: &str = "qualification-run-report-capability";

async fn report(status: DoneStatus, expected: RunState) -> VerifiedReport {
    let (directory, repository) = init_repo();
    let provider = FiniteProvider::install(directory.path());
    provider.require_enrolled();
    let mut app = qa_state(&repository, directory.path());
    let owner = adopted_run(&mut app, &repository, directory.path(), "reporting-run");
    let caller = primary_agent_id(&app, &owner);
    let mut active = app.runs.remove(&owner).unwrap();
    active.run.state = RunState::Building;
    active.auto_advance = false;
    app.finish_run_mutation(owner.clone(), active).unwrap();
    app.record_agent_working_since(&owner, &caller, Some(now_rfc3339()));
    app.test_install_mcp_token(caller.clone(), TOKEN.into());
    // This report has no held tasks or queued turns. The enrolled fixture
    // remains a safe boundary if unexpected delivery is queued, and its
    // observed count proves that this path did not require a provider start.
    assert!(app.delivery_queue.is_idle());
    let fixture = RareFixture::new("mcp-coding-report", app);
    let summary = format!("Qualification coding report {status:?}");
    let proof = fixture
        .report_mcp(&caller, TOKEN, DoneReport::new(status, &summary))
        .await;
    let (saved, thread) = {
        let app = fixture.state().lock().unwrap();
        let saved = app
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.id == owner)
            .unwrap();
        let conversation = saved
            .agents
            .iter()
            .find(|agent| agent.id == caller)
            .unwrap();
        assert!(conversation.working_since.is_none());
        let thread = app
            .store
            .as_ref()
            .unwrap()
            .thread_items(conversation.conversation_id())
            .unwrap();
        (saved, thread)
    };
    assert_eq!(saved.state, expected);
    assert_eq!(saved.last_summary.as_deref(), Some(summary.as_str()));
    assert!(saved.last_error.is_none());
    assert!(serde_json::to_string(&thread).unwrap().contains(&summary));
    assert!(fixture.state().lock().unwrap().delivery_queue.is_idle());
    assert_eq!(provider.opens.load(Ordering::SeqCst), 0);
    let verified = fixture
        .verify_report(
            proof,
            json!({
                "run_id":owner,"caller":caller,"persisted_state":saved.state,
                "persisted_summary":saved.last_summary,"persisted_thread":thread,
            }),
        )
        .await;
    assert!(fixture.finish().await.is_empty());
    verified
}

pub(super) async fn execute() -> Vec<VerifiedReport> {
    let mut reports = Vec::new();
    for (status, state) in [
        (DoneStatus::Completed, RunState::Review),
        (DoneStatus::Blocked, RunState::Blocked),
        (DoneStatus::Failed, RunState::Failed),
    ] {
        reports.push(report(status, state).await);
    }
    reports
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_coding_done_reports_persist_all_three_terminal_outcomes_without_ack() {
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(execute().await.len(), 3);
}
