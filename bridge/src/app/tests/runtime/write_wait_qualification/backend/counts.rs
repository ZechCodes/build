//! Bind measured native fixture state after support bootstrap and before T0.
use super::*;
use serde_json::{Value, json};

pub(super) fn bind(prepared: &mut PreparedBackend) -> Result<(), String> {
    if prepared.mode != PipelineMode::Qualification {
        return Ok(());
    }
    let actual = prepared
        .fixture
        .as_ref()
        .ok_or("owned normal fixture missing")?
        .qualification_count_receipt();
    let supplied = prepared.provenance.fixture_counts.clone();
    let index = usize::from(prepared.name.envelope == Envelope::Large);
    let declared = supplied["expected_envelopes"]
        .as_array()
        .and_then(|rows| rows.get(index))
        .ok_or("normal fixture declaration missing")?;
    let expected = expected(prepared.name.envelope);
    if supplied["qualification"] != true || declared != &expected {
        return Err(format!(
            "normal declaration differs from pinned envelope: declared={declared}; expected={expected}; measured={actual}"
        ));
    }
    verify(&actual, &expected)?;
    prepared.provenance.fixture_counts = json!({
        "supplied_contract":supplied,"actual_pre_t0":actual,"trial":prepared.name,
        "source_revision":prepared.provenance.source_revision,
        "source_tree":prepared.provenance.source_tree,
        "binary_sha256":prepared.provenance.binary_sha256,
        "boundary":"after real native support/bootstrap; before first caller or producer start"
    });
    Ok(())
}
fn expected(envelope: Envelope) -> Value {
    match envelope {
        Envelope::Small => json!({"label":"small","projects":4,"tracker_tasks":100,
            "registry_workspaces":10,"active_workspaces":8,"canonical_histories":16,"thread_items":1000}),
        Envelope::Large => json!({"label":"large","projects":4,"tracker_tasks":10000,
            "registry_workspaces":200,"active_workspaces":8,"canonical_histories":206,"thread_items":100000}),
    }
}
fn verify(actual: &Value, expected: &Value) -> Result<(), String> {
    let mapped = [
        ("projects", "projects"),
        ("registry_residents", "registry_workspaces"),
        ("active_workspaces", "active_workspaces"),
        ("canonical_histories", "canonical_histories"),
        ("total_thread_items", "thread_items"),
    ];
    let expected_tasks = expected["tracker_tasks"]
        .as_u64()
        .ok_or("pinned task total missing")?;
    let task_counts = actual["task_counts"]
        .as_array()
        .ok_or("native task counts missing")?;
    let correct = mapped
        .iter()
        .all(|(measured, declared)| actual[*measured] == expected[*declared])
        && task_counts.len() == 4
        && task_counts
            .iter()
            .all(|count| count.as_u64() == Some(expected_tasks / 4))
        && actual["primary_checkout_counts"] == json!([1, 1, 1, 1])
        && actual["live_sessions"] == 8;
    if !correct {
        return Err(format!(
            "actual pre-T0 fixture differs from pinned envelope: expected={expected}; measured={actual}"
        ));
    }
    Ok(())
}
