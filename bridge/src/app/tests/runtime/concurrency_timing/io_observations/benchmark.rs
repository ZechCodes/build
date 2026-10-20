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
}

impl Capture {
    fn new(trace: bool) -> Self {
        Self {
            origin: Instant::now(),
            phases: Default::default(),
            records: Vec::new(),
            trace,
        }
    }

    fn fixture(&mut self, fixture: usize) -> IoSample {
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
        let sample = IoSample::new_with_clock(clock);
        self.records.push(json!({"kind":"setup","fixture":fixture,
            "start_ns":start.duration_since(self.origin).as_nanos(),
            "duration_ns":start.elapsed().as_nanos(),"threads_before":threads_before,
            "threads_after":thread_count(),"phases":sample.setup}));
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
        if let Some(holder) = holder {
            holder.join().unwrap();
        }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit paired benchmark; not a routine unit gate"]
async fn paired_observation_benchmark() {
    let samples = count("BUILD_OBSERVATION_BENCH_SAMPLES", 128);
    let warmups = count("BUILD_OBSERVATION_BENCH_WARMUPS", 16);
    let cold = count("BUILD_OBSERVATION_BENCH_COLD", 16);
    assert!(samples > 0 && cold > 0);
    let trace = std::env::var("BUILD_OBSERVATION_BENCH_TRACE").is_ok_and(|v| v == "1");
    let output =
        PathBuf::from(std::env::var("BUILD_OBSERVATION_BENCH_OUTPUT").expect("output path"));
    let unix_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut capture = Capture::new(trace);
    capture.records.push(json!({"kind":"configuration","unix_ns":unix_ns,
        "samples":samples,"warmups":warmups,"cold_batches":cold,"trace":trace,
        "cold_semantics":"git.log is first observation and first RPC except git_worker, where the gated git.stage RPC precedes it; remaining methods are first-use positions",
        "client_endpoint":"before payload construction/encryption through final reply decryption",
        "gate_ms":GATE.as_millis()}));
    let mut fixture = 0;
    for scenario in ["isolated", "app_held", "git_worker"] {
        let mut retained = None;
        for batch in 0..cold {
            drop(retained.take());
            fixture += 1;
            let mut sample = capture.fixture(fixture);
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
            "fixture":fixture,"includes":"last cold + warmup + measured","stats":sample.sample.clock.stats()}));
    }
    capture
        .records
        .push(json!({"kind":"phases","events":*capture.phases.lock().unwrap()}));
    let mut file = std::fs::File::create(output).unwrap();
    for record in capture.records {
        writeln!(file, "{record}").unwrap();
    }
}
