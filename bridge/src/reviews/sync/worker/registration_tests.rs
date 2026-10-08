use super::*;
use crate::reviews::sync::reconcile::tests::Fixture;
use crate::watch::metadata::operations;
use rusqlite::{params, Connection};
use std::cell::Cell;

thread_local! {
    static LOADS: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn record_load() {
    LOADS.set(LOADS.get() + 1);
}

fn counts() -> (usize, usize, usize) {
    let (resolutions, registrations) = operations::counts();
    (LOADS.get(), resolutions, registrations)
}

fn seed_review(connection: &Connection, fixture: &Fixture, number: usize) -> PathBuf {
    let repository = fixture._home.path().join(format!("receiver-{number}.git"));
    git2::Repository::init_bare(&repository).unwrap();
    let task = format!("z-paged-{number:03}");
    let snapshot = format!("snapshot-{number}");
    connection
        .execute(
            "INSERT INTO reviews SELECT ?1, workspace_id, version,
         json_set(record, '$.task_id', ?1,
                  '$.pull_request.latest_published_snapshot_id', ?2)
         FROM reviews WHERE task_id = ?3",
            params![task, snapshot, fixture.task_id()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO review_snapshots SELECT ?1, ?2, number,
         json_set(record, '$.id', ?1) FROM review_snapshots WHERE task_id = ?3",
            params![snapshot, task, fixture.task_id()],
        )
        .unwrap();
    seed_binding(connection, fixture, &task, &repository);
    repository.canonicalize().unwrap()
}

fn seed_binding(
    connection: &Connection,
    fixture: &Fixture,
    task: &str,
    repository: &std::path::Path,
) {
    connection
        .execute(
            "INSERT INTO review_branch_bindings
         SELECT ?1, directory_id, ?1, dedicated_ref, ?2, receiving_ref,
         json_set(record, '$.working_repository', ?2,
                  '$.source_repository', ?2, '$.receiving_repository', ?2)
         FROM review_branch_bindings WHERE task_id = ?3",
            params![task, repository.to_str().unwrap(), fixture.task_id()],
        )
        .unwrap();
}

fn scan_to_completion(worker: &mut Worker, now: Instant) -> (usize, usize, usize) {
    let before = counts();
    let stopped = AtomicBool::new(true);
    for turn in 0..512 {
        let previous = counts();
        worker.turn_at(&stopped, now + Duration::from_millis(turn));
        let current = counts();
        let operations = current.0 - previous.0 + current.1 - previous.1 + current.2 - previous.2;
        assert!(
            operations <= 8,
            "turn {turn} performed {operations} loads, resolutions and registrations"
        );
        if worker.scanning.is_none() {
            return (
                current.0 - before.0,
                current.1 - before.1,
                current.2 - before.2,
            );
        }
    }
    panic!("metadata discovery did not finish");
}

#[test]
fn multipage_registration_and_refresh_are_bounded_per_worker_turn() {
    let fixture = Fixture::new();
    let connection = Connection::open(fixture._home.path().join("db/build.db")).unwrap();
    let roots: Vec<_> = (0..PAGE + 3)
        .map(|number| seed_review(&connection, &fixture, number))
        .collect();
    let mut worker = Worker::new(fixture.store.clone(), Arc::new(|_| {}));
    let started = Instant::now();
    let startup = scan_to_completion(&mut worker, started);
    assert_eq!(startup.0, PAGE + 4, "every active state is loaded once");
    assert_eq!(startup.1, PAGE + 6, "shared binding paths resolve once");
    assert_eq!(
        startup.2,
        PAGE + 6,
        "shared metadata identities register once"
    );
    for (number, root) in roots.iter().enumerate() {
        assert!(worker.identities[root].contains(&format!("z-paged-{number:03}")));
    }
    let refresh = scan_to_completion(&mut worker, started + POLL + Duration::from_secs(1));
    assert_eq!(
        refresh, startup,
        "poll refreshes roots incrementally each scan"
    );
    connection
        .execute("DELETE FROM reviews WHERE task_id LIKE 'z-paged-%'", [])
        .unwrap();
    scan_to_completion(&mut worker, started + POLL * 2 + Duration::from_secs(2));
    for root in roots {
        assert!(!worker.identities.contains_key(&root));
        assert!(
            !worker.watchers.identities().contains(&root),
            "stale watcher removed"
        );
    }
}

#[test]
fn discovered_metadata_routes_events_before_the_last_page_finishes() {
    let fixture = Fixture::new();
    let connection = Connection::open(fixture._home.path().join("db/build.db")).unwrap();
    for number in 0..PAGE {
        seed_review(&connection, &fixture, number);
    }
    let mut worker = Worker::new(fixture.store.clone(), Arc::new(|_| {}));
    let started = Instant::now();
    worker.turn_at(&AtomicBool::new(true), started);
    assert!(worker.scanning.is_some());
    let root = fixture.review().bindings[0]
        .receiving_repository
        .canonicalize()
        .unwrap();
    assert!(worker.identities[&root].contains(fixture.task_id()));
    assert!(worker.watchers.identities().contains(&root));
    worker.scheduler.ready(started, usize::MAX);
    let (sender, events) = mpsc::channel();
    worker.events = events;
    sender.send(root).unwrap();
    worker.drain_events(started);
    assert!(worker
        .scheduler
        .ready(started + Duration::from_millis(499), usize::MAX)
        .is_empty());
    assert_eq!(
        worker
            .scheduler
            .ready(started + Duration::from_millis(500), usize::MAX),
        [fixture.task_id()]
    );
}
