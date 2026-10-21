//! Independent sampling: no coordinator/app/writer/collector mutation mutex.
use super::*;
pub(super) struct WatchThread {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}
impl WatchThread {
    pub(super) fn start(observe: Observation, probe: Arc<dyn IndependentProbe>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let join = std::thread::spawn(move || {
            let mut latched = BTreeSet::new();
            while !stopping.load(Ordering::Acquire) {
                let now = observe.now_us();
                let mut send = |v: Violation| {
                    if latched.insert((v.original_id.clone(), v.kind.clone())) {
                        let _ = observe.wake.send(v);
                    }
                };
                inspect(probe.snapshot(), now, &mut send);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            join: Some(join),
        }
    }
    pub(super) fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn inspect(snapshot: ProbeSnapshot, now: u64, send: &mut impl FnMut(Violation)) {
    for v in snapshot.caller_breaches {
        send(v);
    }
    for row in snapshot.registry {
        inspect_registry(row, now, send);
    }
    for failure in snapshot.producer_failures {
        send(Violation {
            original_id: None,
            kind: "producer".into(),
            observed_at_us: now,
            measured_us: None,
            limit_us: None,
            detail: failure,
        });
    }
}
fn inspect_registry(row: OpenRegistryRow, now: u64, send: &mut impl FnMut(Violation)) {
    for (kind, measured) in [
        ("known_wait_union", row.known_wait_union_us),
        ("wait_plus_sql_union", row.wait_plus_sql_union_us),
        ("conservative_wait_upper", row.conservative_upper_us),
    ] {
        if measured > WRITE_BOUND_US {
            send(Violation {
                original_id: Some(row.original_id.clone()),
                kind: kind.into(),
                observed_at_us: now,
                measured_us: Some(measured),
                limit_us: Some(WRITE_BOUND_US),
                detail: format!("open registry {}", row.write_id),
            });
        }
    }
    for service in row.active_finite_services {
        if service.elapsed_us > service.ceiling_us {
            send(Violation {
                original_id: Some(row.original_id.clone()),
                kind: "unfinished_finite_service".into(),
                observed_at_us: now,
                measured_us: Some(service.elapsed_us),
                limit_us: Some(service.ceiling_us),
                detail: service.registry_interval_id,
            });
        }
    }
}
