use super::*;
use serde_json::json;

#[test]
fn folding_moves_winning_payloads_without_copying_nested_values() {
    let row = json!({
        "project_id": "p",
        "project": "Project",
        "agents": [{"surfaces": {"goal": "a large retained goal ".repeat(128)}}],
    });
    let expected = row.clone();
    let original = row["agents"][0]["surfaces"]["goal"]
        .as_str()
        .unwrap()
        .as_ptr();
    let candidate = WorkItemCandidate {
        kind: WorkItemKind::Branch,
        key: WorkItemKey::Branch {
            project_id: "p".into(),
            branch: "feature".into(),
        },
        source: Some(BranchSource::Run),
        task_id: None,
        implementation_active: true,
        implementation_merged: false,
        row,
    };
    let rows = fold_work_items(vec![candidate]);
    assert_eq!(rows, vec![expected]);
    assert_eq!(
        rows[0]["agents"][0]["surfaces"]["goal"]
            .as_str()
            .unwrap()
            .as_ptr(),
        original,
        "the fold owns its winners and must move their nested payloads"
    );
}
