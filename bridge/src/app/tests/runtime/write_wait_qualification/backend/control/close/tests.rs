//! A failed real deadline must survive the actual full-fixture CloseJob path.
use super::*;
use crate::app::tests::runtime::write_wait_qualification::read_recipes::FullReadOutcome;
use crate::app::tests::runtime::write_wait_qualification::trial_complete::{
    Envelope, JsonArtifacts, Route, Traffic,
};
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "actual full native CloseJob with real5s breach; explicit quiet release NI10"]
async fn actual_close_job_audits_failed_terminal_watch_before_full_native_cleanup() {
    assert!(
        !cfg!(debug_assertions),
        "native close control requires release-opt3"
    );
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(crate::priority::own_nice(), 10);
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-close-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("native_close_control_artifacts={}", artifacts.display());
    let name = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let mut prepared = PreparedBackend::unassembled(
        name,
        PipelineMode::Qualification,
        super::super::super::preparation_tests::marker(),
    )
    .await;
    let setup = std::panic::AssertUnwindSafe(prepare_one(&mut prepared, &artifacts))
        .catch_unwind()
        .await;
    let (original, params) = match setup {
        Ok(Ok(selected)) => selected,
        other => {
            let fallback =
                super::super::super::native_read_tests::close_attached(prepared, &artifacts).await;
            panic!(
                "native close setup failed before control; setup={other:?}, actual cleanup={fallback:?}"
            );
        }
    };
    let watch = prepared.watch.as_ref().unwrap().clone();
    let owner = Arc::clone(prepared.lifecycle.as_ref().unwrap());
    let weak = Arc::downgrade(&prepared.fixture.as_ref().unwrap().state);
    let start = Instant::now() + Duration::from_millis(10);
    let observe = Observation::for_native_control_at(&prepared.plan, start);
    watch.start_at(start);
    // This is deliberately a failed control original, never a qualification
    // sample. Use the unchanged actual5s watchdog and its real monotonic clock.
    tokio::time::sleep_until((start + Duration::from_millis(5100)).into()).await;
    let observer = owner.observer();
    let observed_until = Instant::now() + Duration::from_secs(2);
    while !observer.failed() && Instant::now() < observed_until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let failed_before_reply = observer.failed();
    // Send and clean up even if the bounded observation did not arrive.
    watch.offer(&original);
    let proof = prepared
        .fixture
        .as_ref()
        .unwrap()
        .rpc
        .request(0, &original, "tasks.get", params.clone())
        .await;
    let before_close = watch.reconcile_final();
    let sink: Arc<dyn ArtifactSink> =
        Arc::new(JsonArtifacts::new(artifacts.join("engine")).unwrap());
    let completed =
        completed_origins::CompletedOrigins::new(Vec::<(String, String)>::new()).unwrap();
    let backend = ControlBackend::from_attached(prepared, sink, completed);
    *backend.observation.lock().unwrap() = Some(observe);
    for producer in &backend.producers.handles {
        producer.stop_offering();
    }
    backend.start_close();
    let task = backend
        .cleanup_task
        .lock()
        .unwrap()
        .take()
        .expect("actual owned CloseJob task");
    let task_result = task.await;
    // Capture the engine result before any fallback; later cleanup cannot turn
    // a failed CloseJob into a successful observation.
    let actual_closed = backend.closed.load(Ordering::Acquire);
    let engine_errors = backend.close_error.lock().unwrap().clone();
    let archived: Result<Value, String> = std::fs::read(
        artifacts
            .join("engine")
            .join(name.label())
            .join("transport-final.json"),
    )
    .map_err(|error| error.to_string())
    .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()));
    let retained = backend.resources.write().await.take();
    let fallback = if let Some(prepared) = retained {
        if prepared.fixture.is_some() {
            Some(super::super::super::native_read_tests::close_attached(prepared, &artifacts).await)
        } else {
            drop(prepared);
            None
        }
    } else {
        None
    };
    owner.request_finish();
    let until = Instant::now() + Duration::from_secs(5);
    while !owner.is_finished() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let report = owner.report();
    drop(backend);
    let until = Instant::now() + Duration::from_secs(10);
    while weak.upgrade().is_some() && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    std::fs::write(
        artifacts.join("control-result.json"),
        serde_json::to_vec_pretty(&json!({
            "qualification":false,"qualification_successes":0,"actual_closed":actual_closed,"failed_before_reply":failed_before_reply,
            "engine_errors":engine_errors,"fallback":format!("{fallback:?}"),"archive":archived,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        task_result.is_ok(),
        "actual CloseJob task failed: {task_result:?}"
    );
    assert!(
        weak.upgrade().is_none(),
        "actual AppState resources must be released before assertions"
    );
    assert!(
        failed_before_reply,
        "actual watchdog must observe failure before terminal reply"
    );
    assert_eq!(proof.reply["ok"], true, "{}", proof.reply);
    assert_eq!(proof.reply["result"]["task"]["id"], params["task_id"]);
    assert_eq!(before_close.rows.len(), 1);
    assert!(before_close.rows[0].breached && before_close.rows[0].final_at.is_some());
    assert!(
        actual_closed,
        "actual CloseJob must audit before full native cleanup: {engine_errors:?}; fallback={fallback:?}"
    );
    let report = report.expect("actual watchdog joined");
    assert!(report.failed && !report.panicked && !report.breaches.is_empty());
    assert!(
        !engine_errors.is_empty(),
        "failed workload cannot become a clean result"
    );
    let archived = archived.expect("actual final transport audit persisted");
    assert_eq!(archived["qualification"], false);
    assert_eq!(archived["native_joined"], true);
    assert_eq!(archived["guard_joined"], true);
    assert_eq!(archived["watch_report"]["failed"], true);
    for key in ["reconciliation_before_cleanup", "reconciliation"] {
        assert_eq!(archived[key]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(archived[key]["rows"][0]["original_id"], original);
        assert_eq!(archived[key]["rows"][0]["breached"], true);
        assert!(!archived[key]["violations"].as_array().unwrap().is_empty());
    }
    assert!(!archived["audit_violations"].as_array().unwrap().is_empty());
}

async fn prepare_one(
    prepared: &mut PreparedBackend,
    artifacts: &std::path::Path,
) -> Result<(String, Value), String> {
    prepared.assemble_support().await?;
    if prepared.watch.is_some() || prepared.lifecycle.is_some() {
        return Err("normal originals enrolled before explicit close control selection".into());
    }
    // This setup-only read exercises the isolated gh stand-in required by
    // full native cleanup. It precedes collectors and trial watch attachment.
    // The caller catches setup/oracle failure and performs actual cleanup.
    let case = prepared
        .rpc_cases
        .iter()
        .find(|case| case.method == "github.repos")
        .ok_or("native GitHub prerequisite case missing")?;
    let fixture = prepared
        .fixture
        .as_ref()
        .ok_or("owned full fixture missing")?;
    let reads = prepared
        .reads
        .as_ref()
        .ok_or("native read witnesses missing")?;
    let id = "fixture-prerequisite/close-job/github.repos";
    let proof = fixture
        .rpc
        .request(0, id, case.method, case.params.clone())
        .await;
    std::fs::write(
        artifacts.join("fixture-service-reply.json"),
        serde_json::to_vec_pretty(&json!({
            "qualification":false,"trial_original":false,"boundary":"before watch attachment",
            "original_id":proof.original_id,"method":proof.method,"reply":proof.reply,
        }))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if proof.original_id != id || proof.reply["id"] != id || proof.method != case.method {
        return Err("native GitHub fixture prerequisite reply identity mismatch".into());
    }
    if reads.assert_rpc_reply(case, &proof.reply) != FullReadOutcome::Ready {
        return Err("native GitHub fixture prerequisite was not Ready".into());
    }
    let source = prepared
        .plan
        .iter()
        .find(|slot| {
            slot.traffic == Traffic::Read
                && slot.route == Route::SpaRpc
                && slot.method.as_deref() == Some("tasks.get")
        })
        .ok_or("genuine normal tasks.get source slot missing")?
        .clone();
    let params = prepared
        .rpc_cases
        .iter()
        .find(|case| case.method == "tasks.get")
        .ok_or("native tasks.get case missing")?
        .params
        .clone();
    let mut selected = source.clone();
    selected.original_id = format!("close-control/{}", source.original_id);
    selected.sequence = 0;
    selected.due_us = 0;
    prepared.plan = vec![selected.clone()];
    std::fs::write(artifacts.join("schedule.json"), serde_json::to_vec_pretty(&json!({
        "qualification":false,"normal_originals_enrolled":0,"source_slot":source,"selected_slot":selected,
    })).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    prepared.prepare_collectors()?;
    prepared
        .fixture
        .as_mut()
        .ok_or("owned full fixture missing")?
        .rpc
        .attach_client_watch(
            prepared
                .watch
                .as_ref()
                .ok_or("actual watch missing")?
                .clone(),
        );
    Ok((selected.original_id, params))
}
