//! Workflow orchestration facade for plans, runs, reporting, and workspace lifecycle.
mod plans;
mod reporting;
mod runs;
mod workspace;

pub use plans::{ActivePlan, PlanWorkspace};
pub use reporting::{AgentTurn, OrchestratorError, ReportOutcome};
pub use runs::{ActiveRun, ImplementableIssue, PreparedImplementation, RunSource};
pub(crate) use workspace::{
    append_durable_conversation, conversation_prompt, AgentLaunch, PreparedAgentLaunch,
    CHECKOUT_REAP_WAIT, HARNESS_READY_GRACE, PROMPT_WRITE_EXIT_GRACE,
};
pub use workspace::{
    gate_plan_message, gate_plan_stage_notes, mcp_config_path, AdoptableCheckout, Agent,
    Orchestrator, ResumeIdProbe, SessionLocatorFactory, SpawnOptions, TranscriptProbe, WarmBuilder,
    CATCH_UP_MESSAGES, NEW_THREAD_MESSAGES_PROMPT,
};

#[cfg(test)]
mod tests;
