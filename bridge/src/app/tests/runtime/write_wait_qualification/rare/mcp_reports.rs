//! Terminal reports are actual no-reply MCP writes, separate from action43.
use super::*;
use crate::app::mcp::{handle_done_stream, ControlPlane};
use crate::mcp::{DoneReport, DoneStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod plan;
mod routers;
mod runs;

pub(super) struct McpReportProof {
    original_id: String,
    report: DoneReport,
    sent_at: Instant,
    closed_at: Instant,
}

pub(super) struct VerifiedReport {
    pub(super) original_id: String,
    pub(super) report: DoneReport,
    pub(super) effect: Value,
    pub(super) wait: WriteSnapshot,
}

impl RareFixture {
    /// Half-close only the client's sending direction. The daemon consumes the
    /// real report, reaches EOF, and closes its sending direction without an
    /// acknowledgement. A fabricated success response cannot prove this path.
    pub(super) async fn report_mcp(
        &self,
        caller: &str,
        token: &str,
        report: DoneReport,
    ) -> McpReportProof {
        assert!(!self.breached.load(Ordering::Acquire));
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        let original_id = format!(
            "qual/rare/{}/{}/report/{sequence:04}",
            self.family, self.fixture_id
        );
        let plane = ControlPlane::new(&self.state().lock().unwrap());
        let (mut client, daemon) = tokio::net::UnixStream::pair().unwrap();
        let mut server = tokio::spawn(handle_done_stream(Arc::clone(self.state()), daemon, plane));
        let frame = json!({
            "id":original_id,"task_id":caller,"session_token":token,"report":report,
        });
        let sent_at = Instant::now();
        client
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        let mut response = Vec::new();
        let read = tokio::time::timeout(WAIT_BUDGET, client.read_to_end(&mut response)).await;
        let closed_at = Instant::now();
        drop(client);
        let joined = tokio::time::timeout(WAIT_BUDGET, &mut server).await;
        if joined.is_err() {
            server.abort();
            let _cancelled = server.await;
        }
        joined
            .expect("owned MCP report handler did not finish")
            .unwrap();
        read.expect("actual report exceeded five seconds").unwrap();
        assert!(
            response.is_empty(),
            "reports must not acknowledge: {response:?}"
        );
        eprintln!(
            "rare_mcp_report={}",
            json!({"original_id":original_id,"caller":caller,"report":report,
                "response_bytes":response.len(),"socket_ms":closed_at.duration_since(sent_at).as_secs_f64()*1000.0})
        );
        McpReportProof {
            original_id,
            report,
            sent_at,
            closed_at,
        }
    }

    pub(super) async fn verify_report(
        &self,
        proof: McpReportProof,
        effect: Value,
    ) -> VerifiedReport {
        assert!(effect.is_object() && !effect.as_object().unwrap().is_empty());
        assert!(proof.closed_at.duration_since(proof.sent_at) <= WAIT_BUDGET);
        let wait = self.settled_origin(&proof.original_id).await;
        assert_eq!(wait.method, "mcp.control:done");
        assert_eq!(wait.cohort, WriteCohort::SemanticWrite);
        assert!(wait.admitted_at.is_some());
        assert!(
            wait.acknowledged_at.is_none(),
            "a no-reply report has no ACK"
        );
        assert!(!wait.cancelled && !wait.abandoned && !wait.breached());
        assert!(wait.conservative_upper <= WAIT_BUDGET);
        assert!(wait.max_chargeable_interval <= WAIT_BUDGET);
        eprintln!(
            "rare_report_verified={}",
            json!({"original_id":proof.original_id,"report":proof.report,"effect":effect,
                "acknowledged":false,"known_wait_ms":wait.known_wait_union.as_secs_f64()*1000.0,
                "sqlite_service_ms":wait.sql_service_union.as_secs_f64()*1000.0,
                "conservative_upper_ms":wait.conservative_upper.as_secs_f64()*1000.0})
        );
        VerifiedReport {
            original_id: proof.original_id,
            report: proof.report,
            effect,
            wait,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_done_statuses_have_actual_coding_and_router_no_reply_proofs() {
    crate::git_fixture::environment::isolated_git_test!();
    let mut reports = runs::execute().await;
    reports.extend(routers::execute().await);
    reports.push(plan::execute().await);
    assert_eq!(reports.len(), 7);
    let mut original_ids = std::collections::BTreeSet::new();
    for report in &reports {
        assert!(original_ids.insert(&report.original_id));
        assert!(report.wait.root_finished && report.wait.acknowledged_at.is_none());
        assert!(!report.effect.as_object().unwrap().is_empty());
    }
    for status in [
        DoneStatus::Completed,
        DoneStatus::Blocked,
        DoneStatus::Failed,
    ] {
        assert_eq!(
            reports
                .iter()
                .filter(|row| row.report.status == status)
                .count(),
            if status == DoneStatus::Completed {
                3
            } else {
                2
            }
        );
    }
}
