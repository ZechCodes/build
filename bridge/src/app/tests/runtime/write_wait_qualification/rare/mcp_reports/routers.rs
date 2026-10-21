//! Real router reports retire their actual capture session without replying.
use super::super::router_fixture::RouterFixture;
use super::*;
use crate::app::tests::*;

async fn report(status: DoneStatus) -> VerifiedReport {
    let mut context = RouterFixture::new("mcp-router-report");
    let identity = context
        .capture("qualification router report remains undecided")
        .await;
    let proof = context
        .fixture
        .report_mcp(
            &identity.agent_id,
            &identity.token,
            DoneReport::new(status, format!("Qualification router report {status:?}")),
        )
        .await;
    let saved = context.capture_record(&identity.capture_id);
    let retired = {
        let app = context.fixture.state().lock().unwrap();
        !app.router_sessions.contains_key(&identity.capture_id)
            && app
                .session_registry
                .test_token(&identity.agent_id)
                .is_none()
            && !app
                .session_registry
                .contains(&TabKey::agent(&identity.scratch, &identity.agent_id))
    };
    // Cleanup joins the real retirement/reaper even when an effect assertion
    // fails. The captured retirement state precedes this cleanup.
    context.quiesce().await;
    assert!(
        retired,
        "the actual report must retire its authenticated router"
    );
    assert_eq!(
        saved["state"], "failed",
        "undecided routes remain visible as failed"
    );
    assert!(!identity.scratch.exists());
    let verified = context
        .fixture
        .verify_report(
            proof,
            json!({
                "capture_id":identity.capture_id,"router_agent":identity.agent_id,
                "persisted_capture":saved,"token_revoked":retired,
                "retired_scratch":identity.scratch,
            }),
        )
        .await;
    assert!(context.finish().await.is_empty());
    verified
}

pub(super) async fn execute() -> Vec<VerifiedReport> {
    let mut reports = Vec::new();
    for status in [
        DoneStatus::Completed,
        DoneStatus::Blocked,
        DoneStatus::Failed,
    ] {
        reports.push(report(status).await);
    }
    reports
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_router_done_reports_retire_all_three_statuses_without_ack() {
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(execute().await.len(), 3);
}
