//! Compatibility-only actual native reads after normal support.
//! This does not start a normal original, enrol 143k clients, or grant Ready.
use super::super::client_watch::{ClientWatch, TrafficCohort};
use super::super::read_recipes::{
    assert_mcp_reply, collect_native_rpc_reads, mcp_cases, mcp_read_method,
    verify_native_rpc_reads, McpCaller,
};
use super::*;
use futures_util::FutureExt;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "native59 post-support compatibility only; run serial at NI10, not a normal qualification trial"]
async fn actual_full_read_recipes_survive_normal_support_setup() {
    crate::git_fixture::environment::isolated_git_test!();
    assert_eq!(
        crate::priority::own_nice(),
        10,
        "actual NI10 control required"
    );
    let artifacts = tempfile::Builder::new()
        .prefix("qualification-native59-NoQualification-")
        .tempdir()
        .unwrap()
        .keep();
    eprintln!("native59_compatibility_artifacts={}", artifacts.display());
    let name = TrialName {
        envelope: Envelope::Small,
        number: 1,
    };
    let mut prepared =
        PreparedBackend::unassembled(name, PipelineMode::Qualification, marker()).await;
    let mut rpc_replies = Vec::new();
    let mut mcp_replies = Vec::new();
    let mut read_inputs = None;
    let mut mcp_expected = None;
    let collected = std::panic::AssertUnwindSafe(async {
        prepared.assemble_support().await?;
        if prepared.watch.is_some() || prepared.lifecycle.is_some() || prepared.mcp.is_some() {
            return Err(
                "support setup enrolled trial collectors before compatibility boundary".into(),
            );
        }
        // This boundary includes the same actual git/terminal/subscription/
        // producer setup as normal assembly. No normal collector was started.
        let reads = Arc::clone(
            prepared
                .reads
                .as_ref()
                .ok_or("actual native reads missing")?,
        );
        read_inputs = Some(Arc::clone(&reads));
        let fixture = prepared.fixture.as_ref().ok_or("owned fixture missing")?;
        rpc_replies = collect_native_rpc_reads(&fixture.rpc, &reads).await;
        for (index, (case, proof)) in rpc_replies.iter().enumerate() {
            let source_slot = prepared
                .plan
                .iter()
                .find(|slot| {
                    slot.route == super::super::trial_complete::Route::SpaRpc
                        && slot.traffic == super::super::trial_complete::Traffic::Read
                        && slot.method.as_deref() == Some(case.method)
                })
                .ok_or("actual normal RPC source slot missing")?;
            save(
                &artifacts,
                &format!("rpc-{index}.json"),
                &json!({
                    "method":case.method,"source_slot":source_slot,
                    "original_id":proof.original_id,"reply":proof.reply,
                    "qualification":false
                }),
            )?;
        }
        collect_mcp_after_support(&mut prepared, &artifacts, &mut mcp_replies).await?;
        mcp_expected = Some(prepared.mcp_inputs.remove(0).1);
        Ok::<(), String>(())
    })
    .catch_unwind()
    .await;
    // All errors, including decoded-oracle errors, are inspected only after
    // the real existing bounded shutdown of exactly these owned resources.
    let cleanup = prepared.shutdown().await;
    let (cleanup_detail, cleanup_proven) = match cleanup {
        Ok(e) => (
            format!(
                "producers_joined={} mcp_quiet={} watch_joined={} native_closed={}",
                e.producers_joined, e.mcp_quiet, e.client_watch_joined, e.native_cleanup_completed
            ),
            e.producers_joined
                && e.mcp_quiet
                && e.client_watch_joined
                && e.native_cleanup_completed,
        ),
        Err(failure) => {
            let reason = failure.reason;
            std::mem::forget(failure.retained);
            save(
                &artifacts,
                "cleanup-failure.json",
                &json!({"reason":reason,"qualification":false}),
            )
            .unwrap();
            panic!("actual native59 cleanup failed; retained owned resources: {reason}");
        }
    };
    save(
        &artifacts,
        "summary.json",
        &json!({
            "qualification":false,"normal_trials_started":0,"rpc_replies":rpc_replies.len(),
            "mcp_replies":mcp_replies.len(),"cleanup":cleanup_detail,
            "boundary":"actual normal support assembled; no normal collector enrollment",
            "mcp_schedule":"14 immutable offsets, 150ms apart; original100ms read budget"
        }),
    )
    .unwrap();
    match collected {
        Err(original) => std::panic::resume_unwind(original),
        Ok(Err(error)) => panic!(
            "actual native59 collection failed: {error}; artifacts={}",
            artifacts.display()
        ),
        Ok(Ok(())) => {}
    }
    assert!(
        cleanup_proven,
        "actual native59 cleanup evidence incomplete"
    );
    assert_eq!(rpc_replies.len(), 45);
    assert_eq!(mcp_replies.len(), 14);
    let reads = read_inputs.expect("actual native saved witnesses");
    let mut oracle_errors = Vec::new();
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        verify_native_rpc_reads(&reads, &rpc_replies)
    })) {
        oracle_errors.push(format!("RPC native oracle: {}", panic_detail(&*panic)));
    }
    let expected = mcp_expected.expect("actual MCP saved witness retained");
    for (action, proof) in mcp_replies {
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_mcp_reply(&action, &proof.reply, &expected)
        })) {
            oracle_errors.push(format!("{}: {}", action.tool_name(), panic_detail(&*panic)));
        }
    }
    save(
        &artifacts,
        "oracle-errors.json",
        &json!({"errors":oracle_errors,"qualification":false}),
    )
    .unwrap();
    assert!(
        oracle_errors.is_empty(),
        "native59 post-support oracles failed: {oracle_errors:?}"
    );
}

async fn collect_mcp_after_support(
    prepared: &mut PreparedBackend,
    artifacts: &Path,
    replies: &mut Vec<(crate::mcp::BridgeAction, super::super::rpc::ReplyProof)>,
) -> Result<(), String> {
    let (inputs, _) = prepared
        .mcp_inputs
        .first()
        .ok_or("actual owner0 read inputs missing")?;
    let cases = mcp_cases(inputs);
    if cases.len() != 14 {
        return Err("native MCP catalog drift".into());
    }
    let mut schedule = Vec::new();
    let watch = ClientWatch::new(Arc::new(Instant::now));
    for (index, (caller, action)) in cases.into_iter().enumerate() {
        let method = mcp_read_method(&action);
        let source = prepared
            .plan
            .iter()
            .find(|slot| {
                slot.traffic == super::super::trial_complete::Traffic::Read
                    && slot.route == super::super::trial_complete::Route::AgentMcp
                    && slot.owner == 0
                    && slot.method.as_deref() == Some(method.as_str())
            })
            .ok_or("actual normal source slot missing")?;
        let id = format!("compatibility-only/{index}/{}", source.original_id);
        let due = Duration::from_millis(index as u64 * 150);
        watch.register_relative_at(&id, due, TrafficCohort::Read);
        save(
            artifacts,
            &format!("mcp-source-{index}.json"),
            &json!({
                "source_slot":source,"derived_control_id":id,"fixed_due_us":due.as_micros(),
                "qualification":false
            }),
        )?;
        schedule.push((caller, action, id, due));
    }
    let fixture = prepared.fixture.as_ref().ok_or("owned fixture missing")?;
    let reads = prepared
        .reads
        .as_deref()
        .ok_or("native read witnesses missing")?;
    let guard = watch.watch();
    prepared.lifecycle = Some(watch_owner::WatchOwner::new(guard));
    prepared.watch = Some(watch.clone());
    prepared.mcp = Some(McpHarness::new(
        Arc::clone(&fixture.state),
        Arc::clone(&fixture.registry),
        setup::actual_identities(fixture, reads),
        watch.clone(),
    )?);
    let start = Instant::now();
    watch.start_at(start);
    for (index, (caller, action, id, due)) in schedule.into_iter().enumerate() {
        tokio::time::sleep_until((start + due).into()).await;
        watch.offer(&id);
        let endpoint = match caller {
            McpCaller::Coding => 0,
            McpCaller::Router => 8,
        };
        let proof = prepared
            .mcp
            .as_ref()
            .unwrap()
            .execute(endpoint, &id, action.clone(), true)
            .await?;
        save(
            artifacts,
            &format!("mcp-{index}.json"),
            &json!({
                "method":mcp_read_method(&action),"original_id":proof.original_id,
                "reply":proof.reply,"qualification":false
            }),
        )?;
        replies.push((action, proof));
    }
    Ok(())
}

fn save(root: &Path, name: &str, value: &serde_json::Value) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(name))
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}
pub(super) fn marker() -> Provenance {
    Provenance {
        source_revision: "NoQualification-source-bound-by-outer-runner".into(),
        source_tree: "NoQualification-source-bound-by-outer-runner".into(),
        binary_sha256: "NoQualification-binary-bound-by-outer-runner".into(),
        profile: format!(
            "compatibility-only; debug_assertions={}",
            cfg!(debug_assertions)
        ),
        nice: i64::from(crate::priority::own_nice()),
        cpu_affinity: vec![],
        fresh_quiet_release_sha256: "NoQualification".into(),
        fixture_counts: json!({"qualification":false,"actual_support_compatibility_only":true}),
        fixture_seed: "normal-native59-support-compatibility-only".into(),
        service_hook_contract_sha256: "NoQualification-no-semantic-writes".into(),
    }
}

fn panic_detail(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|value| (*value).into()))
        .unwrap_or_else(|| "non-string actual native assertion panic".into())
}
