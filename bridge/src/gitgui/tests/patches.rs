// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn a_files_patch_is_answered_for_every_asked_path_in_order() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("new.txt"), "a\nb\n").unwrap();
    std::fs::write(repo.join("README.md"), "# project\nedit\n").unwrap();

    let answer = file_patches(&repo, &["new.txt".to_string(), "README.md".to_string()]).unwrap();
    let files = answer["files"].as_array().unwrap();

    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "new.txt");
    assert!(files[0]["patch"].as_str().unwrap().contains("+a"));
    assert_eq!(files[0]["truncated"], false);
    assert_eq!(files[1]["path"], "README.md");
    assert!(files[1]["patch"].as_str().unwrap().contains("+edit"));

    let status = status_payload(&repo).unwrap();
    let readme = status["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "README.md")
        .unwrap();
    assert_eq!(
        files[1]["content_key"], readme["content_key"],
        "one file, one key on both verbs"
    );
}
/// `git.diff` answers every path it is asked for, in order, so a path it
/// cannot read is an error rather than a silently dropped answer.
#[test]
fn a_path_git_diff_cannot_read_is_an_error_not_a_gap() {
    let (_dir, repo) = crate::git_fixture::init_repo();

    assert!(file_patches(&repo, &["../evil".to_string()])
        .unwrap_err()
        .contains("escapes the worktree"));
    assert!(file_patches(&repo, &[".build/mcp.json".to_string()])
        .unwrap_err()
        .contains("not readable through git.diff"));
    assert!(file_patches(&repo, &[]).is_err());
    let too_many: Vec<String> = (0..=GIT_DIFF_MAX_PATHS)
        .map(|n| format!("f{n}.txt"))
        .collect();
    assert!(file_patches(&repo, &too_many).is_err());
}
#[test]
fn a_patch_past_the_cap_is_truncated_and_flagged() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    std::fs::write(repo.join("big.txt"), "line\n".repeat(500)).unwrap();

    let answer = file_patches_capped(
        &repo,
        &["big.txt".to_string()],
        200,
        GIT_DIFF_MAX_ANSWER_BYTES,
    )
    .unwrap();
    let file = &answer["files"][0];

    assert_eq!(file["truncated"], true);
    assert!(file["patch"].as_str().unwrap().len() <= 200);
}
/// The per-file cap is not the whole story: 50 files at 1 MiB each would be
/// 50 MiB, eight times what a relay frame holds. One budget is spent in
/// request order, and the files past it answer empty and flagged.
#[test]
fn one_answer_budget_is_spent_across_the_files_in_request_order() {
    let (_dir, repo) = crate::git_fixture::init_repo();
    let paths: Vec<String> = (0..GIT_DIFF_MAX_PATHS)
        .map(|n| format!("f{n:02}.txt"))
        .collect();
    for path in &paths {
        std::fs::write(repo.join(path), "line\n".repeat(200)).unwrap();
    }

    let answer = file_patches_capped(&repo, &paths, 200, 1_000).unwrap();
    let files = answer["files"].as_array().unwrap();

    assert_eq!(files.len(), GIT_DIFF_MAX_PATHS);
    let spent: usize = files
        .iter()
        .map(|file| file["patch"].as_str().unwrap().len())
        .sum();
    assert!(spent <= 1_000, "answer spent {spent} bytes");
    assert_eq!(files[0]["truncated"], true);
    assert_eq!(files[0]["patch"].as_str().unwrap().len(), 200);
    let tail = files.last().unwrap();
    assert_eq!(tail["patch"], "", "the budget was gone by the tail");
    assert_eq!(tail["truncated"], true);
    assert_eq!(
        tail["content_key"].as_str().unwrap().len(),
        16,
        "a flagged-empty file still carries its key"
    );

    // The real caps: every path answers whole, inside the answer budget.
    let whole = file_patches(&repo, &paths).unwrap();
    let whole_files = whole["files"].as_array().unwrap();
    let whole_spent: usize = whole_files
        .iter()
        .map(|file| file["patch"].as_str().unwrap().len())
        .sum();
    assert!(whole_spent < GIT_DIFF_MAX_ANSWER_BYTES);
    assert!(whole_files
        .iter()
        .all(|file| file["truncated"] == false && !file["patch"].as_str().unwrap().is_empty()));
}
