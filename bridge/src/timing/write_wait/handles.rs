use super::*;
use registry::Finish;

impl LogicalWrite {
    pub(crate) fn id(&self) -> WriteId {
        self.context.origins[0]
    }
    pub(crate) fn context(&self) -> WriteContext {
        self.context.clone()
    }
    pub(crate) fn child(&self) -> Obligation {
        self.context.child()
    }
    pub(crate) fn classify(
        &self,
        original_id: impl Into<String>,
        method: impl Into<String>,
        cohort: WriteCohort,
    ) {
        self.context
            .registry
            .classify(&self.context, original_id.into(), method.into(), cohort);
    }
    pub(crate) fn admitted(&self) {
        self.context.registry.milestone(&self.context, false);
    }
    pub(crate) fn acknowledged(&self) {
        self.context.registry.milestone(&self.context, true);
    }
    pub(crate) fn complete(mut self) {
        self.finished = true;
        self.context
            .registry
            .finish(&self.context, true, Finish::Complete);
    }
    pub(crate) fn cancel(mut self) {
        self.finished = true;
        self.context
            .registry
            .finish(&self.context, true, Finish::Cancel);
    }
}
impl Drop for LogicalWrite {
    fn drop(&mut self) {
        if !self.finished {
            self.context
                .registry
                .finish(&self.context, true, Finish::Abandon);
        }
    }
}
impl Obligation {
    pub(crate) fn id(&self) -> ObligationId {
        self.context.obligation
    }
    pub(crate) fn context(&self) -> WriteContext {
        self.context.clone()
    }
    pub(crate) fn child(&self) -> Obligation {
        self.context.child()
    }
    pub(crate) fn interval(&self, category: IntervalCategory) -> WaitGuard {
        self.context.interval(category)
    }
    pub(crate) fn complete(mut self) {
        self.finished = true;
        self.context
            .registry
            .finish(&self.context, false, Finish::Complete);
    }
    pub(crate) fn cancel(mut self) {
        self.finished = true;
        self.context
            .registry
            .finish(&self.context, false, Finish::Cancel);
    }
}
impl Drop for Obligation {
    fn drop(&mut self) {
        if !self.finished {
            self.context
                .registry
                .finish(&self.context, false, Finish::Abandon);
        }
    }
}
impl WriteContext {
    pub(crate) fn same_registry(&self, _other: &Self) -> bool {
        true
    }
    pub(crate) fn original_ids(&self) -> Vec<String> {
        let active = self.registry.active.load_full();
        self.origins
            .iter()
            .map(|id| {
                let record = active
                    .get(id)
                    .expect("logical-write context requires a live origin");
                assert!(
                    record.outstanding.contains(&self.obligation),
                    "logical-write context requires owning obligation"
                );
                record.row.original_id.clone()
            })
            .collect()
    }

    pub(crate) fn origins(&self) -> &[WriteId] {
        &self.origins
    }
    pub(crate) fn child(&self) -> Obligation {
        self.registry
            .add_child(&self.origins, Some(self.obligation))
    }
    pub(crate) fn interval(&self, category: IntervalCategory) -> WaitGuard {
        let (id, ended) = self.registry.start_interval(self, category);
        WaitGuard {
            id,
            context: self.clone(),
            ended,
        }
    }
    /// Bind only inside a synchronous section. Explicitly carry context across await.
    pub(crate) fn bind_sync(&self) -> SyncContextGuard {
        let previous = CURRENT.with(|current| current.replace(Some(self.clone())));
        SyncContextGuard {
            previous,
            not_send: std::marker::PhantomData,
        }
    }
}
impl WaitGuard {
    pub(crate) fn id(&self) -> WaitId {
        self.id
    }
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.ended.get_or_init(|| (self.context.registry.clock)());
        self.context
            .registry
            .closing
            .store(self.id.0, Ordering::Release);
        self.context.registry.close_interval(&self.context, self.id);
    }
}
impl Drop for SyncContextGuard {
    fn drop(&mut self) {
        CURRENT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            thread.join().unwrap();
        }
    }
}
