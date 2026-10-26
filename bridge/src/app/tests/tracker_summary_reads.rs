//! The opt-in list must progress with bounded records and complete filters.
use super::tracker::{filed, tracked};
use super::*;

fn answer(app: &mut AppState, method: &str, params: Value) -> Value {
    let response = app.handle(req(method, params));
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

#[test]
fn summary_list_defaults_and_legacy_shape_are_independent() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for number in 0..105 { filed(&mut app, &project, &format!("Task {number}")); }
    let summary = answer(&mut app, "tasks.list", json!({"project_id":project,"summary":true}));
    assert_eq!(summary["tasks"].as_array().unwrap().len(),100);
    assert_eq!(summary["summary"],true);
    assert!(summary["next_cursor"].is_string());
    assert!(summary["identities"].is_object());
    for row in summary["tasks"].as_array().unwrap() {
        for omitted in ["body","attachments","timeline","dependencies","identities"] { assert!(row.get(omitted).is_none(),"{row}"); }
        assert!(row["links"].get("commits").is_none());
        assert_eq!(row["project_id"],project);
    }
    let legacy = answer(&mut app,"tasks.list",json!({"project_id":project}));
    assert_eq!(legacy["tasks"].as_array().unwrap().len(),105);
    assert!(legacy["tasks"][0]["body"].is_string());
    assert!(legacy.get("summary").is_none());
    assert!(legacy.get("identities").is_none());
}

#[test]
fn summary_filters_sparse_pages_and_equivalent_cursors() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let first = filed(&mut app,&project,"older API");
    answer(&mut app,"tasks.update",json!({"task_id":first["id"],"labels":["API"],"status":"in_review"}));
    for number in 0..12 { filed(&mut app,&project,&format!("Other {number}")); }
    let mut params = json!({"project_id":project,"summary":true,"limit":1,"states":["closed","open","open"],"statuses":["In review","done"],"labels":["api","API"]});
    let first_page = answer(&mut app,"tasks.list",params.clone());
    assert_eq!(first_page["tasks"],json!([]));
    assert!(first_page["next_cursor"].is_string());
    params["cursor"] = first_page["next_cursor"].clone();
    params["states"] = json!(["open","closed"]);
    params["statuses"] = json!(["done","in_review"]);
    params["labels"] = json!(["Api"]);
    let mut found = Vec::new();
    for _ in 0..10 {
        let page = answer(&mut app,"tasks.list",params.clone());
        found.extend(page["tasks"].as_array().unwrap().iter().map(|row|row["id"].clone()));
        let Some(cursor) = page.get("next_cursor") else { break };
        assert_ne!(params["cursor"], *cursor);
        params["cursor"] = cursor.clone();
    }
    assert_eq!(found,vec![first["id"].clone()]);
    let mut changed = params.clone();
    changed["labels"] = json!(["other"]);
    assert_eq!(app.handle(req("tasks.list",changed))["ok"],false);
}

#[test]
fn summary_invalid_filters_limits_and_cursor_mode_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for filter in [json!({"states":[]}),json!({"state":"open","states":["open"]}),json!({"state":null,"states":["open"]}),json!({"statuses":["closed"]}),json!({"labels":[""]}),json!({"assignees":[]}),json!({"limit":0}),json!({"limit":501}),json!({"cursor":"invalid"})] {
        let mut params = json!({"project_id":project,"summary":true});
        params.as_object_mut().unwrap().extend(filter.as_object().unwrap().clone());
        assert_eq!(app.handle(req("tasks.list",params.clone()))["ok"],false,"{params}");
    }
}

#[test]
fn filter_choices_seek_unique_values_independently_of_task_filters() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for label in ["API","api","UI"] {
        let task = filed(&mut app,&project,label);
        answer(&mut app,"tasks.update",json!({"task_id":task["id"],"labels":[label]}));
    }
    let first = answer(&mut app,"tasks.filterChoices",json!({"project_id":project,"dimension":"label","limit":1}));
    assert_eq!(first["choices"].as_array().unwrap().len(),1);
    let second = answer(&mut app,"tasks.filterChoices",json!({"project_id":project,"dimension":"label","limit":1,"cursor":first["next_cursor"]}));
    assert_eq!(second["choices"],json!(["UI"]));
    assert!(second.get("next_cursor").is_none());
    for dimension in ["state","status"] {
        let choices = answer(&mut app,"tasks.filterChoices",json!({"project_id":project,"dimension":dimension}));
        assert!(choices["choices"].as_array().unwrap().len() >= 2);
    }
}
