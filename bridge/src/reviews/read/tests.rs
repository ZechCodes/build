use super::*;
use crate::git_fixture::{git_command, git_in, init_repo};
use crate::reviews::model::{ReviewBase, ReviewBaseKind, ReviewDirectoryStatus};

fn saved_directory(repo: &Path) -> ReviewDirectory {
    let base = String::from_utf8(
        git_command(repo, &["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    git_in(repo, &["checkout", "-b", "feature"]);
    std::fs::write(repo.join("new.txt"), "saved content\n").unwrap();
    git_in(repo, &["add", "new.txt"]);
    git_in(repo, &["commit", "-m", "saved"]);
    let head = String::from_utf8(
        git_command(repo, &["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let common_git_dir = git2::Repository::open(repo)
        .unwrap()
        .commondir()
        .canonicalize()
        .unwrap();
    ReviewDirectory {
        id: "directory-1".into(),
        source_id: "source-1".into(),
        name: "repo".into(),
        path: repo.into(),
        source_path: repo.into(),
        is_git: true,
        status: ReviewDirectoryStatus::Git,
        reason: None,
        common_git_dir: Some(common_git_dir),
        branch: Some("feature".into()),
        base: Some(ReviewBase {
            kind: ReviewBaseKind::Configured,
            name: Some("main".into()),
            oid: base,
        }),
        head: Some(head),
        uncommitted_files: Some(0),
    }
}

fn request(mode: ReviewReadMode, path: Option<&str>) -> ReviewReadRequest {
    ReviewReadRequest {
        mode,
        path: path.map(str::to_owned),
        paths: Vec::new(),
        range: None,
        patch: true,
        commit: None,
        from_snapshot_id: None,
        limit: None,
        cursor: None,
    }
}

#[test]
fn full_saved_tree_and_blob_ignore_live_checkout_edits() {
    let (_temp, repo) = init_repo();
    let directory = saved_directory(&repo);
    std::fs::write(repo.join("new.txt"), "live edit\n").unwrap();
    std::fs::remove_file(repo.join("README.md")).unwrap();
    let ReviewReadResult::Tree(tree) =
        read(&directory, &request(ReviewReadMode::Tree, None)).unwrap()
    else {
        panic!("tree expected")
    };
    assert_eq!(
        tree.entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["new.txt", "README.md"]
    );
    let ReviewReadResult::Blob(blob) = read(
        &directory,
        &request(ReviewReadMode::Blob, Some("README.md")),
    )
    .unwrap() else {
        panic!("blob expected")
    };
    assert_eq!(blob.content_b64, crate::encoding::b64encode(b"# project\n"));
    assert!(!blob.editable);
    let ReviewReadResult::Changes(changes) =
        read(&directory, &request(ReviewReadMode::Changes, None)).unwrap()
    else {
        panic!("changes expected")
    };
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "new.txt");
    assert!(changes.patch.unwrap().contains("+saved content"));
}

#[test]
fn blob_pages_name_the_saved_blob_and_reject_traversal() {
    let (_temp, repo) = init_repo();
    let directory = saved_directory(&repo);
    let mut blob_request = request(ReviewReadMode::Blob, Some("new.txt"));
    blob_request.range = Some(FileRange {
        offset: 0,
        bytes: 4096,
        raw: None,
    });
    let ReviewReadResult::Blob(blob) = read(&directory, &blob_request).unwrap() else {
        panic!("blob expected")
    };
    let expected_blob = String::from_utf8(
        git_command(&repo, &["rev-parse", "HEAD:new.txt"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(blob.range.unwrap().version.unwrap(), expected_blob.trim());
    blob_request.range.as_mut().unwrap().raw = Some(true);
    assert!(read(&directory, &blob_request).is_err());
    assert!(read(
        &directory,
        &request(ReviewReadMode::Blob, Some("../README.md"))
    )
    .is_err());
}

#[test]
fn saved_objects_remain_readable_after_linked_checkout_is_removed() {
    let (temp, repo) = init_repo();
    let linked = temp.path().join("linked");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "review-checkout",
            linked.to_str().unwrap(),
        ],
    );
    let directory = saved_directory(&linked);
    git_in(
        &repo,
        &["worktree", "remove", "--force", linked.to_str().unwrap()],
    );
    let ReviewReadResult::Blob(blob) =
        read(&directory, &request(ReviewReadMode::Blob, Some("new.txt"))).unwrap()
    else {
        panic!("blob expected")
    };
    assert_eq!(
        blob.content_b64,
        crate::encoding::b64encode(b"saved content\n")
    );
}

#[test]
fn deleted_repository_and_unavailable_record_return_source_unavailable() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    directory.status = ReviewDirectoryStatus::Unavailable;
    assert_eq!(
        read(&directory, &request(ReviewReadMode::Tree, None)).unwrap_err(),
        "Source unavailable"
    );
    directory.status = ReviewDirectoryStatus::Git;
    std::fs::remove_dir_all(&repo).unwrap();
    assert_eq!(
        read(&directory, &request(ReviewReadMode::Tree, None)).unwrap_err(),
        "Source unavailable"
    );
}

#[test]
fn paged_blob_reads_join_exactly_and_keep_one_oid_version() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    let body = (0..300)
        .map(|line| format!("{line:04} {}\n", "x".repeat(35)))
        .collect::<String>();
    std::fs::write(repo.join("large.txt"), &body).unwrap();
    git_in(&repo, &["add", "large.txt"]);
    git_in(&repo, &["commit", "-m", "large"]);
    directory.head = Some(
        String::from_utf8(
            git_command(&repo, &["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into(),
    );
    let mut offset = 0;
    let mut collected = Vec::new();
    let mut version = None;
    let mut pages = 0;
    loop {
        let mut page_request = request(ReviewReadMode::Blob, Some("large.txt"));
        page_request.range = Some(FileRange {
            offset,
            bytes: 4096,
            raw: None,
        });
        let ReviewReadResult::Blob(page) = read(&directory, &page_request).unwrap() else {
            panic!("blob expected")
        };
        let span = page.range.unwrap();
        assert!(version
            .as_ref()
            .is_none_or(|oid| Some(oid) == span.version.as_ref()));
        version = span.version;
        collected.extend(crate::encoding::b64decode(&page.content_b64).unwrap());
        pages += 1;
        offset = span.end;
        if offset == span.total {
            break;
        }
    }
    assert!(pages >= 3);
    assert_eq!(collected, body.as_bytes());
}

#[test]
fn large_patch_has_a_bounded_first_read_and_readable_later_pages() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    let body = "line of new committed text repeated many times\n".repeat(30_000);
    std::fs::write(repo.join("large.txt"), body).unwrap();
    git_in(&repo, &["add", "large.txt"]);
    git_in(&repo, &["commit", "-m", "large patch"]);
    directory.head = Some(
        String::from_utf8(
            git_command(&repo, &["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into(),
    );
    let mut first_request = request(ReviewReadMode::Changes, None);
    first_request.paths = vec!["large.txt".into()];
    let ReviewReadResult::Changes(first) = read(&directory, &first_request).unwrap() else {
        panic!("changes expected")
    };
    let patch = first.patch.unwrap();
    assert!(first.truncated);
    assert!(!first.files_truncated);
    assert!(patch.len() <= crate::body_page::BODY_PAGE_MAX_BYTES as usize);
    let mut later = request(ReviewReadMode::Changes, None);
    later.paths = vec!["large.txt".into()];
    later.range = Some(FileRange {
        offset: patch.len() as u64,
        bytes: 4096,
        raw: None,
    });
    let ReviewReadResult::Changes(page) = read(&directory, &later).unwrap() else {
        panic!("changes expected")
    };
    let span = page.range.unwrap();
    assert!(span.end > span.offset);
    assert!(span.total > patch.len() as u64);
    assert!(!page.truncated);
    let base = directory.base.as_ref().unwrap().oid.as_str();
    let head = directory.head.as_deref().unwrap();
    let paths = vec!["large.txt".into()];
    let full = crate::diff::diff_between_saved_commits(
        directory.common_git_dir.as_deref().unwrap(),
        Some(base),
        head,
        crate::diff::DiffPaths::Only(&paths),
    )
    .unwrap();
    let (expected, expected_span) =
        crate::body_page::text_page(full.patch(), later.range.unwrap().body());
    assert_eq!(page.patch.as_deref(), Some(expected.as_str()));
    assert_eq!(span, expected_span);
    let bounded = crate::diff::diff_between_saved_commits_bounded(
        directory.common_git_dir.as_deref().unwrap(),
        Some(base),
        head,
        crate::diff::DiffPaths::Only(&paths),
        true,
        Some(later.range.unwrap().body()),
    )
    .unwrap();
    assert!(bounded.buffered_patch_bytes <= crate::body_page::BODY_PAGE_MAX_BYTES as usize + 3);
}

#[test]
fn many_changed_paths_cap_rows_without_losing_the_total() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    for index in 0..1_205 {
        std::fs::write(repo.join(format!("file-{index:04}.txt")), "one line\n").unwrap();
    }
    git_in(&repo, &["add", "."]);
    assert!(git_command(&repo, &["commit", "-m", "many paths"])
        .output()
        .unwrap()
        .status
        .success());
    directory.head = Some(
        String::from_utf8(
            git_command(&repo, &["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into(),
    );
    let mut listing = request(ReviewReadMode::Changes, None);
    listing.patch = false;
    let ReviewReadResult::Changes(changes) = read(&directory, &listing).unwrap() else {
        panic!("changes expected")
    };
    assert_eq!(changes.stat.files_changed, 1_206);
    assert_eq!(changes.files.len(), 1_000);
    assert!(changes.files_truncated);
    assert!(changes.patch.is_none());
    listing.paths = vec!["file-1204.txt".into()];
    let ReviewReadResult::Changes(single) = read(&directory, &listing).unwrap() else {
        panic!("changes expected")
    };
    assert_eq!(single.files.len(), 1);
    assert_eq!(single.files[0].path, "file-1204.txt");
    assert!(!single.files_truncated);
    let base = directory.base.as_ref().unwrap().oid.as_str();
    let head = directory.head.as_deref().unwrap();
    let bounded = crate::diff::diff_between_saved_commits_bounded(
        directory.common_git_dir.as_deref().unwrap(),
        Some(base),
        head,
        crate::diff::DiffPaths::All,
        false,
        None,
    )
    .unwrap();
    assert_eq!(bounded.buffered_patch_bytes, 0);
}

#[test]
fn invalid_options_are_refused_before_opening_git() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    directory.common_git_dir = None;
    let mut bad = request(ReviewReadMode::Blob, None);
    assert!(read(&directory, &bad)
        .unwrap_err()
        .starts_with("invalid review read:"));
    let traversal = request(ReviewReadMode::Blob, Some("../outside"));
    assert!(read(&directory, &traversal)
        .unwrap_err()
        .starts_with("invalid review read:"));
    bad.mode = ReviewReadMode::Tree;
    bad.range = Some(FileRange {
        offset: 0,
        bytes: 4096,
        raw: None,
    });
    assert!(read(&directory, &bad)
        .unwrap_err()
        .starts_with("invalid review read:"));
    bad.mode = ReviewReadMode::Changes;
    bad.path = None;
    bad.range = None;
    bad.paths = vec!["README.md".into(); 101];
    assert!(read(&directory, &bad)
        .unwrap_err()
        .starts_with("invalid review read:"));
}

#[cfg(unix)]
#[test]
fn symlinks_and_gitlinks_are_labelled_and_not_opened() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    std::os::unix::fs::symlink("README.md", repo.join("link")).unwrap();
    git_in(&repo, &["add", "link"]);
    let oid = String::from_utf8(
        git_command(&repo, &["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let cacheinfo = format!("160000,{},nested", oid.trim());
    git_in(&repo, &["update-index", "--add", "--cacheinfo", &cacheinfo]);
    git_in(&repo, &["commit", "-m", "special entries"]);
    directory.head = Some(
        String::from_utf8(
            git_command(&repo, &["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into(),
    );
    let ReviewReadResult::Tree(tree) =
        read(&directory, &request(ReviewReadMode::Tree, None)).unwrap()
    else {
        panic!("tree expected")
    };
    assert_eq!(
        tree.entries
            .iter()
            .find(|entry| entry.name == "link")
            .unwrap()
            .kind,
        "symlink"
    );
    assert_eq!(
        tree.entries
            .iter()
            .find(|entry| entry.name == "nested")
            .unwrap()
            .kind,
        "submodule"
    );
    assert!(read(&directory, &request(ReviewReadMode::Blob, Some("link"))).is_err());
    assert!(read(&directory, &request(ReviewReadMode::Blob, Some("nested"))).is_err());
}

fn commit_file(repo: &Path, path: &str, subject: &str) -> String {
    std::fs::write(repo.join(path), format!("{subject}\n")).unwrap();
    git_in(repo, &["add", path]);
    git_in(repo, &["commit", "-m", subject]);
    git2::Repository::open(repo)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string()
}

#[test]
fn revision_commits_are_topological_and_page_without_duplicates() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    let first = directory.head.clone().unwrap();
    let second = commit_file(&repo, "second.txt", "second");
    let third = commit_file(&repo, "third.txt", "third");
    directory.head = Some(third.clone());
    let mut req = request(ReviewReadMode::Commits, None);
    req.limit = Some(2);
    let ReviewReadResult::Commits(page) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(
        page.commits.iter().map(|c| &c.oid).collect::<Vec<_>>(),
        [&third, &second]
    );
    assert!(page.commits_truncated);
    assert_eq!(
        page.commits[0].parents.as_slice(),
        std::slice::from_ref(&second)
    );
    assert_eq!(page.commits[0].subject, "third");
    assert!(!page.commits[0].author_name.is_empty());
    assert!(page.commits[0].time > 0);
    assert_eq!(
        (
            page.commits[0].files_changed,
            page.commits[0].insertions,
            page.commits[0].deletions
        ),
        (1, 1, 0)
    );
    req.cursor = page.next_cursor;
    let ReviewReadResult::Commits(last) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(last.commits[0].oid, first);
    assert!(!last.commits_truncated);
}

#[test]
fn single_commit_diff_filters_paths_and_rejects_outside_range() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    commit_file(&repo, "second.txt", "second");
    directory.head = Some(commit_file(&repo, "third.txt", "third"));
    let mut req = request(ReviewReadMode::Commit, None);
    req.commit = directory.head.clone();
    let ReviewReadResult::Changes(changes) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "third.txt");
    req.paths = vec!["second.txt".into()];
    let ReviewReadResult::Changes(filtered) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert!(filtered.files.is_empty());
    req.commit = Some(directory.base.as_ref().unwrap().oid.clone());
    assert!(read(&directory, &req)
        .unwrap_err()
        .contains("commit_outside_range"));
}

#[test]
fn since_revision_compares_trees_after_push_and_rewrite() {
    let (_temp, repo) = init_repo();
    let earlier = saved_directory(&repo);
    let mut directory = earlier.clone();
    directory.head = Some(commit_file(&repo, "second.txt", "second"));
    let mut req = request(ReviewReadMode::Since, None);
    req.from_snapshot_id = Some("earlier".into());
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert!(!changes.rewritten);
    assert_eq!(changes.since_baseline, Some(SinceBaseline::PreviousHead));
    assert_eq!(changes.files[0].path, "second.txt");
    git_in(
        &repo,
        &["reset", "--hard", &directory.base.as_ref().unwrap().oid],
    );
    directory.head = Some(commit_file(&repo, "rewrite.txt", "rewrite"));
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert!(changes.rewritten);
    assert_eq!(changes.since_baseline, Some(SinceBaseline::PreviousHead));
    assert_eq!(changes.files.len(), 2);
    assert!(changes.patch.unwrap().contains("-saved content"));
}

#[test]
fn commit_pages_reject_cursors_from_another_revision_and_invalid_limits() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    directory.head = Some(commit_file(&repo, "second.txt", "second"));
    let mut req = request(ReviewReadMode::Commits, None);
    req.limit = Some(1);
    let ReviewReadResult::Commits(first) = read(&directory, &req).unwrap() else {
        panic!()
    };
    req.cursor = first.next_cursor;
    directory.head = Some(commit_file(&repo, "third.txt", "third"));
    assert!(read(&directory, &req).unwrap_err().contains("cursor"));
    req.cursor = None;
    req.limit = Some(0);
    assert!(read(&directory, &req).is_err());
    req.limit = Some(201);
    assert!(read(&directory, &req).is_err());
}

#[test]
fn since_missing_earlier_head_returns_rewritten_tree_diff() {
    let (_temp, repo) = init_repo();
    let directory = saved_directory(&repo);
    let mut earlier = directory.clone();
    earlier.head = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    let mut req = request(ReviewReadMode::Since, None);
    req.from_snapshot_id = Some("earlier".into());
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert!(changes.rewritten);
    assert_eq!(changes.since_baseline, Some(SinceBaseline::Base));
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "new.txt");
}

#[test]
fn receiver_reads_survive_source_workspace_deletion() {
    let (temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    let earlier = directory.clone();
    directory.head = Some(commit_file(&repo, "second.txt", "second"));
    let receiver = temp.path().join("receiver.git");
    git_in(
        temp.path(),
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            receiver.to_str().unwrap(),
        ],
    );
    directory.common_git_dir = Some(receiver);
    std::fs::remove_dir_all(&repo).unwrap();
    let req = request(ReviewReadMode::Commits, None);
    let ReviewReadResult::Commits(commits) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(commits.commits.len(), 2);
    let mut req = request(ReviewReadMode::Commit, None);
    req.commit = directory.head.clone();
    let ReviewReadResult::Changes(changes) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(changes.files[0].path, "second.txt");
    req.mode = ReviewReadMode::Since;
    req.commit = None;
    req.from_snapshot_id = Some("earlier".into());
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert_eq!(changes.files[0].path, "second.txt");
    assert!(!changes.rewritten);
}

#[test]
fn merge_commit_diff_uses_first_parent_and_commit_list_respects_topology() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    git_in(&repo, &["checkout", "-b", "side"]);
    let side = commit_file(&repo, "side.txt", "side");
    git_in(&repo, &["checkout", "feature"]);
    let main = commit_file(&repo, "main.txt", "main");
    git_in(&repo, &["merge", "--no-ff", "side", "-m", "merge side"]);
    directory.head = Some(
        git2::Repository::open(&repo)
            .unwrap()
            .head()
            .unwrap()
            .target()
            .unwrap()
            .to_string(),
    );
    let mut req = request(ReviewReadMode::Commit, None);
    req.commit = directory.head.clone();
    req.paths = vec!["side.txt".into()];
    req.range = Some(FileRange {
        offset: 0,
        bytes: 4096,
        raw: None,
    });
    let ReviewReadResult::Changes(changes) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "side.txt");
    assert!(changes.patch.unwrap().contains("+side"));
    assert!(changes.range.unwrap().total > 0);
    req.paths.clear();
    req.range = None;
    let ReviewReadResult::Changes(changes) = read(&directory, &req).unwrap() else {
        panic!()
    };
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "side.txt");
    let ReviewReadResult::Commits(page) =
        read(&directory, &request(ReviewReadMode::Commits, None)).unwrap()
    else {
        panic!()
    };
    assert_eq!(page.commits[0].oid, directory.head.unwrap());
    assert_eq!(page.commits[0].parents, [main.clone(), side.clone()]);
    let positions: std::collections::HashMap<_, _> = page
        .commits
        .iter()
        .enumerate()
        .map(|(index, commit)| (commit.oid.as_str(), index))
        .collect();
    for commit in &page.commits {
        for parent in &commit.parents {
            if let Some(parent_position) = positions.get(parent.as_str()) {
                assert!(positions[commit.oid.as_str()] < *parent_position);
            }
        }
    }
}

#[test]
fn commit_list_display_fields_are_bounded_at_utf8_boundaries() {
    let (_temp, repo) = init_repo();
    let mut directory = saved_directory(&repo);
    let oversized = "é".repeat(600);
    std::fs::write(repo.join("large-subject.txt"), "large display fields\n").unwrap();
    git_in(&repo, &["add", "large-subject.txt"]);
    assert!(git_command(&repo, &["commit", "-m", &oversized])
        .env("GIT_AUTHOR_NAME", &oversized)
        .status()
        .unwrap()
        .success());
    directory.head = git2::Repository::open(&repo)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .map(|oid| oid.to_string());
    let ReviewReadResult::Commits(page) =
        read(&directory, &request(ReviewReadMode::Commits, None)).unwrap()
    else {
        panic!()
    };
    let commit = &page.commits[0];
    assert_eq!(commit.subject.len(), crate::gitgui::GIT_SUBJECT_MAX_BYTES);
    assert_eq!(
        commit.author_name.len(),
        crate::gitgui::GIT_SUBJECT_MAX_BYTES
    );
    assert!(oversized.starts_with(&commit.subject));
    assert!(oversized.starts_with(&commit.author_name));
}

#[test]
fn since_unavailable_earlier_capture_uses_recorded_base() {
    let (_temp, repo) = init_repo();
    let directory = saved_directory(&repo);
    let mut earlier = directory.clone();
    earlier.status = ReviewDirectoryStatus::Unavailable;
    earlier.head = None;
    earlier.base = None;
    earlier.common_git_dir = None;
    let mut req = request(ReviewReadMode::Since, None);
    req.from_snapshot_id = Some("unavailable-earlier".into());
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert!(changes.rewritten);
    assert_eq!(changes.since_baseline, Some(SinceBaseline::Base));
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, "new.txt");
    assert_eq!(changes.stat.files_changed, 1);
    assert!(!changes.patch.unwrap().contains("README.md"));
}

#[test]
fn since_genuinely_unborn_earlier_revision_uses_empty_previous_tree() {
    let (_temp, repo) = init_repo();
    let directory = saved_directory(&repo);
    let mut earlier = directory.clone();
    earlier.status = ReviewDirectoryStatus::NoCommits;
    earlier.head = None;
    earlier.base = None;
    let mut req = request(ReviewReadMode::Since, None);
    req.from_snapshot_id = Some("unborn-earlier".into());
    let ReviewReadResult::Changes(changes) = read_since(&directory, &earlier, &req).unwrap() else {
        panic!()
    };
    assert!(!changes.rewritten);
    assert_eq!(changes.since_baseline, Some(SinceBaseline::PreviousHead));
    assert_eq!(changes.files.len(), 2);
    assert!(changes.files.iter().any(|file| file.path == "README.md"));
}
