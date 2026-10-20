use super::*;

pub(in crate::app::tests) mod agent_tabs;
mod claude_tui_compaction;
mod codex_tui_compaction;
pub(in crate::app::tests) mod compaction;
mod compaction_policy;
pub(in crate::app::tests) mod compaction_tools;
mod concurrency_timing;
pub(in crate::app::tests) mod delivery;
pub(in crate::app::tests) mod frame_locks;
pub(in crate::app::tests) mod idle_sessions;
mod observation_saturation;
pub(in crate::app::tests) mod off_the_workers;
mod pi_compaction;
pub(in crate::app::tests) mod run_agents;
mod scope_observations;
mod snapshot_publication;
mod sql_reads;
pub(in crate::app::tests) mod start_refused;
mod status_publication_timing;
pub(in crate::app::tests) mod terminal_lifecycle;
mod terminal_reaper;
pub(in crate::app::tests) mod terminals;

mod lock_hold_busy;
