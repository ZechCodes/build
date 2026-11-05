use super::*;

#[test]
fn bounded_task_timeline_pages_match_legacy_order_without_gaps_or_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "paged timeline");
    let comments = (0..125)
        .map(|i| comment(&task, &format!("synthetic {i}"), NOW))
        .collect::<Vec<_>>();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    let legacy = store.load_tracker_timeline(&task.id).unwrap();
    let mut page = store.task_timeline_page(&task.id, None, 50).unwrap();
    assert_eq!(page.timeline.len(), 50);
    assert!(page.has_more);
    let mut collected = page.timeline;
    while let Some(cursor) = page.next_cursor {
        page = store
            .task_timeline_page(&task.id, Some(&cursor), 50)
            .unwrap();
        collected.splice(0..0, page.timeline.clone());
    }
    assert!(!page.has_more);
    assert_eq!(
        serde_json::to_value(collected).unwrap(),
        serde_json::to_value(legacy).unwrap()
    );
}

#[test]
fn timeline_cursor_rejects_another_task_store_incarnation_and_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "cursor source");
    let comments = (0..3)
        .map(|i| comment(&task, &format!("synthetic {i}"), NOW))
        .collect::<Vec<_>>();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    let cursor = store
        .task_timeline_page(&task.id, None, 1)
        .unwrap()
        .next_cursor
        .unwrap();
    let another = filed(&store, "another task");
    assert!(store
        .task_timeline_page(&another.id, Some(&cursor), 1)
        .is_err());
    assert!(store
        .task_timeline_page(&task.id, Some("not-a-cursor"), 1)
        .is_err());
    let mut changed = cursor.clone().into_bytes();
    changed[0] = if changed[0] == b'a' { b'b' } else { b'a' };
    assert!(store
        .task_timeline_page(&task.id, Some(std::str::from_utf8(&changed).unwrap()), 1)
        .is_err());
    store
        .writer_for_test()
        .execute(
            "UPDATE meta SET value='synthetic-other-incarnation' WHERE key='sync_incarnation'",
            [],
        )
        .unwrap();
    assert!(store
        .task_timeline_page(&task.id, Some(&cursor), 1)
        .is_err());
}

#[test]
fn bounded_timeline_limits_bytes_and_does_not_decode_unrequested_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "bounded bytes");
    let old = comment(&task, "old", "2026-01-01T00:00:00Z");
    let newest = comment(&task, "new", "2026-12-01T00:00:00Z");
    store
        .save_tracker_task_activity(&task, &[old.clone(), newest], &[])
        .unwrap();
    store
        .writer_for_test()
        .execute(
            "UPDATE tracker_comments SET record='invalid old record' WHERE id=?1",
            [&old.id],
        )
        .unwrap();
    assert_eq!(
        store
            .task_timeline_page(&task.id, None, 1)
            .unwrap()
            .timeline
            .len(),
        1
    );
    assert!(store.task_timeline_page(&task.id, None, 0).is_err());
    assert!(store.task_timeline_page(&task.id, None, 101).is_err());
}

#[test]
fn timeline_read_watermark_covers_news_outside_the_timestamp_window() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "read watermark");
    let newest = comment(&task, "new timestamp", "2026-12-01T00:00:00Z");
    let old = comment(&task, "old timestamp newer ULID", "2026-01-01T00:00:00Z");
    store
        .save_tracker_task_activity(&task, &[newest, old.clone()], &[])
        .unwrap();
    let page = store.task_timeline_page(&task.id, None, 1).unwrap();
    assert_ne!(page.timeline[0].ordering_key().1, old.id);
    assert_eq!(page.timeline_read_through.as_deref(), Some(old.id.as_str()));
}

#[test]
fn later_oversized_history_waits_behind_a_continuation_and_is_refused_when_requested() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "oversized older item");
    let old = comment(&task, &"x".repeat(65_536), "2026-01-01T00:00:00Z");
    let newest = comment(&task, "bounded newest", "2026-12-01T00:00:00Z");
    store
        .save_tracker_task_activity(&task, &[old, newest], &[])
        .unwrap();
    let page = store.task_timeline_page(&task.id, None, 50).unwrap();
    assert!(!page.timeline.is_empty());
    assert!(page.has_more);
    let refused = store.task_timeline_page(&task.id, page.next_cursor.as_deref(), 50);
    assert!(refused.unwrap_err().to_string().contains("exceeds"));
}

#[test]
fn anchored_timeline_window_resolves_an_old_entry_without_draining_newer_pages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "anchor target");
    let comments = (0..125)
        .map(|i| comment(&task, &format!("synthetic {i}"), NOW))
        .collect::<Vec<_>>();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    let target = &comments[22].id;
    let page = store
        .task_timeline_anchor_page(&task.id, target, 10)
        .unwrap();
    assert!(page
        .timeline
        .iter()
        .any(|row| row.ordering_key().1 == target));
    assert_eq!(page.timeline.len(), 10);
    assert!(page.has_more);
    let other = filed(&store, "foreign anchor");
    assert!(store
        .task_timeline_anchor_page(&other.id, target, 10)
        .is_err());
}

#[test]
fn last_reviewed_snapshot_hint_survives_outside_the_newest_timeline_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "review hint");
    let mut reviewed = comment(&task, "reviewed", "2026-01-01T00:00:00Z");
    reviewed.author = Actor::User;
    reviewed.opinion = Some(Box::new(crate::tracker::ReviewOpinion {
        snapshot_id: "synthetic-review-snapshot".into(),
        verdict: crate::tracker::ReviewVerdict::Approve,
    }));
    let newest = comment(&task, "unrelated", "2026-12-01T00:00:00Z");
    store
        .save_tracker_task_activity(&task, &[reviewed, newest], &[])
        .unwrap();
    let page = store.task_timeline_page(&task.id, None, 1).unwrap();
    assert_eq!(
        page.last_reviewed_snapshot_id.as_deref(),
        Some("synthetic-review-snapshot")
    );
    assert!(!page.timeline_incarnation.is_empty());
}

#[test]
fn opinion_index_upgrade_reports_cost_for_small_and_long_histories() {
    for length in [8, 4096] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let task = filed(&store, "opinion index upgrade");
        let mut comments = (0..length)
            .map(|i| comment(&task, &format!("synthetic {i}"), NOW))
            .collect::<Vec<_>>();
        comments[0].author = Actor::User;
        comments[0].opinion = Some(Box::new(crate::tracker::ReviewOpinion {
            snapshot_id: "synthetic-index-review".into(),
            verdict: crate::tracker::ReviewVerdict::Approve,
        }));
        store
            .save_tracker_task_activity(&task, &comments, &[])
            .unwrap();
        let elapsed = {
            let connection = store.writer_for_test();
            let sql: String = connection.query_row("SELECT sql FROM sqlite_master WHERE type='index' AND name='tracker_comments_user_opinions'", [], |row|row.get(0)).unwrap();
            connection
                .execute_batch("DROP INDEX tracker_comments_user_opinions")
                .unwrap();
            let started = std::time::Instant::now();
            connection.execute_batch(&sql).unwrap();
            let elapsed = started.elapsed();
            let installed:bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='tracker_comments_user_opinions')",[],|row|row.get(0)).unwrap();
            assert!(installed);
            let plan = connection.prepare("EXPLAIN QUERY PLAN SELECT CASE WHEN length(CAST(json_extract(record,'$.opinion.snapshot_id') AS BLOB))<=256 THEN json_extract(record,'$.opinion.snapshot_id') END FROM tracker_comments WHERE task_id=?1 AND CASE WHEN json_valid(record) THEN json_extract(record,'$.author.kind')='user' AND json_type(record,'$.opinion.snapshot_id')='text' ELSE 0 END ORDER BY created_at DESC,id DESC LIMIT 1").unwrap().query_map([&task.id],|row|row.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join("\n");
            assert!(plan.contains("tracker_comments_user_opinions"), "{plan}");
            assert!(!plan.contains("TEMP B-TREE"), "{plan}");
            elapsed
        };
        let page = store.task_timeline_page(&task.id, None, 50).unwrap();
        assert_eq!(
            page.last_reviewed_snapshot_id.as_deref(),
            Some("synthetic-index-review")
        );
        eprintln!("task_opinion_index_upgrade comments={length} build_micros={} note=single-synthetic-index-build-not-total-startup",elapsed.as_micros());
    }
}
