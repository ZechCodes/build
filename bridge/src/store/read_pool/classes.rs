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
// Three task pages plus two task details and two history reads leave at least
// one SQL reader for other classes. This is not a reservation against all types.
const READS_PER_CLASS: usize = READ_CONNECTIONS / 4;
const TASK_LIST_READS: usize = 3;
const _: () = assert!(READS_PER_CLASS > 0);
const _: () = assert!(TASK_LIST_READS + 2 * READS_PER_CLASS < READ_CONNECTIONS);

pub(super) struct ReadQuotas([Arc<Semaphore>; CLASSES]);

impl ReadQuotas {
    pub(super) fn new() -> Self {
        Self(std::array::from_fn(|index| {
            let permits = if index == ReadClass::TaskList as usize {
                TASK_LIST_READS
            } else {
                READS_PER_CLASS
            };
            Arc::new(Semaphore::new(permits))
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
