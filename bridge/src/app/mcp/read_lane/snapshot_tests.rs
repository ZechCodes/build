use super::*;
use crate::app::snapshot_projection::work_counts::detail_scans;

fn split(value: &Value) -> SharedJson {
    match value {
        Value::Object(fields) => SharedJson::object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), split(value))),
        ),
        Value::Array(values) => SharedJson::array(values.iter().map(split)),
        value => SharedJson::value(value.clone()),
    }
}

#[test]
fn compact_work_projection_selects_only_digest_fields_for_every_node_shape() {
    let rows = json!([
        {"kind":"run","run_id":"","task_id":"task-fallback","project":"Project","working":true},
        {"kind":"task","run_id":null,"task_id":"task-1","worktree_id":"wt-1","state":"open"},
        {"kind":"worktree","run_id":42,"task_id":false,"worktree_id":"wt-2"},
        {"kind":"capture","run_id":"never-visible"},
        {"kind":"unknown","run_id":false,"task_id":17},
        {}
    ]);
    let mut board = json!({"items":rows,"ignored":vec!["large unused payload".repeat(100);100]});
    for row in board["items"].as_array_mut().unwrap() {
        row["unused"] = json!(vec!["unused row content".repeat(100); 100]);
    }
    let expected = router_work_digest(board.clone());
    let mixed_rows = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if index % 2 == 0 {
                SharedJson::value(value.clone())
            } else {
                split(value)
            }
        });
    let shapes = [
        SharedJson::value(board.clone()),
        split(&board),
        SharedJson::object([("items".into(), SharedJson::value(board["items"].clone()))]),
        SharedJson::object([("items".into(), SharedJson::array(mixed_rows))]),
    ];
    for shape in shapes {
        let (answer, count) = detail_scans::measure(|| snapshot_value(&shape, true));
        assert_eq!(answer, expected);
        assert!(
            count.materialized_nodes <= 8 * 5,
            "compact work read materialized unselected board/row content: {count:?}"
        );
    }
    assert_eq!(expected["work"][0]["entity_id"], "");
    assert_eq!(expected["work"][1]["entity_id"], "task-1");
    assert_eq!(expected["work"][2]["entity_id"], "wt-2");
    assert!(expected["work"][3]["entity_id"].is_null());
}
