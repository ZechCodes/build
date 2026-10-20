//! Opt-in raw samples. The acceptance path installs no internal phase observer.
use super::*;
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub(super) fn setup_mark(records: &mut Vec<Value>, phase: &str, start: &mut Instant) {
    let end = Instant::now();
    records.push(
        json!({"phase":phase,"duration_ns":end.duration_since(*start).as_nanos(),
        "threads_after":thread_count()}),
    );
    *start = Instant::now();
}

fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

fn count(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |value| value.parse().expect("positive integer"))
}

struct Capture {
    origin: Instant,
    phases: Arc<Mutex<Vec<Value>>>,
    records: Vec<Value>,
    trace: bool,
    sql_startup_eager: bool,
    previous_stats: BTreeMap<usize, Value>,
}

impl Capture {
    fn new(trace: bool, sql_startup_eager: bool) -> Self {
        Self {
            origin: Instant::now(),
            phases: Default::default(),
            records: Vec::new(),
            trace,
            sql_startup_eager,
            previous_stats: BTreeMap::new(),
        }
    }

    fn fixture(&mut self, fixture: usize, first_method: Option<&str>, scenario: &str) -> IoSample {
        let clock = FrameClock::new();
        if self.trace {
            let phases = Arc::clone(&self.phases);
            let origin = self.origin;
            clock.observe_requests(Arc::new(move |frame, phase, at| {
                phases.lock().unwrap().push(json!({"fixture":fixture,
                    "id":frame.payload["id"],"method":frame.payload["method"],
                    "phase":phase,"at_ns":at.duration_since(origin).as_nanos()}));
            }));
        }
        let start = Instant::now();
        let threads_before = thread_count();
        let mut sample = IoSample::new_with_clock_and_sql_startup(clock, self.sql_startup_eager);
        if let Some(first) = first_method {
            let position = sample
                .requests
                .iter()
                .position(|(method, _)| *method == first)
                .expect("FIRST_METHOD must exactly name one of the 22 fixture observations");
            let request = sample.requests.remove(position);
            sample.requests.insert(0, request);
        }
        let order: Vec<_> = sample.requests.iter().map(|(method, _)| *method).collect();
        let configuration = self.records.first_mut().unwrap();
        if configuration["actual_request_order"].is_null() {
            configuration["actual_request_order"] = json!(order);
            configuration["first_observation_method"] = json!(order[0]);
            configuration["first_carrier_rpc_method"] = json!(if scenario == "git_worker" {
                "git.stage"
            } else {
                order[0]
            });
        } else {
            assert_eq!(configuration["actual_request_order"], json!(order));
        }
        self.records.push(json!({"kind":"setup","fixture":fixture,
            "start_ns":start.duration_since(self.origin).as_nanos(),
            "duration_ns":start.elapsed().as_nanos(),"threads_before":threads_before,
            "scenario":scenario,"first_fixture_in_process":fixture == 1,
            "first_carrier_rpc_method":if scenario == "git_worker" {"git.stage"} else {order[0]},
            "first_observation_method":order[0],"actual_request_order":order,
            "threads_after":thread_count(),"phases":sample.setup,
            "read_pool_initialization":sample.read_pool_initialization,
            "observation_worker_starts":sample.worker_starts.load(Ordering::Acquire),
            "sql_worker_starts":sample.sql_worker_starts.load(Ordering::Acquire),
            "sql_startup_mode":if self.sql_startup_eager {"eager8"} else {"demand"}}));
        self.previous_stats
            .insert(fixture, sample.sample.clock.stats());
        sample
    }

    async fn batch(
        &mut self,
        sample: &mut IoSample,
        scenario: &str,
        fixture: usize,
        population: &str,
        batch: usize,
    ) {
        let workers_before = sample.worker_starts.load(Ordering::Acquire);
        let sql_workers_before = sample.sql_worker_starts.load(Ordering::Acquire);
        let holder = gate(sample, scenario).await;
        let mut pending = BTreeMap::new();
        for (position, (method, params)) in sample.requests.clone().into_iter().enumerate() {
            let id = sample.sample.next_id;
            // Registration/JSON allocation stays outside the client wall interval.
            let metadata = json!({"kind":"request","scenario":scenario,"fixture":fixture,
                "population":population,"batch":batch,"position":position,"id":id,"method":method});
            let started = Instant::now();
            sample.sample.send(method, params).await;
            pending.insert(id, (started, method, metadata));
            if scenario == "isolated" {
                self.answers(sample, &mut pending, 1).await;
            }
        }
        if scenario != "isolated" {
            let replies = pending.len() + usize::from(scenario == "git_worker");
            self.answers(sample, &mut pending, replies).await;
        }
        assert!(pending.is_empty(), "all observation replies checked");
        let workers_after_client_replies = sample.worker_starts.load(Ordering::Acquire);
        let sql_workers_after_client_replies = sample.sql_worker_starts.load(Ordering::Acquire);
        if let Some(holder) = holder {
            holder.join().unwrap();
        }
        self.capture_queue_deltas(sample, fixture, scenario == "git_worker")
            .await;
        self.records.push(json!({"kind":"observation_workers","fixture":fixture,
            "scenario":scenario,"population":population,"batch":batch,
            "before_batch":workers_before,"after_client_replies":workers_after_client_replies,
            "after_bookkeeping":sample.worker_starts.load(Ordering::Acquire),
            "at_ns":Instant::now().duration_since(self.origin).as_nanos(),
            "semantics":"actual successful dedicated I/O thread starts; client reply may precede execution completion; no quiescence forced"}));
        self.records.push(json!({"kind":"sql_workers","fixture":fixture,
            "scenario":scenario,"population":population,"batch":batch,
            "before_batch":sql_workers_before,"after_client_replies":sql_workers_after_client_replies,
            "after_bookkeeping":sample.sql_worker_starts.load(Ordering::Acquire),
            "at_ns":Instant::now().duration_since(self.origin).as_nanos(),
            "semantics":"actual successful shared SQL thread starts; client reply may precede execution completion; no quiescence forced"}));
    }

    async fn capture_queue_deltas(&mut self, sample: &IoSample, fixture: usize, writer: bool) {
        let previous = self.previous_stats.get(&fixture).unwrap();
        // Timer drop may follow reply enqueue. Wait only after every client timestamp.
        let current = tokio::time::timeout(PATIENCE, async {
            loop {
                let current = sample.sample.clock.stats();
                let complete = sample.requests.iter().all(|(method, _)| {
                    let before = served(previous, method);
                    let after = served(&current, method);
                    assert!(after <= before + 1, "one computation per method per batch");
                    after == before + 1
                        && ["queued", "lock_wait", "held"].iter().all(|phase| {
                            current["methods"][method][phase]["count"]
                                .as_u64()
                                .unwrap_or(0)
                                == previous["methods"][method][phase]["count"]
                                    .as_u64()
                                    .unwrap_or(0)
                                    + 1
                        })
                }) && current["frames_served"].as_u64().unwrap()
                    >= previous["frames_served"].as_u64().unwrap()
                        + sample.requests.len() as u64
                        + u64::from(writer);
                if complete {
                    break sample.sample.clock.stats();
                }
                tokio::time::sleep(Duration::from_micros(100)).await;
            }
        })
        .await
        .expect("all completed replies finish production timing bookkeeping");
        for record in self.records.iter_mut().rev().take(sample.requests.len()) {
            assert_eq!(record["kind"], "request");
            assert_eq!(record["fixture"], fixture);
            let method = record["method"].as_str().unwrap().to_string();
            for phase in ["queued", "lock_wait", "held"] {
                let before = sum_micros(previous, &method, phase);
                let after = sum_micros(&current, &method, phase);
                record[format!("frame_{phase}_ns")] = json!((after - before) * 1_000);
            }
            record["frame_count_delta"] = json!(1);
        }
        self.previous_stats.insert(fixture, current);
    }

    async fn answers(
        &mut self,
        sample: &mut IoSample,
        pending: &mut BTreeMap<u64, (Instant, &'static str, Value)>,
        replies: usize,
    ) {
        for _ in 0..replies {
            loop {
                let envelope = within_patience(sample.sample.wire.outbound.recv()).await;
                let answer = SessionSender::decrypt_push(&sample.sample.key, &envelope);
                let finished = Instant::now();
                if answer.get("accepted").is_some() {
                    continue;
                }
                assert_eq!(answer["ok"], true, "{answer}");
                let id = answer["id"].as_u64().unwrap();
                if let Some((started, method, mut metadata)) = pending.remove(&id) {
                    validate_payload(method, &answer["result"]);
                    metadata["client_start_ns"] =
                        json!(started.duration_since(self.origin).as_nanos());
                    metadata["client_reply_ns"] =
                        json!(finished.duration_since(self.origin).as_nanos());
                    metadata["client_total_ns"] =
                        json!(finished.duration_since(started).as_nanos());
                    self.records.push(metadata);
                } else {
                    assert_eq!(
                        id + 1 + sample.requests.len() as u64,
                        sample.sample.next_id,
                        "only the known gate writer may have an unregistered reply"
                    );
                }
                break;
            }
        }
    }
}

fn served(stats: &Value, method: &str) -> u64 {
    stats["methods"][method]["served"].as_u64().unwrap_or(0)
}

fn sum_micros(stats: &Value, method: &str, phase: &str) -> u64 {
    // Production sums are integer microseconds exposed as milliseconds.
    (stats["methods"][method][phase]["sum_ms"]
        .as_f64()
        .unwrap_or(0.0)
        * 1_000.0)
        .round() as u64
}

async fn gate(sample: &mut IoSample, scenario: &str) -> Option<std::thread::JoinHandle<()>> {
    match scenario {
        "isolated" => None,
        "app_held" => {
            let state = Arc::clone(&sample.sample.state);
            let (entered, acquired) = mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _guard = state.lock().unwrap();
                entered.send(()).unwrap();
                std::thread::sleep(GATE);
            });
            acquired.recv_timeout(PATIENCE).unwrap();
            Some(holder)
        }
        "git_worker" => {
            let (gate, handle) = crate::app::OffLockGate::new();
            sample.sample.state.lock().unwrap().off_lock_gate = Some(gate);
            sample
                .sample
                .send(
                    "git.stage",
                    json!({"project_id":sample.sample.project_id,
                "paths":["README.md"]}),
                )
                .await;
            handle.wait_for_arrival();
            sample.sample.state.lock().unwrap().off_lock_gate = None;
            Some(std::thread::spawn(move || {
                std::thread::sleep(GATE);
                handle.release();
            }))
        }
        _ => panic!("unknown benchmark scenario"),
    }
}

fn selected_scenarios(selected: Option<&str>) -> Vec<&'static str> {
    match selected {
        None => vec!["isolated", "app_held", "git_worker"],
        Some("isolated") => vec!["isolated"],
        Some("app_held") => vec!["app_held"],
        Some("git_worker") => vec!["git_worker"],
        Some(_) => panic!("SCENARIO must exactly name isolated, app_held, or git_worker"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit paired benchmark; not a routine unit gate"]
async fn paired_observation_benchmark() {
    let samples = count("BUILD_OBSERVATION_BENCH_SAMPLES", 128);
    let warmups = count("BUILD_OBSERVATION_BENCH_WARMUPS", 16);
    let cold = count("BUILD_OBSERVATION_BENCH_COLD", 16);
    assert!(samples > 0 && cold > 0);
    let trace = std::env::var("BUILD_OBSERVATION_BENCH_TRACE").is_ok_and(|v| v == "1");
    let first_method = std::env::var("BUILD_OBSERVATION_BENCH_FIRST_METHOD").ok();
    let selected_scenario = std::env::var("BUILD_OBSERVATION_BENCH_SCENARIO").ok();
    let requested_sql_startup = std::env::var("BUILD_OBSERVATION_BENCH_SQL_STARTUP").ok();
    let sql_startup_mode = requested_sql_startup.as_deref().unwrap_or("demand");
    assert!(
        matches!(sql_startup_mode, "demand" | "eager8"),
        "SQL_STARTUP must exactly name demand or eager8"
    );
    let scenarios = selected_scenarios(selected_scenario.as_deref());
    let output =
        PathBuf::from(std::env::var("BUILD_OBSERVATION_BENCH_OUTPUT").expect("output path"));
    let unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut capture = Capture::new(trace, sql_startup_mode == "eager8");
    capture.records.push(json!({"kind":"configuration","unix_ns":unix_ns,
        "samples":samples,"warmups":warmups,"cold_batches":cold,"trace":trace,
        "requested_first_method":first_method,"requested_scenario":selected_scenario,
        "requested_sql_startup":requested_sql_startup,"sql_startup_mode":sql_startup_mode,
        "sql_startup_semantics":"per-handler test policy only; same generic named SQLpool mechanism starts8 at timed lazy initialization in eager8 mode or grows from0 on demand; benchmark adds no reader warmup",
        "sql_worker_starts_available":true,
        "read_pool_initialization_available":true,
        "read_pool_initialization_semantics":"natural production schema initialization is nested within Store/helper seed; benchmark performs no additional SQL warmup",
        "observation_worker_starts_available":true,
        "scenarios":scenarios,"actual_request_order":null,
        "process_semantics":"cold batches reset app/handler/intake fixtures within one test process; they are not OS-process restarts. Only fixture1 carries this process's first encrypted carrier data RPC. Seed helpers invoke app logic before the carrier data RPC.",
        "cold_semantics":"actual_request_order[0] is the first encrypted observation RPC per fresh fixture; it is also the first carrier data RPC except git_worker, where gated git.stage precedes it. Other observations are first-use positions. Setup identifies the first fixture in the process; handler/intake reset is distinct from process restart.",
        "client_endpoint":"before payload construction/encryption through final reply decryption",
        "gate_ms":GATE.as_millis()}));
    let mut fixture = 0;
    for scenario in scenarios {
        let mut retained = None;
        for batch in 0..cold {
            drop(retained.take());
            fixture += 1;
            let mut sample = capture.fixture(fixture, first_method.as_deref(), scenario);
            capture
                .batch(&mut sample, scenario, fixture, "cold", batch)
                .await;
            retained = Some(sample);
        }
        let mut sample = retained.unwrap();
        for batch in 0..warmups {
            capture
                .batch(&mut sample, scenario, fixture, "warmup", batch)
                .await;
        }
        for batch in 0..samples {
            capture
                .batch(&mut sample, scenario, fixture, "measured", batch)
                .await;
        }
        capture.records.push(json!({"kind":"frame_clock","scenario":scenario,
            "fixture":fixture,"includes":"last cold + warmup + measured","stats":sample.sample.clock.stats(),
            "observation_worker_starts":sample.worker_starts.load(Ordering::Acquire),
            "sql_worker_starts":sample.sql_worker_starts.load(Ordering::Acquire)}));
    }
    capture
        .records
        .push(json!({"kind":"phases","events":*capture.phases.lock().unwrap()}));
    let mut file = std::fs::File::create(output).unwrap();
    for record in capture.records {
        writeln!(file, "{record}").unwrap();
    }
}
