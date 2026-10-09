use super::*;
use serde_json::{json, Value};

fn facts() -> (Review, ReviewMergeIntent) {
    let original = json!({
        "id":"original","snapshot_id":"snapshot","directory_id":"directory","source_name":"source",
        "source_path":"/repo/one","actor":{"kind":"user"},"started_at":"1","status":"interrupted",
        "steps":[
            {"kind":"merge","branch":"main","status":"succeeded","input_head":"reviewed-head","result_head":"merged-head"},
            {"kind":"push","branch":"main","remote":"origin","status":"interrupted","input_head":"merged-head"}
        ]
    });
    let retry = json!({
        "id":"retry","snapshot_id":"snapshot","directory_id":"directory","source_name":"source",
        "source_path":"/repo/one","actor":{"kind":"user"},"started_at":"2","status":"succeeded",
        "steps":[{"kind":"push","branch":"main","remote":"origin","merge_action_id":"original",
            "status":"succeeded","input_head":"merged-head","result_head":"merged-head"}]
    });
    let review = serde_json::from_value(json!({
        "task_id":"task","workspace_id":"workspace","version":1,"state":"open","snapshots":[],
        "completion":null,"actions":[original,retry]
    }))
    .unwrap();
    let intent = serde_json::from_value(json!({
        "project_path":"/project","request_id":"intent","version":1,"state":"interrupted",
        "created_at":"1","updated_at":"2","action_ids":["original","retry"],
        "request":{"task_id":"task","expected_version":1,"snapshot_id":"snapshot","actor":{"kind":"user"},
            "sources":[{"directory_id":"directory","repository_id":"repository","head":"reviewed-head",
                "base_branch_ref":"refs/heads/main","expected_base_head":"base-head",
                "push":{"remote":"origin","branch":"main"}}]}
    })).unwrap();
    (review, intent)
}

#[test]
fn a_later_linked_success_settles_an_inline_interrupted_push() {
    let (review, intent) = facts();
    assert!(!unresolved_actions(&review, &intent));
    assert!(
        uncertain_action(&review.actions[0]),
        "refresh admission stays conservative"
    );
}

#[test]
fn a_later_linked_success_settles_an_interrupted_push_only_retry() {
    let (mut review, mut intent) = facts();
    let mut merge = review.actions[0].clone();
    merge.id = "recorded-merge".into();
    merge.status = ActionStatus::Succeeded;
    merge.steps.truncate(1);
    review.actions[0].steps.remove(0);
    review.actions[0].steps[0].merge_action_id = Some(merge.id.clone());
    review.actions[1].steps[0].merge_action_id = Some(merge.id.clone());
    intent.action_ids.insert(0, merge.id.clone());
    review.actions.insert(0, merge);
    assert!(!unresolved_actions(&review, &intent));
}

#[test]
fn success_on_another_obligation_does_not_settle_an_interrupted_push() {
    let (review, intent) = facts();
    for (pointer, value) in [
        ("/actions/1/directory_id", json!("other-source")),
        ("/actions/1/source_path", json!("/repo/two")),
        ("/actions/1/snapshot_id", json!("other-snapshot")),
        ("/actions/1/steps/0/branch", json!("release")),
        ("/actions/1/steps/0/remote", json!("elsewhere")),
        ("/actions/1/steps/0/input_head", json!("wrong-head")),
        ("/actions/1/steps/0/result_head", json!("wrong-head")),
        ("/actions/1/steps/0/merge_action_id", json!("other-merge")),
        ("/actions/1/steps/0/status", json!("failed")),
    ] {
        let mut changed = serde_json::to_value(&review).unwrap();
        *changed.pointer_mut(pointer).unwrap() = value;
        let changed = serde_json::from_value(changed).unwrap();
        assert!(unresolved_actions(&changed, &intent), "{pointer}");
    }
}

#[test]
fn success_from_another_intent_or_before_interruption_does_not_settle_it() {
    let (review, mut intent) = facts();
    intent.action_ids.pop();
    assert!(unresolved_actions(&review, &intent));
    intent.action_ids.push("retry".into());
    intent.action_ids.reverse();
    assert!(unresolved_actions(&review, &intent));
    intent.action_ids.reverse();
    intent.action_ids.push("missing-action".into());
    assert!(unresolved_actions(&review, &intent));
}

#[test]
fn settling_a_push_does_not_make_following_pending_work_known_unstarted() {
    let (mut review, intent) = facts();
    let mut pending = review.actions[0].steps[1].clone();
    pending.status = StepStatus::Pending;
    pending.input_head = None;
    review.actions[0].steps.push(pending);
    assert!(unresolved_actions(&review, &intent));
}

#[test]
fn running_or_unknown_outcomes_remain_unresolved() {
    let (review, intent) = facts();
    for (pointer, value) in [
        ("/actions/0/status", json!("running")),
        ("/actions/0/steps/1/status", json!("running")),
        ("/actions/0/steps/1/status", json!("pending")),
        ("/actions/0/steps/0/status", json!("interrupted")),
        ("/actions/0/steps/0/result_head", Value::Null),
        ("/actions/0/steps/1/input_head", Value::Null),
        ("/actions/1/steps/0/result_head", Value::Null),
        ("/actions/0/steps", json!([])),
    ] {
        let mut changed = serde_json::to_value(&review).unwrap();
        *changed.pointer_mut(pointer).unwrap() = value;
        let changed = serde_json::from_value(changed).unwrap();
        assert!(unresolved_actions(&changed, &intent), "{pointer}");
    }
}
