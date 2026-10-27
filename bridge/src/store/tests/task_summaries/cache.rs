//! Repeated supported writes reuse their compiled projection program.
use super::*;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ProjectionPreparations<'a> {
    store: &'a Store,
    count: Box<AtomicUsize>,
}

unsafe extern "C" fn observe_projection_insert(
    state: *mut c_void,
    action: c_int,
    table: *const c_char,
    _column: *const c_char,
    _database: *const c_char,
    _trigger: *const c_char,
) -> c_int {
    if action == rusqlite::ffi::SQLITE_INSERT && !table.is_null() {
        // SAFETY: SQLite supplies a valid NUL-terminated table name for this
        // callback. The observer owns the atomic until it unregisters below.
        let is_projection = unsafe { CStr::from_ptr(table) }.to_bytes()
            == b"tracker_task_summaries";
        if is_projection {
            unsafe { &*state.cast::<AtomicUsize>() }.fetch_add(1, Ordering::Relaxed);
        }
    }
    rusqlite::ffi::SQLITE_OK
}

impl<'a> ProjectionPreparations<'a> {
    fn observe(store: &'a Store) -> Self {
        let mut observer = Self {
            store,
            count: Box::new(AtomicUsize::new(0)),
        };
        let connection = store.connection();
        // SAFETY: the mutex guard exclusively owns the live connection. The
        // boxed counter stays at a fixed address until Drop unregisters the
        // callback under the same mutex, before freeing the counter.
        let code = unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(
                connection.handle(),
                Some(observe_projection_insert),
                (&mut *observer.count as *mut AtomicUsize).cast(),
            )
        };
        assert_eq!(code, rusqlite::ffi::SQLITE_OK);
        observer
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

impl Drop for ProjectionPreparations<'_> {
    fn drop(&mut self) {
        let connection = self.store.connection();
        // SAFETY: the mutex excludes all users while the still-live callback
        // state is removed. The Store outlives this observer.
        let code = unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(
                connection.handle(),
                None,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(code, rusqlite::ffi::SQLITE_OK);
    }
}

fn save_metadata_and_comment(store: &Store, task: &mut Task, sequence: usize) {
    task.title = format!("metadata {sequence}");
    task.labels = vec![format!("label-{sequence}")];
    task.updated_at = format!("2026-01-01T00:00:{sequence:02}Z");
    let comment = said(task, &format!("tc-{sequence:026}"), &task.updated_at);
    store
        .save_tracker_task_activity(task, &[comment], &[])
        .unwrap();
}

#[test]
fn repeated_metadata_and_comment_writes_reuse_the_projection_program() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "cached writes", Actor::User, support::NOW);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let preparations = ProjectionPreparations::observe(&store);
    for sequence in 1..=3 {
        save_metadata_and_comment(&store, &mut task, sequence);
    }
    let warmed = preparations.count();
    assert!(warmed > 0, "the observer must see projection preparation");
    for sequence in 4..=8 {
        save_metadata_and_comment(&store, &mut task, sequence);
    }
    assert_eq!(
        preparations.count(),
        warmed,
        "normal metadata/history writes must reuse the warmed projection program"
    );
    let card = summary(&store, "/summary");
    assert_eq!(card["title"], "metadata 8");
    assert_eq!(card["labels"], serde_json::json!(["label-8"]));
    assert_eq!(card["unread_count"], 8);
    assert_matches_history(&store, &task.id);
}
