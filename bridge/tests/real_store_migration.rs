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
fn conversations(store: &Store) -> BTreeMap<String, Vec<serde_json::Value>> {
    let mut out: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for issue in store.load_all_issues().expect("issues load") {
        for agent in issue.issue.agents.iter() {
            out.insert(
                agent.id.clone(),
                agent
                    .thread
                    .items
                    .iter()
                    .map(|item| serde_json::to_value(item).unwrap())
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
                    .map(|item| serde_json::to_value(item).unwrap())
                    .collect(),
            );
        }
    }
    out
}

/// Every conversation item in the JSON tree, keyed by agent id, straight out of
/// the records — never through the store that is under test.
///
/// Counting records proves nothing about what is inside them. This is the side
/// of the comparison the migration must reproduce exactly: if an item's body,
/// its sequence, or its seen state changed on the way in, only comparing the
/// items themselves will say so.
fn conversations_in_json(root: &Path) -> BTreeMap<String, Vec<serde_json::Value>> {
    let mut out: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    let mut absorb = |value: &serde_json::Value| {
        for agent in value["agents"].as_array().into_iter().flatten() {
            let Some(id) = agent["id"].as_str() else {
                continue;
            };
            out.insert(
                id.to_string(),
                agent["thread"]["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect(),
            );
        }
    };
    if let Ok(entries) = std::fs::read_dir(root.join("issues")) {
        for entry in entries.flatten() {
            let Ok(raw) = std::fs::read_to_string(entry.path().join("record.json")) else {
                continue;
            };
            let aggregate: serde_json::Value = serde_json::from_str(&raw).expect("issue parses");
            absorb(&aggregate["issue"]);
            for implementation in aggregate["implementations"]
                .as_array()
                .into_iter()
                .flatten()
            {
                absorb(implementation);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(root.join("runs")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            absorb(&serde_json::from_str::<serde_json::Value>(&raw).expect("run parses"));
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

    // Conversations are the thing that cannot be re-derived. Compare them
    // against the JSON they came from — item by item, not by count — because a
    // migration that dropped a field or reordered a thread would pass every
    // count assertion above it.
    let source = conversations_in_json(&root);
    let before = conversations(&store);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        source.keys().collect::<Vec<_>>(),
        "the imported store holds exactly the agents the JSON did"
    );
    for (agent_id, items) in &source {
        assert_eq!(
            before.get(agent_id),
            Some(items),
            "agent {agent_id}: the imported conversation differs from the JSON it came from"
        );
    }
    let items: usize = before.values().map(Vec::len).sum();
    println!(
        "{} agents, {items} conversation items — every one identical to its JSON",
        before.len()
    );
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

    // The JSON tree is untouched — which is what makes "throw the database
    // away and rebuild" a real recovery rather than a claim. Prove it: delete
    // the database (and the marker with it) and import again from scratch.
    for entry in std::fs::read_dir(root.join("issues")).expect("issues dir") {
        let dir = entry.expect("entry").path();
        assert!(
            dir.join("record.json").is_file(),
            "{} lost its JSON record — the import must not move it",
            dir.display()
        );
    }
    drop(reopened);
    for sidecar in ["build.db", "build.db-wal", "build.db-shm"] {
        let _ = std::fs::remove_file(root.join(sidecar));
    }
    let rebuilt = Store::new(&root).expect("store reopens after the database is deleted");
    let reimported = rebuilt.import_json_store().expect("the rebuild imports");
    assert_eq!(
        reimported, imported,
        "a rebuild imports exactly what it did before"
    );
    assert_eq!(
        conversations(&rebuilt),
        source,
        "a database rebuilt from the untouched JSON holds the same conversations"
    );

    // The import leaves a note for whoever opens this directory, and that note
    // is the timestamp the rollback guard measures against.
    assert!(
        root.join("SUPERSEDED-BY-build.db.md").is_file(),
        "the import left no note saying these records were superseded"
    );
    rebuilt
        .refuse_a_rolled_back_store()
        .expect("an untouched tree is not a rollback");

    // Now stage the rollback: an older bridge writing a record back. Starting
    // must refuse rather than serve one of the two copies silently.
    let touched = std::fs::read_dir(root.join("issues"))
        .expect("issues dir")
        .flatten()
        .map(|entry| entry.path().join("record.json"))
        .find(|path| path.is_file())
        .expect("a record to touch");
    let raw = std::fs::read_to_string(&touched).expect("record reads");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&touched, raw).expect("record rewrites");
    match rebuilt.refuse_a_rolled_back_store() {
        Err(error) => println!("rollback refused: {error}"),
        Ok(()) => panic!("a JSON record written after the import was not noticed"),
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
