//! Actual reduced fixture plus the same RPC carrier and caller watchdog.
use super::*;
pub(super) struct Context {
    pub fixture: Arc<QualificationFixture>,
    pub watch: ClientWatch,
    pub guard: Option<ClientWatchGuard>,
    pub anchor: Instant,
    pub registry: Arc<WriteWaitRegistry>,
    pub fired: Arc<Mutex<Vec<WriteSnapshot>>>,
    pub watchdog: Option<crate::timing::write_wait::Watchdog>,
}
impl Context {
    pub async fn new(original: &str) -> Result<Self, String> {
        let (mut fixture, _) = QualificationFixture::pipeline_control().await;
        let watch = ClientWatch::new(Arc::new(Instant::now));
        let anchor = Instant::now() + Duration::from_millis(100);
        watch.register_relative_at(original, Duration::ZERO, TrafficCohort::SemanticWrite);
        let guard = watch.watch();
        let until = Instant::now() + Duration::from_secs(5);
        while !fixture.registry.active_snapshot_with_time().1.is_empty() && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let attached = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fixture.rpc.attach_client_watch(watch.clone())
        }))
        .is_ok();
        if !attached {
            use futures_util::FutureExt;
            let clean = std::panic::AssertUnwindSafe(fixture.shutdown())
                .catch_unwind()
                .await
                .is_ok();
            let joined = tokio::task::spawn_blocking(move || guard.finish())
                .await
                .is_ok();
            return Err(format!(
                "actual bootstrap attachment refused; native cleanup={clean}; watch join={joined}"
            ));
        }
        let fired = Arc::new(Mutex::new(Vec::new()));
        let records = Arc::clone(&fired);
        let registry = Arc::clone(&fixture.registry);
        let watchdog = registry.watchdog(
            Duration::from_millis(5),
            Arc::new(move |row| records.lock().unwrap().push(row)),
        );
        Ok(Self {
            fixture: Arc::new(fixture),
            watch,
            guard: Some(guard),
            anchor,
            registry,
            fired,
            watchdog: Some(watchdog),
        })
    }
    pub async fn wait_kind(&self, original: &str, kind: WaitKind) -> Option<Instant> {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            let (_, rows) = self.registry.active_snapshot_with_time();
            if let Some(started) = rows
                .iter()
                .filter(|row| row.original_id == original)
                .flat_map(|row| &row.active)
                .find(|span| span.category == IntervalCategory::KnownWait(kind))
                .map(|span| span.started)
            {
                return Some(started);
            }
            if Instant::now() >= until {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    pub async fn intended_offer(&mut self, original: &str) {
        self.anchor = Instant::now() + Duration::from_millis(100);
        self.watch.start_at(self.anchor);
        tokio::time::sleep_until(tokio::time::Instant::from_std(self.anchor)).await;
        self.watch.offer(original);
    }
    pub fn active(&self, original: &str) -> Option<WriteSnapshot> {
        self.registry
            .active_snapshot_with_time()
            .1
            .into_iter()
            .find(|row| row.original_id == original)
    }
    pub fn clients(&self) -> Vec<Value> {
        clients(&self.watch, self.anchor)
    }
    pub async fn shutdown(self) -> Result<WatchReport, RetainedFault> {
        let Context {
            fixture,
            mut guard,
            watchdog,
            registry: _,
            watch: _,
            fired: _,
            anchor: _,
        } = self;
        let fixture = match Arc::try_unwrap(fixture) {
            Ok(fixture) => fixture,
            Err(fixture) => {
                if let Some(guard) = guard.take() {
                    std::mem::forget(guard);
                }
                std::mem::forget(watchdog);
                return Err(retained(fixture));
            }
        };
        use futures_util::FutureExt;
        // Public native cleanup retires/joins every actual tab and carrier.
        let clean = std::panic::AssertUnwindSafe(fixture.shutdown())
            .catch_unwind()
            .await
            .is_ok();
        drop(watchdog);
        let report =
            tokio::task::spawn_blocking(move || guard.take().expect("owned watchdog").finish())
                .await;
        match report {
            Ok(report) if clean => Ok(report),
            _ => Err(RetainedFault {
                fixture: None,
                caller: None,
                raw_state: None,
                raw_rpc: None,
                private_roots: Vec::new(),
                holders: Vec::new(),
            }),
        }
    }
}
pub(super) fn retained(fixture: Arc<QualificationFixture>) -> RetainedFault {
    RetainedFault {
        fixture: Some(fixture),
        caller: None,
        raw_state: None,
        raw_rpc: None,
        private_roots: Vec::new(),
        holders: Vec::new(),
    }
}

pub(super) fn clients(watch: &ClientWatch, anchor: Instant) -> Vec<Value> {
    watch.snapshot().into_iter().map(|row|json!({"original_id":row.original_id,"intended_us":evidence::at(row.intended_at,anchor),"offered_us":row.offered_at.map(|v|evidence::at(v,anchor)),"sent_us":row.sent_at.map(|v|evidence::at(v,anchor)),"server_registered_us":row.server_registered_at.map(|v|evidence::at(v,anchor)),"admitted_us":row.admitted_at.map(|v|evidence::at(v,anchor)),"decoded_final_us":row.final_at.map(|v|evidence::at(v,anchor)),"cancelled":row.cancelled,"breached":row.breached})).collect()
}
