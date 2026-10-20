use super::*;

pub(in crate::app::tests) mod agent_tabs;
mod claude_tui_compaction;
mod codex_tui_compaction;
pub(in crate::app::tests) mod compaction;
mod compaction_policy;
pub(in crate::app::tests) mod compaction_tools;
mod concurrency_timing;
mod sql_reads;
pub(in crate::app::tests) mod delivery;
pub(in crate::app::tests) mod frame_locks;
pub(in crate::app::tests) mod idle_sessions;
pub(in crate::app::tests) mod off_the_workers;
mod pi_compaction;
pub(in crate::app::tests) mod run_agents;
pub(in crate::app::tests) mod start_refused;
pub(in crate::app::tests) mod terminal_lifecycle;
pub(in crate::app::tests) mod terminals;
