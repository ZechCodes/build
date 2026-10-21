//! A read start must witness an original writer request actually outstanding.
use super::*;

#[derive(Default)]
pub(in super::super) struct Progress {
    actors: AtomicUsize,
    requests: AtomicUsize,
    changed: tokio::sync::Notify,
    starts: Mutex<Vec<Value>>,
    sends: Mutex<std::collections::BTreeMap<String, (usize, usize)>>,
    actor_totals: Mutex<Vec<f64>>,
}

pub(in super::super) struct Writer<'a>(&'a Progress);
impl Drop for Writer<'_> {
    fn drop(&mut self) {
        self.0.actors.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

pub(in super::super) struct Request<'a>(&'a Progress, bool);
impl Drop for Request<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.requests.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Progress {
    pub(in super::super) fn writer(&self) -> Writer<'_> {
        self.actors.fetch_add(1, Ordering::SeqCst);
        Writer(self)
    }

    pub(in super::super) fn request(&self, action: &BridgeAction) -> Request<'_> {
        let writer = action.tool_name() != "get_task";
        if writer {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.changed.notify_waiters();
        }
        Request(self, writer)
    }

    pub(super) async fn read_started(&self, agent: &str) {
        let began = Instant::now();
        let reached = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let requests = self.requests.load(Ordering::SeqCst);
                let actors = self.actors.load(Ordering::SeqCst);
                if requests > 0 {
                    return Some((actors, requests));
                }
                if actors == 0 {
                    return None;
                }
                changed.await;
            }
        })
        .await
        .ok()
        .flatten();
        self.starts.lock().unwrap().push(json!({
            "agent_id":agent,"start_delay_ms":began.elapsed().as_secs_f64()*1000.0,
            "original_writer_actors":reached.map(|row|row.0),
            "outstanding_writer_requests":reached.map(|row|row.1),
        }));
    }

    pub(in super::super) fn at_send(&self, action: &BridgeAction) {
        if !TOOLS.contains(&action.tool_name()) {
            return;
        }
        let overlapping = self.requests.load(Ordering::SeqCst) > 0;
        let mut sends = self.sends.lock().unwrap();
        let counts = sends.entry(action.tool_name().into()).or_default();
        counts.0 += 1;
        counts.1 += usize::from(overlapping);
    }

    pub(super) fn reader_finished(&self, elapsed: Duration) {
        self.actor_totals
            .lock()
            .unwrap()
            .push(elapsed.as_secs_f64() * 1000.0);
    }

    pub(in super::super) fn snapshot(&self) -> Value {
        let starts = self.starts.lock().unwrap();
        let overlapping = starts
            .iter()
            .filter(|row| row["outstanding_writer_requests"].as_u64().unwrap_or(0) > 0)
            .count();
        let sends = self
            .sends
            .lock()
            .unwrap()
            .iter()
            .map(|(tool, counts)| {
                (
                    tool.clone(),
                    json!({"actual_sends":counts.0,"sends_with_outstanding_writers":counts.1}),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let delays = starts
            .iter()
            .map(|row| row["start_delay_ms"].as_f64().unwrap())
            .collect();
        json!({"read_actors_started":starts.len(),"per_read_method":sends,
            "test_coordination_start_delay_ms":summary(delays),
            "read_actor_wall_ms_including_coordination":summary(self.actor_totals.lock().unwrap().clone()),"started_with_outstanding_writers":overlapping,
            "original_writer_actors_at_capture":self.actors.load(Ordering::SeqCst),
            "outstanding_writer_requests_at_capture":self.requests.load(Ordering::SeqCst),"starts":*starts})
    }

    pub(in super::super) fn assert_complete(&self) {
        let value = self.snapshot();
        assert_eq!(value["read_actors_started"], AGENTS, "{value}");
        assert_eq!(
            value["started_with_outstanding_writers"], AGENTS,
            "every read actor must start before original writer completion: {value}"
        );
        for tool in TOOLS {
            let sent = &value["per_read_method"][tool];
            assert_eq!(
                sent["actual_sends"],
                AGENTS * ROUNDS,
                "all samples remain measured for {tool}: {value}"
            );
            if tool != "get_task" {
                assert!(
                    sent["sends_with_outstanding_writers"].as_u64().unwrap_or(0) > 0,
                    "each new read must actually overlap original writer requests: {tool}: {value}"
                );
            }
        }
    }
}

fn summary(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let count = values.len();
    if count == 0 {
        return json!({"count":0});
    }
    json!({"count":count,"sum_ms":values.iter().sum::<f64>(),"p50_ms":values[(count*50).div_ceil(100)-1],
        "p95_ms":values[(count*95).div_ceil(100)-1],"max_ms":values[count-1]})
}
