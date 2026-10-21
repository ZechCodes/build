use super::*;
use crate::app::read_view::SharedJson;

fn value_nodes(value: &Value) -> usize {
    1 + match value {
        Value::Object(fields) => fields.values().map(value_nodes).sum(),
        Value::Array(values) => values.iter().map(value_nodes).sum(),
        _ => 0,
    }
}

fn mixed(value: Value, depth: usize) -> SharedJson {
    if depth.is_multiple_of(3) {
        return SharedJson::value(value);
    }
    match value {
        Value::Object(fields) => SharedJson::object(
            fields
                .into_iter()
                .map(|(key, value)| (key, mixed(value, depth + 1))),
        ),
        Value::Array(values) => {
            SharedJson::array(values.into_iter().map(|value| mixed(value, depth + 1)))
        }
        value => SharedJson::value(value),
    }
}

fn field(row: &SharedJson, name: &str) -> SharedJson {
    row.field(name)
        .unwrap_or_else(|| panic!("{name} must retain its independently shared payload"))
}

fn assert_payload_sharing(view: &ReadView, workspace: &str, variants: &[String]) {
    let base = view.get_shared("workspace.get", workspace).unwrap();
    let agents = field(base, "agents");
    let run = field(base, "run");
    assert!(
        agents.ptr_eq(&field(&run, "agents")),
        "outer and run agent payloads were copied"
    );
    assert!(field(base, "thread").ptr_eq(&field(&run, "thread")));
    for agent in variants {
        let key = format!("{workspace}/{agent}");
        let variant = view.get_shared("workspace.get", &key).unwrap();
        assert!(
            agents.ptr_eq(&field(variant, "agents")),
            "variant copied shared agents: {key}"
        );
        let run = field(variant, "run");
        assert!(agents.ptr_eq(&field(&run, "agents")));
        assert!(field(variant, "thread").ptr_eq(&field(&run, "thread")));
    }
}

#[test]
fn workspace_detail_variants_index_payload_once_as_rosters_grow() {
    let mut observations = Vec::new();
    for roster_size in [4, 12] {
        let (_tmp, mut state, _) = ownership_fixture(2, false);
        add_tip_agents(&mut state, roster_size);
        warm_detail_fixture(&mut state);
        let old = state.read_view.load();
        let retained = detail_rows(&old);
        let payload_nodes = value_nodes(&retained["workspace-00"]);
        let primary = agent_id(&state, "run-00", 0);
        state
            .runs
            .get_mut("run-00")
            .unwrap()
            .agents
            .by_id_mut(&primary)
            .unwrap()
            .topic = Some("fresh selected workspace payload".into());
        let _ownership = state.begin_snapshot_ownership();
        let (delta, count) = detail_scans::measure(|| {
            state.prepare_workspace_snapshot_details(
                SnapshotDelta::new(state.read_view.next_version()),
                &old,
                Some("run-00"),
            )
        });
        state.publish_prepared_snapshot_projection(delta);
        drop(_ownership);
        let actual = state.read_view.load();
        let mut expected = retained.clone();
        for (key, value) in &mut expected {
            if key == "workspace-00" || key.starts_with("workspace-00/") {
                value["agents"][0]["topic"] = json!("fresh selected workspace payload");
                value["run"]["agents"][0]["topic"] = json!("fresh selected workspace payload");
            }
        }
        assert_eq!(
            detail_rows(&actual),
            expected,
            "complete row output changed"
        );
        assert_eq!(detail_rows(&old), retained);
        assert_detail_full_oracle(&state, &actual);
        assert_eq!(count.base_renders, 1);
        assert_eq!(count.variant_renders, roster_size);
        eprintln!(
            "{roster_size} agents: detail index visits={} base payload nodes={payload_nodes}",
            count.indexed_values
        );
        observations.push((roster_size, payload_nodes, count));
    }
    for (roster_size, payload_nodes, count) in observations {
        assert!(
            count.indexed_values > 0,
            "actual digest indexing was not observed"
        );
        assert!(count.indexed_values <= 2 * payload_nodes,
            "{roster_size} variants repeatedly indexed the same payload: {count:?}, base={payload_nodes}");
    }
}

#[test]
fn workspace_detail_base_variants_and_receipts_share_immutable_payloads() {
    let (tmp, mut state, canonical, alias) = alias_detail_fixture();
    state.store = Some(crate::store::Store::new(tmp.path().join("tasks")).unwrap());
    let mut active = state.take_run("run-00").unwrap();
    active
        .agents
        .by_id_mut(&canonical)
        .unwrap()
        .thread
        .post_user("receipt-bearing user message", None, "2026-01-01T00:00:01Z");
    state.finish_run_mutation("run-00".into(), active).unwrap();
    // Persist all canonical/alias rosters before the native receipt mutation.
    for owner in state.runs.keys().cloned().collect::<Vec<_>>() {
        let active = state.take_run(&owner).unwrap();
        state.finish_run_mutation(owner, active).unwrap();
    }
    state.publish_all_snapshot_projection();
    let old = state.read_view.load();
    let retained = detail_rows(&old);
    let variants = state.runs["run-00"]
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect::<Vec<_>>();
    assert_payload_sharing(&old, "run-00", &variants);
    let untouched_key = format!("run-00/{}", variants[1]);
    let untouched_thread = field(
        old.get_shared("workspace.get", &untouched_key).unwrap(),
        "thread",
    );
    let committed = state
        .store
        .as_ref()
        .unwrap()
        .set_delivery_status(
            &canonical,
            None,
            2,
            2,
            crate::thread::MessageDeliveryStatus::Sent,
        )
        .unwrap();
    assert!(
        committed.sequence > 2,
        "native receipt must advance the counter"
    );
    state.apply_committed_thread_items(&committed);
    let patched = state.read_view.load();
    assert_payload_sharing(&patched, "run-00", &variants);
    assert!(untouched_thread.ptr_eq(&field(
        patched.get_shared("workspace.get", &untouched_key).unwrap(),
        "thread"
    )));
    for key in [format!("run-00/{canonical}"), format!("run-01/{alias}")] {
        let row = patched.get_owned("workspace.get", &key).unwrap();
        assert_eq!(row["thread"]["last_sequence"], committed.sequence);
        assert_eq!(row["run"]["thread"]["last_sequence"], committed.sequence);
    }
    assert_eq!(detail_rows(&old), retained);
    state.publish_owner_snapshot_projection("run-07");
    let actual = state.read_view.load();
    assert_eq!(
        actual
            .get_owned("workspace.get", &format!("run-01/{alias}"))
            .unwrap()["thread"]["last_sequence"],
        committed.sequence
    );
    assert_detail_full_oracle(&state, &actual);
    assert_eq!(detail_rows(&old), retained);
}

#[test]
fn workspace_detail_scan_handles_mixed_rows_without_materializing_payloads() {
    let (_tmp, mut state, canonical, alias) = alias_detail_fixture();
    let old = state.read_view.load();
    let mut delta = SnapshotDelta::new(state.read_view.next_version());
    // Mix leaf roots, composed roots, nested leaves and composed arrays. The
    // last two retained-data cases exercise null/malformed optional fields.
    for (number, (key, value)) in detail_rows(&old).into_iter().enumerate() {
        delta = delta.put_shared("workspace.get", key, mixed(value, number % 3));
    }
    let cases = [
        ("old-direct", json!({"entity_id":"run-00"}), true),
        (
            "old-thread",
            json!({"entity_id":null,"thread":{"agent":{"id":canonical}}}),
            true,
        ),
        (
            "old-secondary",
            json!({"agents":[false,{"conversation_id":null},{"conversation_id":canonical}]}),
            true,
        ),
        (
            "old-unrelated",
            json!({"entity_id":12,"thread":null,"agents":[false,{"conversation_id":77}]}),
            false,
        ),
        ("old-missing", json!({}), false),
    ];
    for (number, (key, value, _)) in cases.iter().enumerate() {
        delta = delta.put_shared("workspace.get", key, mixed(value.clone(), 1 + number % 2));
        delta = delta.put(
            "workspace.get",
            format!("{key}/retired"),
            json!({"sentinel":true}),
        );
    }
    state.publish_prepared_snapshot_projection(delta);
    let previous = state.read_view.load();
    let retained = detail_rows(&previous);
    state
        .runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .remove(&canonical)
        .unwrap();
    state
        .runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&alias)
        .unwrap()
        .bind_conversation(&alias);
    state.workspaces.forget("removed-workspace");
    let _ownership = state.begin_snapshot_ownership();
    let (delta, count) = detail_scans::measure(|| {
        state.prepare_workspace_snapshot_details(
            SnapshotDelta::new(state.read_view.next_version()),
            &previous,
            Some("run-00"),
        )
    });
    state.publish_prepared_snapshot_projection(delta);
    drop(_ownership);
    let actual = state.read_view.load();
    for (key, _, affected) in cases {
        assert_eq!(
            actual.get_shared("workspace.get", key).is_none(),
            affected,
            "{key}"
        );
        assert_eq!(
            actual
                .get_shared("workspace.get", &format!("{key}/retired"))
                .is_none(),
            affected,
            "{key} variant"
        );
    }
    assert!(actual
        .get_shared("workspace.get", &format!("run-00/{canonical}"))
        .is_none());
    assert!(actual
        .get_shared("workspace.get", "removed-workspace")
        .is_none());
    assert_eq!(
        actual
            .get_owned("workspace.get", &format!("run-01/{alias}"))
            .unwrap()["thread"]["agent"]["id"],
        alias
    );
    assert_eq!(detail_rows(&previous), retained);
    assert_eq!(count.registry_scans, 1);
    assert_eq!(
        count.materialized_nodes, 0,
        "dependency selection materialized composed rows: {count:?}"
    );
}
