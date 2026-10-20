use super::*;
use crate::app::read_view::SnapshotDelta;

fn fixture() -> PushFacts {
    let mut facts = PushFacts::default();
    let mut workspaces = Vec::new();
    for (owner, thread, generation, sequence) in [
        ("selected", "thread", 0, 1),
        ("equal", "thread", 0, 8),
        ("newer", "thread", 0, 9),
        ("reset", "thread", 1, 1),
        ("unrelated", "other-thread", 0, 1),
    ] {
        let row = json!({
            "owner": owner,
            "body": format!("{owner} immutable payload ").repeat(128),
            "thread": {"thread_id": thread, "thread_generation_revision": generation, "last_sequence": sequence}
        });
        facts.states.insert(owner.into(), Arc::new(row.clone()));
        facts.terminals.insert(
            owner.into(),
            Arc::new(json!({"tabs": [], "body": format!("{owner} terminal payload ").repeat(128)})),
        );
        workspaces.push(row);
    }
    facts.lists = Arc::new(
        json!({"projects": [{"project_id": "project"}], "workspaces": workspaces, "usage_limits": {}}),
    );
    facts
}

fn body_pointer(value: &Value) -> *const u8 {
    value["body"]
        .as_str()
        .expect("nonempty heap payload")
        .as_ptr()
}

fn committed() -> crate::store::CommittedThreadItems {
    crate::store::CommittedThreadItems {
        conversation_id: "selected".into(),
        thread_id: "thread".into(),
        generation_revision: 0,
        sequence: 8,
        items: Vec::new(),
    }
}

#[test]
fn push_facts_clone_shares_immutable_state_payloads() {
    let before = fixture();
    let after = before.clone();
    for (owner, value) in &before.states {
        assert_eq!(
            body_pointer(value),
            body_pointer(&after.states[owner]),
            "state payload copied for {owner}"
        );
    }
}

#[test]
fn push_facts_clone_shares_immutable_terminal_payloads() {
    let before = fixture();
    let after = before.clone();
    for (owner, value) in &before.terminals {
        assert_eq!(
            body_pointer(value),
            body_pointer(&after.terminals[owner]),
            "terminal payload copied for {owner}"
        );
    }
}

#[test]
fn push_facts_clone_shares_immutable_list_payloads() {
    let before = fixture();
    let after = before.clone();
    for (old, new) in before.lists["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .zip(after.lists["workspaces"].as_array().unwrap())
    {
        assert_eq!(
            body_pointer(old),
            body_pointer(new),
            "workspace list payload copied"
        );
    }
}

#[test]
fn push_facts_receipt_copies_only_older_matching_digests() {
    let before = fixture();
    let mut after = before.clone();
    after.apply_committed_history_delta(&committed());
    assert_ne!(
        body_pointer(&before.states["selected"]),
        body_pointer(&after.states["selected"])
    );
    assert_eq!(after.states["selected"]["thread"]["last_sequence"], 8);
    for owner in ["equal", "newer", "reset", "unrelated"] {
        assert_eq!(
            body_pointer(&before.states[owner]),
            body_pointer(&after.states[owner]),
            "unchanged digest copied for {owner}"
        );
    }
    for (owner, value) in &before.terminals {
        assert_eq!(body_pointer(value), body_pointer(&after.terminals[owner]));
    }
    assert_ne!(
        body_pointer(&before.lists["workspaces"][0]),
        body_pointer(&after.lists["workspaces"][0])
    );
    let repeated = after.clone();
    after.apply_committed_history_delta(&committed());
    for (owner, value) in &repeated.states {
        assert_eq!(
            body_pointer(value),
            body_pointer(&after.states[owner]),
            "repeated receipt copied {owner}"
        );
    }
    assert_eq!(
        body_pointer(&repeated.lists["workspaces"][0]),
        body_pointer(&after.lists["workspaces"][0]),
        "repeated receipt copied lists"
    );
}

#[test]
fn push_facts_receipt_preserves_old_root_and_exact_reader_results() {
    let source = ReadViewSlot::default();
    let before = fixture();
    let state = before.fact(&FactsRequest {
        entity_id: "selected".into(),
        state: true,
        terminals: true,
        ..Default::default()
    });
    let lists = before.fact(&FactsRequest {
        entity_id: crate::changes::BOARD_ITEM_ID.into(),
        lists: BoardLists::WORKSPACES,
        ..Default::default()
    });
    source.publish_delta(SnapshotDelta::new(source.next_version()).set_facts(Arc::new(before)));
    let old_root = source.load();
    let mut candidate = old_root.facts.as_ref().clone();
    candidate.apply_committed_history_delta(&committed());
    source.publish_delta(SnapshotDelta::new(source.next_version()).set_facts(Arc::new(candidate)));
    let next_root = source.load();
    let request = FactsRequest {
        entity_id: "selected".into(),
        state: true,
        terminals: true,
        ..Default::default()
    };
    assert_eq!(old_root.facts.fact(&request), state);
    let mut expected = state.clone();
    expected.state.as_mut().unwrap()["thread"]["last_sequence"] = json!(8);
    assert_eq!(next_root.facts.fact(&request), expected);
    let request = FactsRequest {
        entity_id: crate::changes::BOARD_ITEM_ID.into(),
        lists: BoardLists::WORKSPACES,
        ..Default::default()
    };
    assert_eq!(old_root.facts.fact(&request), lists);
    let mut expected = lists;
    expected.state.as_mut().unwrap()["workspaces"][0]["thread"]["last_sequence"] = json!(8);
    assert_eq!(next_root.facts.fact(&request), expected);
}

#[test]
fn push_facts_digest_predicate_matches_nested_and_malformed_counter_outputs() {
    let cases = [
        (
            json!({"thread_id":"thread","thread_generation_revision":0,"last_sequence":1,
                "nested":[{"thread_id":"thread","thread_generation_revision":0,"last_sequence":3},
                    {"thread_id":"thread","thread_generation_revision":1,"last_sequence":1}]}),
            json!({"thread_id":"thread","thread_generation_revision":0,"last_sequence":8,
                "nested":[{"thread_id":"thread","thread_generation_revision":0,"last_sequence":8},
                    {"thread_id":"thread","thread_generation_revision":1,"last_sequence":1}]}),
        ),
        (
            json!({"thread_id":"other","thread_generation_revision":0,"last_sequence":1,
                "nested":[null,{"deep":{"thread_id":"thread","thread_generation_revision":0,"last_sequence":2}}]}),
            json!({"thread_id":"other","thread_generation_revision":0,"last_sequence":1,
                "nested":[null,{"deep":{"thread_id":"thread","thread_generation_revision":0,"last_sequence":8}}]}),
        ),
        (
            json!([
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":null},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":"1"},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":1.5},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":-1},
                {"thread_id":"thread","thread_generation_revision":"0","last_sequence":1},
                {"thread_id":"thread","thread_generation_revision":0.5,"last_sequence":1},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":8},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":9},
                {"thread_id":"thread","thread_generation_revision":0}
            ]),
            json!([
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":null},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":"1"},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":1.5},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":-1},
                {"thread_id":"thread","thread_generation_revision":"0","last_sequence":1},
                {"thread_id":"thread","thread_generation_revision":0.5,"last_sequence":1},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":8},
                {"thread_id":"thread","thread_generation_revision":0,"last_sequence":9},
                {"thread_id":"thread","thread_generation_revision":0}
            ]),
        ),
    ];
    let delta = committed();
    for (before, expected) in cases {
        let should_change = before != expected;
        assert_eq!(
            crate::app::read_view::history::thread_digests_need_patch(&before, &delta),
            should_change
        );
        let mut after = before;
        assert_eq!(
            crate::app::read_view::history::patch_thread_digests(&mut after, &delta),
            should_change
        );
        assert_eq!(after, expected);
    }
}
