//! Isolated diagnostics, never used by the encrypted cold-request fixture.
use super::*;
use crate::reviews::records::Review;
use crate::store::Store;
use crate::tracker::Task;
use serde_json::{json, Value};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const ROUNDS: usize = 16;
const SCHEMA_PRIME: &str = "SELECT 1 FROM tracker_tasks LIMIT 0";

#[derive(Clone, Copy)]
enum Chain {
    Attachment,
    ReviewDiff,
}

impl Chain {
    fn name(self) -> &'static str {
        match self {
            Self::Attachment => "tasks.attachment",
            Self::ReviewDiff => "tasks.review.diff",
        }
    }

    fn reader(self, read: &ReadTransaction<'_>, id: &str) -> Metadata {
        Metadata {
            task: read.load_tracker_task(id).unwrap().unwrap(),
            review: match self {
                Self::Attachment => None,
                Self::ReviewDiff => read.load_review(id).unwrap(),
            },
        }
    }

    fn writer(self, store: &Store, id: &str) -> Metadata {
        Metadata {
            task: store.load_tracker_task(id).unwrap().unwrap(),
            review: match self {
                Self::Attachment => None,
                Self::ReviewDiff => store.load_review(id).unwrap(),
            },
        }
    }
}

#[derive(Debug, PartialEq)]
struct Metadata {
    task: Task,
    review: Option<Review>,
}

fn resources() -> Value {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let selected: Vec<_> = status
        .lines()
        .filter(|line| {
            [
                "VmRSS:",
                "Threads:",
                "voluntary_ctxt_switches:",
                "nonvoluntary_ctxt_switches:",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    json!({
        "unix_ns":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        "loadavg":std::fs::read_to_string("/proc/loadavg").unwrap(),
        "self_stat":std::fs::read_to_string("/proc/self/stat").unwrap(),
        "self_status":selected,
    })
}

async fn measure_pool(
    store: &Store,
    chain: Chain,
    expected: &Metadata,
    round: usize,
    schema_primed: bool,
    rows: &mut Vec<Value>,
) {
    let control = if schema_primed {
        "schema_primed"
    } else {
        "fresh"
    };
    let before = resources();
    let started = Instant::now();
    let pool = ReadPool::open(&store.dir.join(crate::store::DB_FILE)).unwrap();
    let open_ns = started.elapsed().as_nanos();
    let started = Instant::now();
    let mut connections = Vec::with_capacity(READ_CONNECTIONS);
    for _ in 0..READ_CONNECTIONS {
        connections.push(pool.acquire().await);
    }
    let borrow_all_ns = started.elapsed().as_nanos();
    rows.push(
        json!({"kind":"pool_setup","round":round,"method":chain.name(),
        "control":control,"connections":connections.len(),"open_ns":open_ns,
        "borrow_all_ns":borrow_all_ns,"resources_before":before}),
    );
    // Retain all eight until the whole group ends: LIFO checkout cannot reuse one.
    for (index, connection) in connections.iter_mut().enumerate() {
        let phases = Arc::new(Mutex::new(None));
        let observed = phases.clone();
        connection.observe_transactions(move |begin, query, commit| {
            *observed.lock().unwrap() =
                Some([begin.as_nanos(), query.as_nanos(), commit.as_nanos()]);
        });
        if schema_primed {
            let started = Instant::now();
            let statement = connection
                .connection
                .as_ref()
                .unwrap()
                .prepare(SCHEMA_PRIME)
                .unwrap();
            let prepare_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            drop(statement);
            let drop_ns = started.elapsed().as_nanos();
            rows.push(
                json!({"kind":"schema_prime","round":round,"method":chain.name(),
                "connection":index,"prepare_ns":prepare_ns,"drop_ns":drop_ns,
                "sql":SCHEMA_PRIME,"stepped":false,"statement_cached":false}),
            );
        }
        for read_number in 1..=2 {
            let started = Instant::now();
            let answer = connection
                .read(|read| chain.reader(read, &expected.task.id))
                .unwrap();
            let wall_ns = started.elapsed().as_nanos();
            let [begin, query, commit] = phases.lock().unwrap().take().unwrap();
            assert_eq!(
                &answer, expected,
                "{control} connection {index} read {read_number}"
            );
            rows.push(json!({"kind":"read","round":round,"method":chain.name(),
                "control":control,"connection":index,"read_number":read_number,
                "wall_ns":wall_ns,"begin_ns":begin,"query_ns":query,"commit_ns":commit}));
        }
    }
    rows.push(
        json!({"kind":"pool_end","round":round,"method":chain.name(),
        "control":control,"resources_after":resources()}),
    );
}

fn measure_writer(
    store: &Store,
    chain: Chain,
    expected: &Metadata,
    round: usize,
    rows: &mut Vec<Value>,
) {
    rows.push(
        json!({"kind":"writer_start","round":round,"method":chain.name(),
        "resources_before":resources()}),
    );
    for index in 0..READ_CONNECTIONS {
        for read_number in 1..=2 {
            let started = Instant::now();
            let answer = chain.writer(store, &expected.task.id);
            let wall_ns = started.elapsed().as_nanos();
            assert_eq!(
                &answer, expected,
                "writer sample {index} read {read_number}"
            );
            rows.push(json!({"kind":"read","round":round,"method":chain.name(),
                "control":"warmed_writer","sample":index,"read_number":read_number,
                "wall_ns":wall_ns,"begin_ns":null,"query_ns":wall_ns,"commit_ns":null}));
        }
    }
    rows.push(
        json!({"kind":"writer_end","round":round,"method":chain.name(),
        "resources_after":resources()}),
    );
}

#[tokio::test]
#[ignore = "manual isolated SQL metadata diagnostics; raw samples, no latency assertion"]
async fn mixed_metadata_cold_connections_and_schema_prime() {
    let before = resources();
    let started = Instant::now();
    let fixture = crate::reviews::sync::reconcile::tests::Fixture::new();
    let fixture_setup_ns = started.elapsed().as_nanos();
    let mut rows = vec![json!({"kind":"configuration","rounds":ROUNDS,
        "connections_per_pool":READ_CONNECTIONS,"distinct_first_reads_per_control_and_method":ROUNDS*READ_CONNECTIONS,
        "fixture_setup_ns":fixture_setup_ns,"resources_before_fixture":before,
        "resources_after_fixture":resources(),"schema_prime_sql":SCHEMA_PRIME,
        "writer_transactions":false,"pool_transactions":true,"statement_cache":false,
        "note":"Dedicated temporary fixture; never runs during encrypted cold acceptance captures."})];
    for chain in [Chain::Attachment, Chain::ReviewDiff] {
        let expected = chain.writer(&fixture.store, fixture.task_id());
        if matches!(chain, Chain::ReviewDiff) {
            let review = expected.review.as_ref().expect("fixture has a review");
            assert!(!review.snapshots.is_empty());
            assert!(!review.snapshots[0].directories.is_empty());
        }
        rows.push(json!({"kind":"payload","method":chain.name(),
            "task_record_bytes":serde_json::to_vec(&expected.task).unwrap().len(),
            "review_record_bytes":serde_json::to_vec(&expected.review).unwrap().len(),
            "review_snapshots":expected.review.as_ref().map(|review|review.snapshots.len())}));
        for round in 0..ROUNDS {
            if round % 2 == 0 {
                measure_pool(&fixture.store, chain, &expected, round, false, &mut rows).await;
                measure_writer(&fixture.store, chain, &expected, round, &mut rows);
                measure_pool(&fixture.store, chain, &expected, round, true, &mut rows).await;
            } else {
                measure_pool(&fixture.store, chain, &expected, round, true, &mut rows).await;
                measure_writer(&fixture.store, chain, &expected, round, &mut rows);
                measure_pool(&fixture.store, chain, &expected, round, false, &mut rows).await;
            }
        }
    }
    for row in rows {
        println!("MIXED_METADATA_TIMING {row}");
    }
}
