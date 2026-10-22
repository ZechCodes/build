use super::*;
use crate::app::snapshot_projection::summary_counts;

#[test]
fn workspace_summary_join_visits_each_summary_once() {
    for size in [8, 64] {
        let rows = (0..size)
            .map(|i| json!({"workspace_id": format!("workspace-{i}"), "name": i}))
            .collect::<Vec<_>>();
        let summaries = (0..size)
            .rev()
            .map(|i| json!({"workspace_id": format!("workspace-{i}"), "work_summary": {"files": i}, "can_finish": i % 2 == 0, "finish_blockers": []}))
            .collect::<Vec<_>>();
        let expected = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let mut row = row.clone();
                row["work_summary"] = json!({"files": i});
                row["can_finish"] = json!(i % 2 == 0);
                row["finish_blockers"] = json!([]);
                row
            })
            .collect::<Vec<_>>();
        let (actual, counts) = summary_counts::measure(|| {
            snapshot_push_workspace_rows(&json!(rows), &json!(summaries))
        });
        assert_eq!(actual, json!(expected));
        eprintln!("{size} workspace summary joins: {counts:?}");
        assert!(counts.joins <= size, "repeated summary search: {counts:?}");
    }
}

#[test]
fn workspace_summary_join_preserves_first_duplicates_missing_fields_and_value_ids() {
    let rows = json!([
        {"workspace_id": "duplicate", "name": "first"},
        {"workspace_id": "absent", "can_finish": "retained"},
        {"name": "missing id"},
        {"workspace_id": 7},
        {"workspace_id": {"legacy": true}}
    ]);
    let summaries = json!([
        {"workspace_id": "duplicate", "work_summary": {"winner": 1}, "can_finish": true, "finish_blockers": []},
        {"workspace_id": "duplicate", "work_summary": {"winner": 2}},
        {"work_summary": "missing matches null"},
        {"workspace_id": 7, "can_finish": false},
        {"workspace_id": {"legacy": true}, "finish_blockers": ["legacy"]}
    ]);
    assert_eq!(
        snapshot_push_workspace_rows(&rows, &summaries),
        json!([
            {"workspace_id": "duplicate", "name": "first", "work_summary": {"winner": 1}, "can_finish": true, "finish_blockers": []},
            {"workspace_id": "absent", "can_finish": "retained"},
            {"name": "missing id", "work_summary": "missing matches null", "can_finish": null, "finish_blockers": null},
            {"workspace_id": 7, "work_summary": null, "can_finish": false, "finish_blockers": null},
            {"workspace_id": {"legacy": true}, "work_summary": null, "can_finish": null, "finish_blockers": ["legacy"]}
        ])
    );
    assert_eq!(
        snapshot_push_workspace_rows(&Value::Null, &summaries),
        json!([])
    );
    assert_eq!(snapshot_push_workspace_rows(&rows, &Value::Null), rows);
}
