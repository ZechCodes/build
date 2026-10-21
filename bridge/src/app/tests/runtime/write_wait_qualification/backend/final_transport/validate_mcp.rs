use super::super::super::super::mcp_adapter::{
    McpCallRow, McpHarness, SocketEvidence, ROUTER_ENDPOINT,
};
use super::*;

pub(super) fn validate(
    expected: &BTreeMap<&str, &Slot>,
    report: &McpShutdownReport,
    errors: &mut Vec<String>,
) {
    if !report.quiet || !report.unfinished.is_empty() {
        errors.push(format!(
            "mcp_worker_join: quiet={} unfinished={:?}",
            report.quiet, report.unfinished
        ));
    }
    errors.extend(
        report
            .evidence
            .errors
            .iter()
            .map(|error| format!("mcp_transport_error: {error}")),
    );
    let sockets = validate_sockets(&report.evidence.sockets, errors);
    let mut seen = BTreeSet::new();
    for (key, call) in &report.evidence.calls {
        if key != &call.original_id || !seen.insert(call.original_id.as_str()) {
            errors.push(format!(
                "duplicate_or_mismatched_mcp_original: key={key}, original={}",
                call.original_id
            ));
        }
        let planned = expected
            .get(call.original_id.as_str())
            .copied()
            .filter(|slot| slot.route == Route::AgentMcp);
        match planned {
            Some(slot) => validate_binding(slot, call, errors),
            None => errors.push(format!("unexpected_mcp_original: {}", call.original_id)),
        }
        if !sockets.contains(&call.endpoint) {
            errors.push(format!(
                "missing_mcp_endpoint: {} endpoint={}",
                call.original_id, call.endpoint
            ));
        }
        validate_call(call, errors);
    }
    errors.extend(
        expected
            .values()
            .filter(|slot| {
                slot.route == Route::AgentMcp && !seen.contains(slot.original_id.as_str())
            })
            .map(|slot| format!("missing_mcp_original: {}", slot.original_id)),
    );
}

fn validate_sockets(rows: &[SocketEvidence], errors: &mut Vec<String>) -> BTreeSet<usize> {
    let mut endpoints = BTreeSet::new();
    let mut agents = BTreeSet::new();
    for row in rows {
        if !endpoints.insert(row.endpoint)
            || !agents.insert(row.agent.as_str())
            || row.agent.is_empty()
        {
            errors.push(format!(
                "duplicate_or_missing_mcp_endpoint: {}",
                row.endpoint
            ));
        }
        if row.endpoint > ROUTER_ENDPOINT || row.passive_router != (row.endpoint == ROUTER_ENDPOINT)
        {
            errors.push(format!("mcp_endpoint_role: {}", row.endpoint));
        }
        if row.limit != 2 || row.maximum_active > 2 || row.active != 0 {
            errors.push(format!(
                "mcp_socket_limit: endpoint={} active={} maximum={} limit={}",
                row.endpoint, row.active, row.maximum_active, row.limit
            ));
        }
    }
    endpoints
}

fn validate_binding(slot: &Slot, call: &McpCallRow, errors: &mut Vec<String>) {
    if call.read != (slot.traffic == Traffic::Read) || slot.traffic == Traffic::Exempt {
        errors.push(format!("mcp_cohort_mismatch: {}", call.original_id));
    }
    if McpHarness::endpoint_for_caller(slot.caller).ok() != Some(call.endpoint) {
        errors.push(format!("mcp_endpoint_mismatch: {}", call.original_id));
    }
    // Write slots name their tool through the immutable CommonRequest, checked
    // by the native completion consumer. Declared read methods bind here too.
    if call.method.is_empty()
        || slot
            .method
            .as_ref()
            .is_some_and(|method| method != &call.method)
    {
        errors.push(format!("mcp_method_mismatch: {}", call.original_id));
    }
}

fn validate_call(call: &McpCallRow, errors: &mut Vec<String>) {
    let joined = [
        call.client_closed_at,
        call.server_joined_at,
        call.worker_finished_at,
        call.worker_reaped_at,
    ];
    if joined.iter().any(Option::is_none) || call.abandoned {
        errors.push(format!("mcp_worker_join: {}", call.original_id));
    }
    let transport = [
        call.socket_wait_started_at,
        call.socket_acquired_at,
        call.sent_at,
        call.final_reply_at,
    ];
    let response = call.response_ok == Some(true)
        && call.response_bytes.is_some_and(|bytes| bytes > 0)
        && call.response_sha256.as_ref().is_some_and(|hash| {
            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
    if transport.iter().any(Option::is_none) || !response || call.error.is_some() {
        errors.push(format!("mcp_call_lifecycle: {}", call.original_id));
    }
    // actual_wire_id is optional native evidence; never invent or require an echo.
}
