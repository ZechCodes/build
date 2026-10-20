//! Direct per-computation samples, kept outside production timing and wire stats.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::read_context::ReadService;
use crate::store::ReadConnection;
use crate::transport::Frame;

type Phases = BTreeMap<&'static str, Duration>;
type Requests = BTreeMap<u64, Phases>;
type Methods = BTreeMap<String, Requests>;

#[derive(Clone, Default)]
pub(super) struct SqlMeasurements(Arc<Mutex<Methods>>);

impl SqlMeasurements {
    pub(super) fn scheduled(&self, frame: &Frame, checkout: Duration, handoff: Duration) {
        self.record(frame, "checkout", checkout);
        self.record(frame, "worker_handoff", handoff);
    }

    pub(super) fn call(
        &self,
        reads: &ReadService,
        frame: Frame,
        mut connection: Option<ReadConnection>,
    ) -> Value {
        let observed = frame.clone();
        if let Some(connection) = &mut connection {
            let measurements = self.clone();
            let observed = observed.clone();
            connection.observe_transactions(move |setup, query, finish| {
                measurements.record(&observed, "transaction_setup", setup);
                measurements.record(&observed, "query", query);
                measurements.record(&observed, "transaction_finish", finish);
            });
        }
        let started = Instant::now();
        let answer = reads.call(frame, connection);
        self.record(&observed, "service", started.elapsed());
        answer
    }

    fn record(&self, frame: &Frame, phase: &'static str, duration: Duration) {
        let method = frame.payload["method"].as_str().unwrap();
        let id = frame.payload["id"].as_u64().unwrap();
        let previous = self
            .0
            .lock()
            .unwrap()
            .entry(method.to_string())
            .or_default()
            .entry(id)
            .or_default()
            .insert(phase, duration);
        assert!(previous.is_none(), "one {phase} sample per computation");
    }

    pub(super) fn snapshot(&self) -> Value {
        let methods = self.0.lock().unwrap();
        let summaries: BTreeMap<_, _> = methods
            .iter()
            .map(|(method, requests)| (method, summarize(requests)))
            .collect();
        json!(summaries)
    }
}

fn summarize(requests: &Requests) -> Value {
    let mut phases = BTreeMap::<&str, Vec<f64>>::new();
    for request in requests.values() {
        for (phase, elapsed) in request {
            phases
                .entry(phase)
                .or_default()
                .push(elapsed.as_secs_f64() * 1000.0);
        }
    }
    let summaries: BTreeMap<_, _> = phases
        .into_iter()
        .map(|(phase, values)| (phase, summarize_values(values)))
        .collect();
    let samples: BTreeMap<_, BTreeMap<_, _>> = requests
        .iter()
        .map(|(id, phases)| {
            (
                *id,
                phases
                    .iter()
                    .map(|(phase, duration)| (*phase, duration.as_secs_f64() * 1000.0))
                    .collect(),
            )
        })
        .collect();
    let mut summary = json!(summaries);
    summary["samples_ms"] = json!(samples);
    summary
}

fn summarize_values(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let count = values.len();
    json!({
        "count": count,
        "sum_ms": values.iter().sum::<f64>(),
        "p50_ms": values[(count * 50 - 1) / 100],
        "p95_ms": values[(count * 95 - 1) / 100],
        "max_ms": values[count - 1],
    })
}
