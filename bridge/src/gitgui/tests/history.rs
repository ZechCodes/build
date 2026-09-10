// Exact test bodies moved from the former inline test module.
use super::*;

#[test]
fn hash_prefix_validation_accepts_only_lowercase_hex_of_4_to_40() {
    assert!(is_valid_hash_prefix("deadbeef"));
    assert!(is_valid_hash_prefix("0123"));
    assert!(is_valid_hash_prefix(&"a".repeat(40)));
    assert!(!is_valid_hash_prefix("abc")); // too short
    assert!(!is_valid_hash_prefix(&"a".repeat(41))); // too long
    assert!(!is_valid_hash_prefix("DEADBEEF")); // uppercase
    assert!(!is_valid_hash_prefix("deadbeeg")); // non-hex
    assert!(!is_valid_hash_prefix("HEAD~1")); // revspec, not a hash
    assert!(!is_valid_hash_prefix(""));
}
#[test]
fn truncation_respects_utf8_boundaries_and_flags() {
    let (untouched, truncated) = truncate_at_utf8_boundary("short".to_string(), 100);
    assert_eq!(untouched, "short");
    assert!(!truncated);

    // "é" is two bytes; a cap landing mid-char must back off, not panic.
    let (cut, truncated) = truncate_at_utf8_boundary("aé".to_string(), 2);
    assert_eq!(cut, "a");
    assert!(truncated);

    let (exact, truncated) = truncate_at_utf8_boundary("abcd".to_string(), 4);
    assert_eq!(exact, "abcd");
    assert!(!truncated);
}
#[test]
fn status_files_list_is_capped_with_a_truncation_flag() {
    let dir = tempfile::tempdir().unwrap();
    git2::Repository::init(dir.path()).unwrap();
    for i in 0..8 {
        std::fs::write(dir.path().join(format!("f{i:02}.txt")), "x\n").unwrap();
    }

    // Over the cap: first N by path, flagged; the stat stays exact.
    let capped = status_payload_with_file_cap(dir.path(), 5).unwrap();
    let files = capped["files"].as_array().unwrap();
    assert_eq!(files.len(), 5);
    assert_eq!(capped["files_truncated"], true);
    assert_eq!(files[0]["path"], "f00.txt");
    assert_eq!(files[4]["path"], "f04.txt");
    assert_eq!(files[0]["added"], 1);
    assert_eq!(capped["stat"]["files_changed"], 8);
    assert_eq!(capped["stat"]["insertions"], 8);

    // At the cap exactly: everything fits, no flag.
    let uncapped = status_payload_with_file_cap(dir.path(), 8).unwrap();
    assert_eq!(uncapped["files"].as_array().unwrap().len(), 8);
    assert_eq!(uncapped["files_truncated"], false);
}
