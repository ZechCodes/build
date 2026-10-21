use super::*;

#[test]
fn completed_rows_follow_native_completion_order_in_bounded_batches() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let mut roots: Vec<_> = (0..5)
        .map(|index| {
            Some(registry.begin(format!("archive-{index}"), "thread.post", registry.now()))
        })
        .collect();
    let mut expected = Vec::new();
    for index in [4, 1, 3, 0, 2] {
        let root = roots[index].take().unwrap();
        expected.push(root.id());
        if index == 3 {
            root.cancel();
        } else {
            root.complete();
        }
    }
    let live = registry.begin("still-live", "thread.post", registry.now());
    let first = registry.completed_snapshots_since(&mut cursor, 2).unwrap();
    let second = registry.completed_snapshots_since(&mut cursor, 2).unwrap();
    let third = registry.completed_snapshots_since(&mut cursor, 2).unwrap();
    let empty = registry.completed_snapshots_since(&mut cursor, 2).unwrap();
    assert_eq!(
        first.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected[..2]
    );
    assert_eq!(
        second.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected[2..4]
    );
    assert_eq!(
        third.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected[4..]
    );
    assert!(empty.is_empty());
    assert_eq!(cursor.0.next, 5);
    assert!(second[0].cancelled);
    assert!(first
        .iter()
        .chain(&second)
        .chain(&third)
        .all(|row| row.completed_at.is_some()));
    let id = live.id();
    live.complete();
    assert_eq!(
        registry.completed_snapshots_since(&mut cursor, usize::MAX),
        Err(ArchiveCursorError::OversizedBatchLimit)
    );
    assert_eq!(cursor.0.next, 5);
    let later = registry
        .completed_snapshots_since(&mut cursor, MAX_ARCHIVE_BATCH)
        .unwrap();
    assert_eq!(later.iter().map(|row| row.id).collect::<Vec<_>>(), [id]);
    assert!(registry
        .completed_snapshots_since(&mut cursor, 1)
        .unwrap()
        .is_empty());
    let mut exported: Vec<_> = first
        .into_iter()
        .chain(second)
        .chain(third)
        .chain(later)
        .collect();
    exported.sort_unstable_by_key(|row| row.id);
    assert_eq!(
        exported,
        registry.snapshot(),
        "cursor preserves every native final field"
    );
}

#[test]
fn raw_interval_cursor_preserves_close_order_and_shared_origins_once() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_intervals_cursor();
    let first = registry.begin("first", "thread.post", registry.now());
    let second = registry.begin("second", "thread.post", registry.now());
    let origins = vec![first.id(), second.id()];
    let joined = WriteContext::coalesced(&[first.context(), second.context()]);
    let earlier = joined.interval(IntervalCategory::OwnExternal);
    let later = joined.interval(IntervalCategory::SqlService);
    let expected = [later.id(), earlier.id()];
    drop(later);
    let one = registry.completed_intervals_since(&mut cursor, 1).unwrap();
    let empty = registry.completed_intervals_since(&mut cursor, 1).unwrap();
    drop(earlier);
    first.complete();
    second.complete();
    joined.complete();
    let two = registry.completed_intervals_since(&mut cursor, 1).unwrap();
    assert_eq!(
        one.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected[..1]
    );
    assert!(empty.is_empty());
    assert_eq!(
        two.iter().map(|row| row.id).collect::<Vec<_>>(),
        expected[1..]
    );
    assert_eq!(one[0].origins, origins);
    assert_eq!(two[0].origins, origins);
    assert_eq!(cursor.0.next, 2);
    assert_eq!(
        one.into_iter().chain(two).collect::<Vec<_>>(),
        registry.completed_intervals()
    );
}

#[test]
fn invalid_or_foreign_cursors_never_advance_or_poison_the_collector() {
    let registry = WriteWaitRegistry::new();
    let foreign = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let mut raw = registry.completed_intervals_cursor();
    let root = registry.begin("control", "thread.post", registry.now());
    let id = root.id();
    let interval = root.context().interval(IntervalCategory::SqlService);
    drop(interval);
    root.complete();
    assert_eq!(
        foreign.completed_snapshots_since(&mut cursor, 1),
        Err(ArchiveCursorError::ForeignRegistry)
    );
    assert_eq!(
        foreign.completed_intervals_since(&mut raw, 1),
        Err(ArchiveCursorError::ForeignRegistry)
    );
    assert_eq!(
        registry.completed_snapshots_since(&mut cursor, 0),
        Err(ArchiveCursorError::EmptyBatchLimit)
    );
    assert_eq!(
        registry.completed_intervals_since(&mut raw, 0),
        Err(ArchiveCursorError::EmptyBatchLimit)
    );
    assert_eq!(
        registry.completed_snapshots_since(&mut cursor, MAX_ARCHIVE_BATCH + 1),
        Err(ArchiveCursorError::OversizedBatchLimit)
    );
    assert_eq!(
        registry.completed_intervals_since(&mut raw, usize::MAX),
        Err(ArchiveCursorError::OversizedBatchLimit)
    );
    assert_eq!((cursor.0.next, raw.0.next), (0, 0));
    cursor.0.next = 2;
    raw.0.next = 2;
    assert_eq!(
        registry.completed_snapshots_since(&mut cursor, 1),
        Err(ArchiveCursorError::InvalidPosition)
    );
    assert_eq!(
        registry.completed_intervals_since(&mut raw, 1),
        Err(ArchiveCursorError::InvalidPosition)
    );
    assert_eq!((cursor.0.next, raw.0.next), (2, 2));
    cursor.0.next = 0;
    raw.0.next = 0;
    assert_eq!(
        registry
            .completed_snapshots_since(&mut cursor, 1)
            .unwrap()
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert_eq!(
        registry
            .completed_intervals_since(&mut raw, 1)
            .unwrap()
            .len(),
        1
    );
}
