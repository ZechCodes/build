use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity};
use crate::harness::{pi::PiHarness, Harness};
use crate::models::{AgentProvider, ModelChoice};
use serde_json::json;

fn telemetry() -> PiTelemetry {
    let mut policy = PiHarness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens: 100,
                source: CompactionSource::Device,
            },
            ModelChoice {
                provider: AgentProvider::Pi,
                ..ModelChoice::default()
            },
            Some("0.86.1".into()),
            CompactionLaunchIdentity {
                id: "launch-one".into(),
                thread_id: "thread-one".into(),
            },
        )
        .unwrap();
    policy.enforcement = CompactionEnforcement::BetweenTurn;
    PiTelemetry::new(PiTelemetryConfig {
        path: "/unused".into(),
        owner: "agent-one".into(),
        policy,
    })
}

fn event(sequence: u64, kind: &str) -> Value {
    json!({"generation": 1, "sequence": sequence, "launch_id":"launch-one", "thread_id":"thread-one", "owner":"agent-one", "type":kind})
}

fn context(sequence: u64, tokens: Option<u64>) -> Value {
    let mut value = event(sequence, "context");
    value["tokens"] = json!(tokens);
    value
}

#[test]
fn pi_long_turn_crosses_threshold_but_only_settled_allows_fallback() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.sent(false).unwrap();
    telemetry.observe(&event(2, "working"));
    telemetry.observe(&context(3, Some(101)));
    for (sequence, kind) in [(4, "turn_end"), (5, "agent_end")] {
        telemetry.observe(&event(sequence, kind));
        assert_eq!(telemetry.snapshot().status, AgentStatus::Working);
        assert!(!telemetry.automatic_due(Some(101)));
        assert!(
            telemetry.sent(true).is_err(),
            "manual compact must not abort active tools"
        );
    }
    telemetry.observe(&event(6, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
    assert!(telemetry.automatic_due(Some(101)));
    telemetry.sent(true).unwrap();
    assert!(
        !telemetry.automatic_due(Some(101)),
        "one request owns the idle boundary"
    );
}

#[test]
fn pi_null_reading_invalidates_old_context_and_identical_fresh_usage_is_new() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&context(2, Some(101)));
    telemetry.observe(&event(3, "settled"));
    assert!(telemetry.automatic_due(Some(101)));
    telemetry.observe(&context(4, None));
    assert_eq!(telemetry.snapshot().context, None);
    assert!(!telemetry.automatic_due(Some(101)));
    telemetry.observe(&context(5, Some(101)));
    assert_eq!(telemetry.snapshot().context.unwrap().context_tokens, 101);
    assert!(telemetry.automatic_due(Some(101)));
}

#[test]
fn pi_extension_readiness_and_failure_are_explicit_unsupported_states() {
    let telemetry = telemetry();
    assert_eq!(
        telemetry.policy().enforcement,
        CompactionEnforcement::Unsupported
    );
    assert_eq!(
        telemetry.policy().reason.as_deref(),
        Some("pi_extension_unconfirmed")
    );
    telemetry.observe(&event(1, "ready"));
    assert_eq!(
        telemetry.policy().enforcement,
        CompactionEnforcement::BetweenTurn
    );
    telemetry.observe(&event(2, "unsupported"));
    assert_eq!(
        telemetry.policy().enforcement,
        CompactionEnforcement::Unsupported
    );
    assert_eq!(
        telemetry.policy().reason.as_deref(),
        Some("pi_extension_unavailable")
    );
    telemetry.observe(&event(3, "ready"));
    assert_eq!(
        telemetry.policy().enforcement,
        CompactionEnforcement::Unsupported
    );
}

#[test]
fn pi_foreign_and_replayed_observations_never_unlock_another_turn() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&event(2, "settled"));
    telemetry.sent(false).unwrap();
    telemetry.observe(&event(2, "settled"));
    for field in ["owner", "thread_id", "launch_id"] {
        let mut foreign = event(3, "settled");
        foreign[field] = json!("foreign");
        telemetry.observe(&foreign);
    }
    assert_eq!(telemetry.snapshot().status, AgentStatus::Working);
    telemetry.observe(&event(3, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
}

#[test]
fn pi_extension_failure_preserves_the_owned_child_and_ordinary_input() {
    use crate::harness::AgentSession;
    let directory = tempfile::tempdir().unwrap();
    let mut config = telemetry().config.clone();
    config.path = directory.path().join("telemetry.jsonl");
    let mut spec = crate::pty::HarnessSpec::new("python3")
        .arg("-c")
        .arg("import sys,time; open(sys.argv[1], 'a').write(sys.argv[2]); time.sleep(60)")
        .arg(config.path.to_string_lossy())
        .arg(format!(
            "{}\n{}\n",
            event(1, "ready"),
            event(2, "unsupported")
        ));
    spec.pi_telemetry = Some(Box::new(config));
    let session =
        crate::pty::PtySession::spawn(&spec, None, portable_pty::PtySize::default()).unwrap();
    let exited = session.exited_within(std::time::Duration::from_secs(2));
    let ordinary = session.send_turn(&crate::harness::Turn::new("keep working"));
    let compact = session.send_turn(&crate::harness::Turn::new("/compact"));
    // Always reclaim only the fixture child after observing its survival.
    session.end();
    assert!(!exited, "compaction observation failure must never kill Pi");
    assert!(ordinary.is_ok(), "ordinary input must survive degradation");
    assert!(
        compact.is_err(),
        "Build compaction must stop after degradation"
    );
    assert_eq!(
        session.compaction_policy().unwrap().enforcement,
        CompactionEnforcement::Unsupported
    );
}

async fn finish_after_terminal_eof(
    telemetry: &Arc<PiTelemetry>,
) -> broadcast::Receiver<ActivityReport> {
    let activity = telemetry.activity().unwrap();
    let mut status = telemetry.subscribe();
    // The child has appended its final records and the PTY reader has reached
    // EOF before the sidecar follower's next poll. Retain the session owner.
    let alive = Arc::new(Mutex::new(None));
    telemetry.follow(Arc::downgrade(&alive), || {
        panic!("a process whose terminal is already closed must not be killed")
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        status.wait_for(|snapshot| matches!(snapshot.status, AgentStatus::Ended { .. })),
    )
    .await
    .expect("terminal EOF must publish Ended")
    .unwrap();
    activity
}

#[tokio::test]
async fn pi_terminal_eof_drains_final_compaction_before_closing_activity() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = telemetry().config.clone();
    config.path = directory.path().join("telemetry.jsonl");
    let mut started = event(2, "compaction");
    started["completed"] = json!(false);
    started["reason"] = json!("manual");
    let mut completed = event(3, "compaction");
    completed["completed"] = json!(true);
    completed["reason"] = json!("manual");
    std::fs::write(
        &config.path,
        format!(
            "{}\n{started}\n{completed}\n{}\n",
            event(1, "ready"),
            event(4, "shutdown")
        ),
    )
    .unwrap();
    let telemetry = Arc::new(PiTelemetry::new(config));
    let mut activity = finish_after_terminal_eof(&telemetry).await;
    assert_eq!(
        activity.try_recv().unwrap().activity,
        AgentActivity::Compaction { completed: false }
    );
    assert_eq!(
        activity.try_recv().unwrap().activity,
        AgentActivity::Compaction { completed: true }
    );
    assert!(telemetry.confirmed());
    assert_eq!(telemetry.snapshot().context, None);
    assert!(telemetry.sent(false).is_err());
    assert!(telemetry.sent(true).is_err());
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), activity.recv())
            .await
            .expect("activity must close even while its Pi session is retained"),
        Err(broadcast::error::RecvError::Closed)
    ));
}

#[tokio::test]
async fn pi_terminal_eof_preserves_final_unsupported_and_closes_activity() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = telemetry().config.clone();
    config.path = directory.path().join("telemetry.jsonl");
    std::fs::write(&config.path, format!("{}\n", event(2, "unsupported"))).unwrap();
    let telemetry = Arc::new(PiTelemetry::new(config));
    telemetry.observe(&event(1, "ready"));
    let mut activity = finish_after_terminal_eof(&telemetry).await;
    assert!(
        telemetry.failed(),
        "the final unsupported record must survive EOF"
    );
    assert_eq!(
        telemetry.policy().reason.as_deref(),
        Some("pi_extension_unavailable")
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), activity.recv())
            .await
            .expect("failed Pi activity must close with its owner retained"),
        Err(broadcast::error::RecvError::Closed)
    ));
}

#[test]
fn pi_lost_heartbeat_withdraws_policy_even_when_the_sink_cannot_report_failure() {
    let telemetry = telemetry();
    let started = Instant::now();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&context(2, Some(101)));
    telemetry.check_health(started, started + Duration::from_secs(11));
    assert_eq!(
        telemetry.policy().enforcement,
        CompactionEnforcement::Unsupported
    );
    assert!(!telemetry.automatic_due(Some(101)));
    assert!(telemetry.sent(false).is_ok());
    assert!(telemetry.sent(true).is_err());
}

#[test]
fn pi_slow_mcp_initialization_has_a_startup_budget_and_heartbeat_health() {
    let telemetry = telemetry();
    let started = Instant::now();
    telemetry.check_health(started, started + Duration::from_secs(61));
    assert!(
        !telemetry.failed(),
        "two 30-second MCP calls must fit startup grace"
    );
    telemetry.observe(&event(1, "starting"));
    telemetry.observe(&event(2, "heartbeat"));
    telemetry.check_health(started, Instant::now() + Duration::from_secs(5));
    assert!(!telemetry.failed());
    telemetry.observe(&event(3, "ready"));
    assert!(telemetry.confirmed());
}

#[test]
fn pi_reload_accepts_a_new_generation_and_rejects_retired_replays() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&context(2, Some(101)));
    telemetry.observe(&event(3, "working"));
    let next = |sequence, kind| {
        let mut value = event(sequence, kind);
        value["generation"] = json!(2);
        value
    };
    telemetry.observe(&next(1, "starting"));
    assert!(!telemetry.confirmed());
    assert_eq!(telemetry.snapshot().context, None);
    telemetry.observe(&next(2, "ready"));
    telemetry.observe(&next(3, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
    telemetry.observe(&event(4, "working"));
    telemetry.observe(&context(5, Some(999)));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
    assert_eq!(telemetry.snapshot().context, None);
    telemetry.sent(false).unwrap();
    telemetry.observe(&next(3, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Working);
}

#[test]
fn pi_failure_invalidates_once_per_generation_and_reload_can_requalify() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&context(2, Some(101)));
    let before = telemetry.snapshot().context_reset_count;
    let started = Instant::now();
    for _ in 0..40 {
        telemetry.check_health(started, started + Duration::from_secs(11));
    }
    assert_eq!(telemetry.snapshot().context_reset_count, before + 1);
    assert!(telemetry.failed());
    let mut ready = event(1, "ready");
    ready["generation"] = json!(2);
    telemetry.observe(&ready);
    assert!(telemetry.confirmed());
    let next = telemetry.snapshot().context_reset_count;
    telemetry.fail();
    telemetry.fail();
    assert_eq!(telemetry.snapshot().context_reset_count, next + 1);
}

#[tokio::test]
async fn pi_observer_receipt_advances_only_for_read_records_and_reports_failure() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let mut config = telemetry().config.clone();
    config.path = directory.path().join("telemetry.jsonl");
    std::fs::write(&config.path, format!("{}\n", event(1, "ready"))).unwrap();
    let receipt = directory.path().join("telemetry.jsonl.ack");
    let telemetry = Arc::new(PiTelemetry::new(config));
    let alive = Arc::new(Mutex::new(Some(broadcast::channel(1).0)));
    telemetry.follow(Arc::downgrade(&alive), || AgentStatus::Waiting);
    let acknowledged = wait_for_receipt(&receipt, "ready").await;
    assert_eq!(acknowledged["sequence"], 1);
    assert_eq!(acknowledged["generation"], 1);
    assert_eq!(acknowledged["owner"], "agent-one");
    assert_eq!(
        std::fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::set_permissions(telemetry.path(), std::fs::Permissions::from_mode(0o200)).unwrap();
    assert!(
        std::fs::read(telemetry.path()).is_err(),
        "probe requires actual reader permission loss"
    );
    use std::io::Write;
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(telemetry.path())
        .unwrap();
    writeln!(writer, "{}", event(2, "heartbeat")).unwrap();
    telemetry.check_health(Instant::now(), Instant::now() + Duration::from_secs(11));
    let failed = wait_for_receipt(&receipt, "unsupported").await;
    assert_eq!(
        failed["sequence"], 1,
        "unread writes must not renew observation"
    );
    alive.lock().unwrap().take();
}

async fn wait_for_receipt(path: &std::path::Path, status: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(value) =
                std::fs::read(path).map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
            {
                if value["status"] == status {
                    break value;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("observer must publish its receipt")
}

/// Opt-in carrier probe: runs the real Rust observer, never a bridge daemon.
#[test]
#[ignore = "requires isolated qualification paths supplied by the pinned-Pi driver"]
fn pi_qualification_observer() {
    let mut config = telemetry().config.clone();
    config.path = std::env::var_os("PI_QUAL_LOG").unwrap().into();
    config.owner = std::env::var("PI_QUAL_OWNER").unwrap();
    config.policy.requested.tokens = 150000;
    config.policy.launch_identity.thread_id = std::env::var("PI_QUAL_THREAD_ID").unwrap();
    config.policy.launch_identity.id = std::env::var("PI_QUAL_LAUNCH_ID").unwrap();
    let report = PathBuf::from(std::env::var_os("PI_QUAL_REPORT").unwrap());
    let stop = PathBuf::from(std::env::var_os("PI_QUAL_STOP").unwrap());
    let telemetry = Arc::new(PiTelemetry::new(config));
    let alive = Arc::new(Mutex::new(Some(broadcast::channel(1).0)));
    telemetry.follow(Arc::downgrade(&alive), || AgentStatus::Waiting);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !stop.exists() && Instant::now() < deadline {
        let snapshot = telemetry.snapshot();
        let value = json!({
            "enforcement": format!("{:?}", telemetry.policy().enforcement),
            "reason": telemetry.policy().reason,
            "status": format!("{:?}", snapshot.status),
            "context_reset_count": snapshot.context_reset_count,
        });
        let temporary = report.with_extension("tmp");
        std::fs::write(&temporary, value.to_string()).unwrap();
        std::fs::rename(temporary, &report).unwrap();
        std::thread::sleep(Duration::from_millis(25));
    }
    alive.lock().unwrap().take();
    assert!(stop.exists(), "qualification driver must stop its observer");
}
