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
    let path = app.tracker_project_path(&project).unwrap();
    seed_summary_tasks(root.path(), &path, 1, 105);
    let summary = answer(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"summary":true}),
    );
    assert_eq!(summary["tasks"].as_array().unwrap().len(), 100);
    assert_eq!(summary["summary"], true);
    assert!(summary["next_cursor"].is_string());
    assert!(summary["identities"].is_object());
    for row in summary["tasks"].as_array().unwrap() {
        assert_lightweight_row(row, &project);
    }
    let legacy = answer(&mut app, "tasks.list", json!({"project_id":project}));
    assert_eq!(legacy["tasks"].as_array().unwrap().len(), 105);
    assert!(legacy["tasks"][0]["body"].is_string());
    assert!(legacy.get("summary").is_none());
    assert!(legacy.get("identities").is_none());
}

fn assert_lightweight_row(row: &Value, project: &str) {
    for omitted in ["body", "attachments", "timeline", "identities"] {
        assert!(row.get(omitted).is_none(), "{row}");
    }
    assert!(row["links"].get("commits").is_none());
    assert!(row["dependencies"]["blocked"].is_boolean());
    assert!(row["dependencies"].get("waiting_on").is_none());
    assert!(row["dependencies"].get("blocks").is_none());
    assert_eq!(row["project_id"], project);
}

#[test]
fn summary_filters_sparse_pages_and_equivalent_cursors() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let first = filed(&mut app, &project, "older API");
    answer(
        &mut app,
        "tasks.update",
        json!({"task_id":first["id"],"labels":["API"],"status":"in_review"}),
    );
    for number in 0..12 {
        filed(&mut app, &project, &format!("Other {number}"));
    }
    let mut params = json!({"project_id":project,"summary":true,"limit":1,"states":["closed","open","open"],"statuses":["In review","done"],"labels":["api","API"]});
    let first_page = answer(&mut app, "tasks.list", params.clone());
    assert_eq!(first_page["tasks"], json!([]));
    assert!(first_page["next_cursor"].is_string());
    params["cursor"] = first_page["next_cursor"].clone();
    params["states"] = json!(["open", "closed"]);
    params["statuses"] = json!(["done", "in_review"]);
    params["labels"] = json!(["Api"]);
    let mut found = Vec::new();
    for _ in 0..10 {
        let page = answer(&mut app, "tasks.list", params.clone());
        found.extend(
            page["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].clone()),
        );
        let Some(cursor) = page.get("next_cursor") else {
            break;
        };
        assert_ne!(params["cursor"], *cursor);
        params["cursor"] = cursor.clone();
    }
    assert_eq!(found, vec![first["id"].clone()]);
    let mut changed = params.clone();
    changed["labels"] = json!(["other"]);
    assert_eq!(app.handle(req("tasks.list", changed))["ok"], false);
}

#[test]
fn summary_invalid_filters_limits_and_cursor_mode_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for filter in [
        json!({"states":[]}),
        json!({"state":"open","states":["open"]}),
        json!({"state":null,"states":["open"]}),
        json!({"statuses":["closed"]}),
        json!({"labels":[""]}),
        json!({"assignees":[]}),
        json!({"limit":0}),
        json!({"limit":501}),
        json!({"cursor":"invalid"}),
    ] {
        let mut params = json!({"project_id":project,"summary":true});
        params
            .as_object_mut()
            .unwrap()
            .extend(filter.as_object().unwrap().clone());
        assert_eq!(
            app.handle(req("tasks.list", params.clone()))["ok"],
            false,
            "{params}"
        );
    }
}

#[test]
fn filter_choices_seek_unique_values_independently_of_task_filters() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    for label in ["API", "api", "UI"] {
        let task = filed(&mut app, &project, label);
        answer(
            &mut app,
            "tasks.update",
            json!({"task_id":task["id"],"labels":[label]}),
        );
    }
    let first = answer(
        &mut app,
        "tasks.filterChoices",
        json!({"project_id":project,"dimension":"label","limit":1}),
    );
    assert_eq!(first["choices"].as_array().unwrap().len(), 1);
    let second = answer(
        &mut app,
        "tasks.filterChoices",
        json!({"project_id":project,"dimension":"label","limit":1,"cursor":first["next_cursor"]}),
    );
    assert_eq!(second["choices"], json!(["UI"]));
    assert!(second.get("next_cursor").is_none());
    for dimension in ["state", "status"] {
        let choices = answer(
            &mut app,
            "tasks.filterChoices",
            json!({"project_id":project,"dimension":dimension}),
        );
        assert!(choices["choices"].as_array().unwrap().len() >= 2);
    }
}

#[test]
fn bounded_summary_cursors_cannot_cross_projects_stores_or_modes() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    filed(&mut app, &project, "one");
    filed(&mut app, &project, "two");
    let first = answer(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"summary":true,"limit":1}),
    );
    let cursor = first["next_cursor"].clone();
    let (other_repo, other_path) = init_repo();
    let other_project = super::project_agent::added_project(&mut app, &other_path);
    for params in [
        json!({"project_id":other_project,"summary":true,"cursor":cursor}),
        json!({"project_id":project,"limit":1,"cursor":cursor}),
    ] {
        assert_eq!(app.handle(req("tasks.list", params))["ok"], false);
    }
    let second_root = tempfile::tempdir().unwrap();
    let (_repo2, mut other, project2) = tracked(second_root.path());
    assert_eq!(
        other.handle(req(
            "tasks.list",
            json!({"project_id":project2,"summary":true,"cursor":cursor})
        ))["ok"],
        false
    );
    assert_eq!(
        app.handle(req(
            "tasks.filterChoices",
            json!({"project_id":project,"dimension":"label","cursor":cursor})
        ))["ok"],
        false
    );
    drop(other_repo);
}

fn seed_summary_tasks(root: &Path, project: &str, from: usize, through: usize) {
    let mut connection = rusqlite::Connection::open(root.join("store/build.db")).unwrap();
    let tx = connection.transaction().unwrap();
    let mut insert=tx.prepare("INSERT INTO tracker_tasks(id,project_key,number,state,status,created_at,updated_at,record) VALUES(?1,?2,?3,'open','backlog',?4,?4,?5)").unwrap();
    for number in from..=through {
        let mut task = crate::tracker::Task::drafted(
            project,
            &format!("Task {number}"),
            crate::tracker::Actor::User,
            "2026-10-01T00:00:00Z",
        );
        task.number = number as u64;
        task.labels = vec!["duplicate".into(), format!("label-{:03}", number % 130)];
        task.body = "body omitted from every summary".repeat(200);
        insert
            .execute(rusqlite::params![
                task.id,
                project,
                task.number as i64,
                task.created_at,
                serde_json::to_string(&task).unwrap()
            ])
            .unwrap();
    }
    drop(insert);
    tx.commit().unwrap();
}

#[test]
fn byte_limited_summary_pages_consume_each_task_exactly_once() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let path = app.tracker_project_path(&project).unwrap();
    seed_summary_tasks(root.path(), &path, 1, 75);
    let connection = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    connection
        .execute(
            "UPDATE tracker_task_summaries SET record=json_set(record,'$.title',?1)",
            ["large display".repeat(1800)],
        )
        .unwrap();
    let mut params = json!({"project_id":project,"summary":true,"limit":500});
    let mut ids = std::collections::BTreeSet::new();
    let mut page_count = 0;
    loop {
        page_count += 1;
        assert!(page_count < 10, "a bounded byte cap must make progress");
        let page = answer(&mut app, "tasks.list", params.clone());
        assert!(serde_json::to_vec(&page).unwrap().len() <= 1_048_576);
        for task in page["tasks"].as_array().unwrap() {
            assert!(
                ids.insert(task["id"].as_str().unwrap().to_string()),
                "a byte-limited boundary must not repeat a task"
            );
        }
        let Some(cursor) = page.get("next_cursor") else {
            break;
        };
        assert_ne!(params.get("cursor"), Some(cursor));
        params["cursor"] = cursor.clone();
    }
    assert!(page_count > 1, "byte cap must be exercised");
    assert_eq!(
        ids.len(),
        75,
        "no task may be silently consumed at a byte boundary"
    );
}

#[test]
fn oversized_summary_errors_without_publishing_a_partial_page_or_cursor() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let path = app.tracker_project_path(&project).unwrap();
    seed_summary_tasks(root.path(), &path, 1, 3);
    let connection = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    let id: String = connection
        .query_row(
            "SELECT task_id FROM tracker_task_summaries WHERE number=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Simulate an externally imported corrupt projection beyond its normal CHECK.
    connection
        .execute_batch("PRAGMA ignore_check_constraints=ON")
        .unwrap();
    connection.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.title',?1) WHERE task_id=?2",rusqlite::params!["x".repeat(70_000),id]).unwrap();
    let params = json!({"project_id":project,"summary":true,"limit":3});
    for _ in 0..2 {
        let error = app.handle(req("tasks.list", params.clone()));
        assert_eq!(error["ok"], false, "{error}");
        assert_eq!(error["error_code"], "unavailable", "{error}");
        assert_eq!(error["retryable"], false);
        assert!(error["error"]
            .as_str()
            .unwrap()
            .contains("task summary exceeds 65536 bytes"));
        assert!(
            error.get("result").is_none(),
            "no partial rows or success continuation on an error"
        );
        assert!(error.get("next_cursor").is_none());
    }
    answer(
        &mut app,
        "tasks.update",
        json!({"task_id":id,"title":"repaired"}),
    );
    let repaired = answer(&mut app, "tasks.list", params);
    assert_eq!(repaired["tasks"].as_array().unwrap().len(), 3);
    assert!(repaired["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == id));
}

#[test]
fn invalid_metadata_summary_fails_without_success_cursor_then_recovers() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let path = app.tracker_project_path(&project).unwrap();
    seed_summary_tasks(root.path(), &path, 1, 3);
    let connection = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    let id: String = connection
        .query_row(
            "SELECT task_id FROM tracker_task_summaries WHERE number=2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let imported = json!({"mode":"pull_request","pull_request":{"originating_workspace_id":"x".repeat(129),"status":"open"}});
    connection
        .execute(
            "INSERT INTO reviews(task_id,workspace_id,version,record) VALUES(?1,'workspace',1,?2)",
            rusqlite::params![id, imported.to_string()],
        )
        .unwrap();
    let params = json!({"project_id":project,"summary":true,"limit":3});
    for _ in 0..2 {
        let error = app.handle(req("tasks.list", params.clone()));
        assert_eq!(error["ok"], false, "{error}");
        assert_eq!(error["error_code"], "unavailable");
        assert_eq!(error["retryable"], false);
        assert!(error.get("result").is_none());
        assert!(error.get("next_cursor").is_none());
    }
    connection
        .execute("DELETE FROM reviews WHERE task_id=?1", [&id])
        .unwrap();
    let repaired = answer(&mut app, "tasks.list", params);
    assert_eq!(repaired["tasks"].as_array().unwrap().len(), 3);
    assert!(repaired["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == id));
    assert!(repaired.get("next_cursor").is_none());
}

#[test]
fn server_multi_selections_match_tagged_assignees_without_case_folding_ids() {
    let root = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(root.path());
    let path = app.tracker_project_path(&project).unwrap();
    seed_summary_tasks(root.path(), &path, 1, 3);
    let connection = rusqlite::Connection::open(root.path().join("store/build.db")).unwrap();
    for (number, assignee, status, labels) in [
        (1, json!({"kind":"user"}), "in_review", json!(["API"])),
        (
            2,
            json!({"kind":"agent","agent_id":"AgentA"}),
            "done",
            json!(["ui"]),
        ),
        (
            3,
            json!({"kind":"agent","agent_id":"agenta"}),
            "backlog",
            json!(["API"]),
        ),
    ] {
        connection.execute("UPDATE tracker_tasks SET status=?1,record=json_set(record,'$.status',?1,'$.assignee',json(?2),'$.labels',json(?3)) WHERE number=?4",rusqlite::params![status,assignee.to_string(),labels.to_string(),number]).unwrap();
    }
    let params = json!({"project_id":project,"summary":true,"states":["open","closed"],"statuses":["In review","done"],"labels":["api","UI"],"assignees":[{"kind":"user"},{"kind":"agent","agent_id":"AgentA"}]});
    let result = answer(&mut app, "tasks.list", params);
    let numbers: Vec<u64> = result["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, vec![2, 1]);
    assert!(result["identities"]["AgentA"].is_object());
    assert!(result["identities"].get("agenta").is_none());
    let exact = answer(
        &mut app,
        "tasks.list",
        json!({"project_id":project,"summary":true,"assignee":{"kind":"agent","agent_id":"agenta"}}),
    );
    assert_eq!(exact["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(exact["tasks"][0]["number"], 3);
}
