//! Shared query-class budgets, acquired before borrowing a SQL connection.
use super::{OwnedSemaphorePermit, ReadPool, Semaphore, READ_CONNECTIONS};
use std::sync::Arc;

/// A fixed set of query shapes, shared by all listeners using this pool.
/// Callers, projects and sockets never create additional semaphore classes.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum ReadClass {
    TaskDetails,
    TaskList,
    TaskDependents,
    HistorySearch,
    HistoryRead,
    Comment,
    ReviewSummary,
    ReviewContents,
}

const CLASSES: usize = ReadClass::ReviewContents as usize + 1;
// Two saturated query classes occupy at most half the SQL pool, leaving the
// other half for unrelated reads. This is not a reservation against all types.
const READS_PER_CLASS: usize = READ_CONNECTIONS / 4;
const _: () = assert!(READS_PER_CLASS > 0);

pub(super) struct ReadQuotas([Arc<Semaphore>; CLASSES]);

impl ReadQuotas {
    pub(super) fn new() -> Self {
        Self(std::array::from_fn(|_| {
            Arc::new(Semaphore::new(READS_PER_CLASS))
        }))
    }
}

impl ReadPool {
    /// Waiters stay outside SQL capacity until their class is admitted. The
    /// owned permit follows the SQL job across cancellation and blocking work.
    pub(crate) async fn acquire_class(&self, class: ReadClass) -> OwnedSemaphorePermit {
        self.inner.classes.0[class as usize]
            .clone()
            .acquire_owned()
            .await
            .expect("read classes stay open")
    }
}
