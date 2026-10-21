//! Empty TEST-FIRST seam. Do not implement until the controls establish RED.
use super::*;

pub(super) fn validate_final_transport(
    _plan: &[Slot],
    _reconcile: Option<&FinalReconciliation>,
    _report: Option<&WatchReport>,
    _mcp: &McpShutdownReport,
    _native_joined: bool,
) -> Vec<String> {
    vec!["transport lifecycle audit TESTFIRST pending".into()]
}
