//! The one-off import, run against a copy of a real JSON store.
//!
//! Ignored by default: it needs a store to import, named by
//! `BUILD_MIGRATION_FIXTURE`. The daemon gets exactly one chance at this on a
//! real machine, so the import is exercised against real records — records with
//! the shapes that actually accumulated, not the ones a test author thought to
//! write.
//!
//! ```text
//! BUILD_MIGRATION_FIXTURE=/path/to/tasks cargo test --test real_store_migration -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use build_bridge::store::Store;

fn fixture() -> Option<PathBuf> {
    std::env::var_os("BUILD_MIGRATION_FIXTURE").map(PathBuf::from)
}

/// Every thread item on every agent of a record set, keyed so the two sides can
/// be compared without caring what order anything was read in.
fn conversations(store: &Store) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for issue in store.load_all_issues().expect("issues load") {
        for agent in issue.issue.agents.iter() {
            out.insert(
                agent.id.clone(),
                agent
                    .thread
                    .items
                    .iter()
                    .map(|item| serde_json::to_string(item).unwrap())
                    .collect(),
            );
        }
    }
    for run in store.load_all_runs().expect("runs load") {
        for agent in run.agents.iter() {
            out.insert(
                agent.id.clone(),
                agent
                    .thread
                    .items
                    .iter()
                    .map(|item| serde_json::to_string(item).unwrap())
                    .collect(),
            );
        }
    }
    out
}

fn json_records(root: &Path) -> (usize, usize, usize, usize) {
    let count = |dir: &str, suffix: &str| -> usize {
        std::fs::read_dir(root.join(dir))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().to_string_lossy().ends_with(suffix))
                    .count()
            })
            .unwrap_or(0)
    };
    let issues = std::fs::read_dir(root.join("issues"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().join("record.json").is_file())
                .count()
        })
        .unwrap_or(0);
    let implementations: usize = std::fs::read_dir(root.join("issues"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let raw = std::fs::read_to_string(entry.path().join("record.json")).ok()?;
                    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
                    Some(value["implementations"].as_array()?.len())
                })
                .sum()
        })
        .unwrap_or(0);
    (
        issues,
        implementations + count("runs", ".json"),
        count("captures", ".json"),
        count("archived-worktrees", ".json"),
    )
}

#[test]
#[ignore = "needs BUILD_MIGRATION_FIXTURE pointing at a copy of a real JSON store"]
fn the_real_store_imports_with_every_record_and_conversation_intact() {
    let Some(source) = fixture() else {
        panic!("set BUILD_MIGRATION_FIXTURE to a COPY of a real ~/.build/tasks");
    };
    let work = tempfile::tempdir().expect("temp dir");
    let root = work.path().join("tasks");
    copy_tree(&source, &root);

    let (issues_on_disk, runs_on_disk, captures_on_disk, archived_on_disk) = json_records(&root);
    println!(
        "json store: {issues_on_disk} issues, {runs_on_disk} runs, \
         {captures_on_disk} captures, {archived_on_disk} archived worktrees"
    );
    assert!(issues_on_disk > 0, "the fixture has no issues to import");

    let store = Store::new(&root).expect("store opens");
    let imported = store.import_json_store().expect("the import succeeds");
    println!("imported {imported} records");

    let issues = store.load_all_issues().expect("issues load");
    let runs = store.load_all_runs().expect("runs load");
    let captures = store.load_all_captures().expect("captures load");
    let archived = store
        .load_all_archived_worktrees()
        .expect("archived worktrees load");

    assert_eq!(issues.len(), issues_on_disk, "every issue survives");
    assert_eq!(runs.len(), runs_on_disk, "every run survives");
    assert_eq!(captures.len(), captures_on_disk, "every capture survives");
    assert_eq!(
        archived.len(),
        archived_on_disk,
        "every archived worktree survives"
    );
    assert!(
        !store.load_attention().is_empty(),
        "the attention map survives"
    );

    // Conversations are the thing that cannot be re-derived. Count them, and
    // check every item is byte-identical after a round trip through the rows.
    let before = conversations(&store);
    let items: usize = before.values().map(Vec::len).sum();
    println!("{} agents, {items} conversation items", before.len());
    assert!(items > 0, "the fixture has no conversation to preserve");

    // Reopen from scratch: nothing may depend on the process that imported.
    let reopened = Store::new(&root).expect("store reopens");
    assert_eq!(
        reopened.import_json_store().expect("second import runs"),
        0,
        "the import is one-way — running it again must do nothing"
    );
    assert_eq!(
        conversations(&reopened),
        before,
        "every conversation item survives a reopen unchanged"
    );

    // The JSON tree is parked, never deleted.
    assert!(
        root.join("runs.imported").exists() || runs_on_disk == 0,
        "the imported runs directory is parked, not removed"
    );
    for entry in std::fs::read_dir(root.join("issues")).expect("issues dir") {
        let dir = entry.expect("entry").path();
        assert!(
            !dir.join("record.json").exists(),
            "{} still holds a live JSON record",
            dir.display()
        );
        assert!(
            dir.join("record.json.imported").exists(),
            "{} lost its parked JSON record",
            dir.display()
        );
    }

    // Plan docs stay on disk, because an agent reads and writes them.
    let docs: usize = std::fs::read_dir(root.join("issues"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| count_files(&entry.path().join("docs")))
                .sum()
        })
        .unwrap_or(0);
    println!("{docs} plan doc files left in place");
    assert!(docs > 0, "plan docs must not be swept into the database");
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            if entry.path().is_dir() {
                count_files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create dir");
    for entry in std::fs::read_dir(from).expect("read source") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}
