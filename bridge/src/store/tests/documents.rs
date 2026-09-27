use super::*;

/// Deleting a Task takes its canonical plan docs with it. They live
/// outside the database on purpose, so deleting only rows would leave the
/// plan on disk forever with nothing referring to it.
#[test]
fn deleting_a_task_removes_its_canonical_docs_too() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let store = Store::new(&root).expect("store opens");
    let docs = root.join("issues").join("plan-1").join("docs");
    std::fs::create_dir_all(docs.join(".build/plan")).unwrap();
    std::fs::write(docs.join(".build/plan/01-stage.md"), "# stage").unwrap();

    store.delete_plan("plan-1").expect("the delete succeeds");
    assert!(
        !root.join("issues").join("plan-1").exists(),
        "the Task's docs outlived the Task"
    );
    store
        .delete_plan("plan-1")
        .expect("deleting twice is the same as deleting once");
}
