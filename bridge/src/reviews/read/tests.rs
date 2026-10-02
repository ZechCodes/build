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
