use super::status::git_gui_state;
use super::*;

// ==== bodies read in pages (#95) ===========================================
//
// A patch too large to carry whole is read a page at a time: `range` names
// the byte offset to start at and the most one page may carry, and the
// answer's `range` says where the page sits. Read on from each `end`, the
// pages are the patch, and every page but the last ends a line.

const PAGE: u64 = 65_536;

/// A patch far over one page: `lines` added lines of a new file.
fn big_file(lines: usize) -> String {
    (0..lines)
        .map(|line| format!("added line {line:07}\n"))
        .collect()
}

/// Every page of a body, read on from each `end` through `read`, joined; and
/// how many pages it took.
fn read_all_pages(mut read: impl FnMut(u64) -> Value, field: &str) -> (String, usize) {
    let mut offset = 0u64;
    let mut joined = String::new();
    let mut pages = 0;
    loop {
        let answer = read(offset);
        let page = answer[field]
            .as_str()
            .unwrap_or_else(|| panic!("{answer:?}"));
        let range = &answer["range"];
        assert_eq!(range["offset"], offset, "{range:?}");
        assert_eq!(range["end"], offset + page.len() as u64, "{range:?}");
        assert!(page.len() as u64 <= PAGE);
        joined.push_str(page);
        pages += 1;
        offset = range["end"].as_u64().unwrap();
        if offset == range["total"].as_u64().unwrap() {
            return (joined, pages);
        }
        assert!(
            page.ends_with('\n'),
            "a page that is not the last ends a line"
        );
    }
}

#[test]
fn git_diff_answers_one_path_in_pages() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("big.txt"), big_file(70_000)).unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();

    let whole = state.handle(req(
        "git.diff",
        json!({ "project_id": project_id.clone(), "paths": ["big.txt"] }),
    ));
    let whole_file = &whole["result"]["files"][0];
    assert_eq!(whole_file["truncated"], true, "the whole read is capped");

    let (joined, pages) = read_all_pages(
        |offset| {
            let res = state.handle(req(
                "git.diff",
                json!({
                    "project_id": project_id.clone(),
                    "paths": ["big.txt"],
                    "range": { "offset": offset, "bytes": PAGE },
                }),
            ));
            assert_eq!(res["ok"], true, "{res:?}");
            let file = res["result"]["files"][0].clone();
            assert_eq!(file["truncated"], false);
            assert!(file["range"]["version"].is_string(), "{file:?}");
            assert_eq!(file["content_key"], whole_file["content_key"]);
            file
        },
        "patch",
    );
    assert!(pages > 10, "{pages} pages");
    assert!(joined.starts_with("diff --git a/big.txt b/big.txt"));
    assert!(joined.ends_with("+added line 0069999\n"));
    assert!(
        joined.len() > 1_048_576,
        "the pages carry past the whole-read cap"
    );
}

#[test]
fn git_diff_takes_a_range_for_one_path_only() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let res = state.handle(req(
        "git.diff",
        json!({
            "project_id": project_id,
            "paths": ["a.txt", "b.txt"],
            "range": { "offset": 0, "bytes": PAGE },
        }),
    ));
    assert_eq!(res["ok"], false, "{res:?}");
    assert!(
        res["error"].as_str().unwrap().contains("one path"),
        "{res:?}"
    );
}

#[test]
fn git_show_answers_a_commit_patch_in_pages() {
    let (dir, repo) = init_repo();
    std::fs::write(repo.join("big.txt"), big_file(60_000)).unwrap();
    git_in(&repo, &["add", "big.txt"]);
    git_in(&repo, &["commit", "-m", "big"]);
    let mut state = git_gui_state(&dir, &repo);
    let project_id = state.project_at(0).id.clone();
    let log = state.handle(req(
        "git.log",
        json!({ "project_id": project_id.clone(), "limit": 1 }),
    ));
    let hash = log["result"]["commits"][0]["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let mut patch_bytes = 0;
    let (joined, _) = read_all_pages(
        |offset| {
            let res = state.handle(req(
                "git.show",
                json!({
                    "project_id": project_id.clone(),
                    "hash": hash.clone(),
                    "range": { "offset": offset, "bytes": PAGE },
                }),
            ));
            assert_eq!(res["ok"], true, "{res:?}");
            let result = res["result"].clone();
            assert_eq!(result["truncated"], false);
            assert_eq!(result["stat"]["insertions"], 60_000);
            patch_bytes = result["patch_bytes"].as_u64().unwrap();
            assert_eq!(result["range"]["total"], patch_bytes);
            result
        },
        "patch",
    );
    assert_eq!(joined.len() as u64, patch_bytes);
    assert!(joined.ends_with("+added line 0059999\n"));

    let both = state.handle(req(
        "git.show",
        json!({
            "project_id": project_id,
            "hash": hash,
            "max_bytes": 4096,
            "range": { "offset": 0, "bytes": PAGE },
        }),
    ));
    assert_eq!(
        both["ok"], false,
        "range and max_bytes are two answers: {both:?}"
    );
}

#[test]
fn git_changeset_diff_answers_one_path_in_pages() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = _state.lock().unwrap().project_at(0).id.clone();
    std::fs::write(repo.join("big.txt"), big_file(30_000)).unwrap();
    std::fs::write(repo.join("small.txt"), "small\n").unwrap();

    let (joined, pages) = read_all_pages(
        |offset| {
            let res = call(
                &handler,
                "git.changeset_diff",
                json!({
                    "project_id": project_id.clone(),
                    "paths": ["big.txt"],
                    "range": { "offset": offset, "bytes": PAGE },
                }),
            );
            assert_eq!(res["ok"], true, "{res:?}");
            let result = res["result"].clone();
            assert!(result["range"].is_object(), "{result:?}");
            assert_eq!(result["files"][0]["path"], "big.txt");
            result
        },
        "patch",
    );
    assert!(pages > 5, "{pages} pages");
    assert!(joined.starts_with("diff --git a/big.txt b/big.txt"));
    assert!(!joined.contains("small.txt"));
    assert!(joined.ends_with("+added line 0029999\n"));

    let two = call(
        &handler,
        "git.changeset_diff",
        json!({
            "project_id": project_id,
            "paths": ["big.txt", "small.txt"],
            "range": { "offset": 0, "bytes": PAGE },
        }),
    );
    assert_eq!(two["ok"], false, "{two:?}");
}
