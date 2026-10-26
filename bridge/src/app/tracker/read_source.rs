//! Shared task queries: the SQL lane supplies one read transaction; legacy
//! synchronous mutation/MCP adapters retain the writer-backed Store surface.
use crate::reviews::records::ReviewSummary;
use crate::store::{
    DeliveryIntent, DependencySummary, DependentsPage, PersistedPlan, PersistedRun,
    ReadTransaction, Store, StoreError, TaskFilter, TaskFilterChoice, TaskSeek, TaskStretch,
    TaskSummaryCandidate,
};
use crate::tracker::{Task, TimelineEntry};
use serde_json::Value;
use std::collections::HashMap;

macro_rules! task_queries {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        pub(in crate::app) trait TaskReadSource {
            $(fn $name(&self, $($arg: $ty),*) -> Result<$out, StoreError>;)*
            fn list_filtered_tasks(
                &self, project: &str, filter: TaskFilter<'_>, seek: TaskSeek,
                keep: impl FnMut(&Task, Option<&Value>) -> bool,
            ) -> Result<TaskStretch, StoreError>;
        }
        impl TaskReadSource for Store {
            $(fn $name(&self, $($arg: $ty),*) -> Result<$out, StoreError> {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase(concat!("sql.", stringify!($name)));
                self.$name($($arg),*)
            })*
            fn list_filtered_tasks(
                &self, project: &str, filter: TaskFilter<'_>, seek: TaskSeek,
                keep: impl FnMut(&Task, Option<&Value>) -> bool,
            ) -> Result<TaskStretch, StoreError> {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase("sql.list_filtered_tasks");
                self.list_tracker_tasks_with_assignment_below(project, filter, seek, keep)
            }
        }
        impl TaskReadSource for ReadTransaction<'_> {
            $(fn $name(&self, $($arg: $ty),*) -> Result<$out, StoreError> {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase(concat!("sql.", stringify!($name)));
                self.$name($($arg),*)
            })*
            fn list_filtered_tasks(
                &self, project: &str, filter: TaskFilter<'_>, seek: TaskSeek,
                keep: impl FnMut(&Task, Option<&Value>) -> bool,
            ) -> Result<TaskStretch, StoreError> {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase("sql.list_filtered_tasks");
                self.list_tracker_tasks_with_assignment_below(project, filter, seek, keep)
            }
        }
    };
}

task_queries! {
    tracker_list_key() -> String;
    task_summary_identity(id: &str) -> Option<crate::tracker::TaskAgentIdentity>;
    list_task_summary_candidates(project: &str, below: Option<u64>, limit: usize) -> Vec<TaskSummaryCandidate>;
    list_task_filter_choices(project: &str, dimension: &str, after: Option<&str>, limit: usize) -> Vec<TaskFilterChoice>;
    load_tracker_task(id: &str) -> Option<Task>;
    load_tracker_timeline(id: &str) -> Vec<TimelineEntry>;
    load_tracker_timelines(ids: &[String]) -> HashMap<String, Vec<TimelineEntry>>;
    load_review_summary(id: &str) -> Option<ReviewSummary>;
    waiting_on_summary(project: &str, id: &str) -> DependencySummary;
    waiting_on_blocks(project: &str, id: &str, limit: usize, cursor: Option<&str>) -> DependentsPage;
    current_delivery_intent(id: &str) -> Option<DeliveryIntent>;
    load_all_run_rosters() -> Vec<PersistedRun>;
    load_all_plan_rosters() -> Vec<PersistedPlan>;
}
