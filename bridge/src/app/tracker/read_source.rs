//! Shared task queries: the SQL lane supplies one read transaction; legacy
//! synchronous mutation/MCP adapters retain the writer-backed Store surface.
use crate::reviews::records::ReviewSummary;
use crate::store::{
    DeliveryIntent, DependencySummary, DependentsPage, PersistedPlan, PersistedRun,
    ReadTransaction, Store, StoreError, TaskFilter, TaskFilterChoice, TaskQueryFilter,
    TaskQueryOrder, TaskQueryPosition, TaskSeek, TaskStretch, TaskSummaryCandidate,
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
    task_summary_by_number(project: &str, number: u64) -> Option<TaskSummaryCandidate>;
    task_counts(project: &str, agents: &[String]) -> Vec<crate::store::TaskCounts>;
    tracker_list_key() -> String;
    task_query_basis() -> String;
    task_query_markers_unsupported(project: &str) -> bool;
    task_query_unsupported_domain(project: &str, order: TaskQueryOrder, done_since: bool) -> Option<String>;
    list_task_query_candidates(project: &str, order: TaskQueryOrder, after: Option<TaskQueryPosition>, limit: usize) -> Vec<(TaskSummaryCandidate, TaskQueryPosition)>;
    task_query_matches(id: &str, filter: &TaskQueryFilter) -> bool;
    task_summary_identity_with_owner(id: &str) -> Option<(crate::tracker::TaskAgentIdentity, Option<String>)>;
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
