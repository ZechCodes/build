//! `issues.list` a page at a time (#85): `limit` bounds a page, `next_cursor`
//! names where the next one starts, and a cursor is good only for the filter
//! it was made under.
//!
//! The order is the list's own, number descending, and a cursor is the last
//! number a page answered. A number never moves, so an issue filed while a
//! client is paging lands above the first page and never shifts one below it.

use super::tracker::{filed, refused, tracked};
use super::*;

/// A project holding `count` issues, numbered 1 to `count`, and their ids in
/// the order `issues.list` answers them: newest first.
fn project_of(state: &mut AppState, project_id: &str, count: usize) -> Vec<String> {
    let mut ids: Vec<String> = (1..=count)
        .map(|n| {
            filed(state, project_id, &format!("issue {n}"))["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    ids.reverse();
    ids
}

fn listed(state: &mut AppState, params: Value) -> Value {
    let answered = state.handle(req("issues.list", params));
    assert_eq!(answered["ok"], true, "{answered:?}");
    answered["result"].clone()
}

fn ids_of(page: &Value) -> Vec<String> {
    page["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["id"].as_str().unwrap().to_string())
        .collect()
}

fn numbers_of(page: &Value) -> Vec<u64> {
    page["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|issue| issue["number"].as_u64().unwrap())
        .collect()
}

/// Every page of one filter, asked the way a client walks them: the first
/// without a cursor, each next with the cursor the one before answered.
fn every_page(state: &mut AppState, filter: Value, limit: u64) -> Vec<Value> {
    let mut pages = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params = filter.clone();
        params["limit"] = json!(limit);
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let page = listed(state, params);
        cursor = page
            .get("next_cursor")
            .map(|next| next.as_str().expect("a cursor is a string").to_string());
        pages.push(page);
        if cursor.is_none() {
            return pages;
        }
    }
}

#[test]
fn a_list_asked_for_no_limit_is_the_whole_list_and_names_no_next_page() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ids = project_of(&mut state, &project_id, 7);

    let whole = listed(&mut state, json!({ "project_id": project_id }));

    assert_eq!(ids_of(&whole), ids);
    assert!(whole.get("next_cursor").is_none(), "{whole:?}");
}

#[test]
fn a_page_holds_at_most_its_limit_and_names_the_next_only_while_there_is_one() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ids = project_of(&mut state, &project_id, 7);

    let pages = every_page(&mut state, json!({ "project_id": project_id }), 3);

    let numbers: Vec<Vec<u64>> = pages.iter().map(numbers_of).collect();
    assert_eq!(numbers, vec![vec![7, 6, 5], vec![4, 3, 2], vec![1]]);
    let walked: Vec<String> = pages.iter().flat_map(ids_of).collect();
    assert_eq!(walked, ids, "the pages, end to end, are the list");
    let whole = listed(&mut state, json!({ "project_id": project_id }));
    let rows: Vec<&Value> = pages
        .iter()
        .flat_map(|page| page["issues"].as_array().unwrap())
        .collect();
    let whole_rows: Vec<&Value> = whole["issues"].as_array().unwrap().iter().collect();
    assert_eq!(
        rows, whole_rows,
        "a paged row is the row the whole list answers"
    );
    for page in &pages {
        assert!(page["user_session"].is_object(), "{page:?}");
        assert_eq!(page["project_id"], project_id.as_str());
    }
}

#[test]
fn a_limit_that_fills_the_list_exactly_names_no_next_page() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    project_of(&mut state, &project_id, 4);

    let exact = listed(&mut state, json!({ "project_id": project_id, "limit": 4 }));
    let pages = every_page(&mut state, json!({ "project_id": project_id }), 2);

    assert_eq!(numbers_of(&exact), vec![4, 3, 2, 1]);
    assert!(exact.get("next_cursor").is_none(), "{exact:?}");
    assert_eq!(pages.len(), 2, "no empty third page: {pages:?}");
}

#[test]
fn a_limit_is_one_to_five_hundred() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    project_of(&mut state, &project_id, 2);

    for limit in [1, 500] {
        let page = listed(
            &mut state,
            json!({ "project_id": project_id, "limit": limit }),
        );
        assert_eq!(page["issues"].as_array().unwrap().len(), limit.min(2));
    }
    for limit in [json!(0), json!(501), json!(u64::MAX)] {
        let answered = state.handle(req(
            "issues.list",
            json!({ "project_id": project_id, "limit": limit }),
        ));
        assert_eq!(answered["ok"], false, "{limit}: {answered:?}");
        assert_eq!(answered["error_code"], "invalid_params", "{answered:?}");
        assert_eq!(
            answered["error"],
            format!("Build cannot list {limit} issues at a time: a page holds 1 to 500."),
        );
    }
    for limit in [json!(-1), json!(2.5), json!("10")] {
        let answered = state.handle(req(
            "issues.list",
            json!({ "project_id": project_id, "limit": limit }),
        ));
        assert_eq!(
            answered["error_code"], "invalid_params",
            "{limit}: {answered:?}"
        );
    }
}

#[test]
fn a_cursor_continues_the_filter_it_was_made_under() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ids = project_of(&mut state, &project_id, 9);
    for (index, id) in ids.iter().enumerate() {
        if index % 2 == 0 {
            let labelled = state.handle(req(
                "issues.update",
                json!({ "issue_id": id, "labels": ["sweep"] }),
            ));
            assert_eq!(labelled["ok"], true, "{labelled:?}");
        }
    }
    let filter = json!({ "project_id": project_id, "label": "sweep" });

    let pages = every_page(&mut state, filter.clone(), 2);

    let numbers: Vec<Vec<u64>> = pages.iter().map(numbers_of).collect();
    assert_eq!(numbers, vec![vec![9, 7], vec![5, 3], vec![1]]);
    let whole = listed(&mut state, filter);
    let walked: Vec<String> = pages.iter().flat_map(ids_of).collect();
    assert_eq!(walked, ids_of(&whole));
}

#[test]
fn a_cursor_made_under_another_filter_is_refused_in_a_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    project_of(&mut state, &project_id, 5);
    let open = listed(
        &mut state,
        json!({ "project_id": project_id, "state": "open", "limit": 2 }),
    );
    let cursor = open["next_cursor"].as_str().unwrap().to_string();

    for other in [
        json!({ "project_id": project_id }),
        json!({ "project_id": project_id, "state": "closed" }),
        json!({ "project_id": project_id, "state": "open", "label": "bug" }),
        json!({ "project_id": project_id, "state": "open", "assignee": "none" }),
        json!({ "project_id": project_id, "state": "open", "status": "backlog" }),
    ] {
        let mut params = other.clone();
        params["cursor"] = json!(cursor);
        let answered = state.handle(req("issues.list", params));
        assert_eq!(
            answered["error_code"], "invalid_params",
            "{other}: {answered:?}"
        );
        assert_eq!(
            answered["error"],
            "Build cannot continue this list: the cursor was made for a different filter.",
            "{other}"
        );
    }
    let same = listed(
        &mut state,
        json!({ "project_id": project_id, "state": "open", "cursor": cursor, "limit": 2 }),
    );
    assert_eq!(numbers_of(&same), vec![3, 2]);
}

/// A column named the way it is shown and the way it is stored is one filter,
/// and so is a label spelled in another case: the cursor is made over what
/// the filter means, not how it was typed.
#[test]
fn a_cursor_carries_over_to_the_same_filter_spelled_another_way() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    for id in project_of(&mut state, &project_id, 4) {
        let moved = state.handle(req(
            "issues.update",
            json!({ "issue_id": id, "status": "in_review", "labels": ["Bug"] }),
        ));
        assert_eq!(moved["ok"], true, "{moved:?}");
    }
    let first = listed(
        &mut state,
        json!({ "project_id": project_id, "status": "In review", "label": "BUG", "limit": 2 }),
    );

    let next = listed(
        &mut state,
        json!({
            "project_id": project_id, "status": "in_review", "label": "bug",
            "cursor": first["next_cursor"], "limit": 2,
        }),
    );

    assert_eq!(numbers_of(&next), vec![2, 1]);
}

#[test]
fn a_cursor_this_bridge_did_not_make_is_refused_in_a_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    project_of(&mut state, &project_id, 2);

    for cursor in ["", "not a cursor", "MTIz", "djE6eDp5"] {
        let answered = state.handle(req(
            "issues.list",
            json!({ "project_id": project_id, "cursor": cursor }),
        ));
        assert_eq!(
            answered["error_code"], "invalid_params",
            "{cursor}: {answered:?}"
        );
        assert_eq!(
            answered["error"],
            "Build cannot read this cursor: ask for the list again from the start.",
            "{cursor}"
        );
    }
}

/// Another project's cursor is another filter: the project is part of what
/// a list is narrowed to.
#[test]
fn a_cursor_from_another_project_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    project_of(&mut state, &project_id, 3);
    let (_other_home, other_repo) = init_repo();
    let other_repo = std::fs::canonicalize(&other_repo).unwrap();
    let other = super::project_agent::added_project(&mut state, &other_repo);
    project_of(&mut state, &other, 3);
    let first = listed(&mut state, json!({ "project_id": project_id, "limit": 1 }));

    let error = refused(
        &mut state,
        "issues.list",
        json!({ "project_id": other, "cursor": first["next_cursor"] }),
    );

    assert_eq!(
        error,
        "Build cannot continue this list: the cursor was made for a different filter."
    );
}

/// Issues filed while a client pages land above the first page, where the
/// next full read finds them; the page after the cursor is exactly the rows
/// that were below it. Nothing is answered twice and nothing is skipped.
#[test]
fn issues_filed_between_pages_move_no_row_across_a_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ids = project_of(&mut state, &project_id, 6);
    let first = listed(&mut state, json!({ "project_id": project_id, "limit": 2 }));
    let cursor = first["next_cursor"].as_str().unwrap().to_string();

    filed(&mut state, &project_id, "filed while paging");
    filed(&mut state, &project_id, "filed while paging, again");
    let mut walked = ids_of(&first);
    let mut cursor = Some(cursor);
    while let Some(next) = cursor.take() {
        let page = listed(
            &mut state,
            json!({ "project_id": project_id, "limit": 2, "cursor": next }),
        );
        walked.extend(ids_of(&page));
        cursor = page
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    assert_eq!(
        walked, ids,
        "the walk is the list as it stood when it began"
    );
    let whole = listed(&mut state, json!({ "project_id": project_id }));
    assert_eq!(numbers_of(&whole)[..2], [8, 7]);
}

/// An issue that leaves the filter between pages leaves that page; the rest
/// of the page is what it would have been.
#[test]
fn an_issue_that_leaves_the_filter_between_pages_is_not_answered_on_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let ids = project_of(&mut state, &project_id, 6);
    let filter = json!({ "project_id": project_id, "state": "open", "limit": 2 });
    let first = listed(&mut state, filter.clone());
    let closed = state.handle(req("issues.close", json!({ "issue_id": ids[3] })));
    assert_eq!(closed["ok"], true, "{closed:?}");

    let mut params = filter;
    params["cursor"] = first["next_cursor"].clone();
    let second = listed(&mut state, params);

    assert_eq!(numbers_of(&second), vec![4, 2]);
}
