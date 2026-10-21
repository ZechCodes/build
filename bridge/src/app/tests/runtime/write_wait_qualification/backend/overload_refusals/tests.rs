//! Native terminal/protocol accounting controls; no physical400 claim.
use super::super::completed_origins::CompletedOrigins;
use super::*;
use test_support::*;
fn finish(f: &NativeFixture) -> Result<Arc<VerifiedRefusals>, String> {
    f.audit.finish(
        &f.registry.snapshot(),
        &f.registry.completed_intervals(),
        &f.observe,
        Some(&f.accepted),
    )
}
#[test]
fn refusal_partition_preserves_strict_normal_completion_and_exact_zero_busy() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    let row = f.registry.snapshot().pop().unwrap();
    let normal =
        CompletedOrigins::new([(f.slot.original_id.clone(), f.raw.method.clone())]).unwrap();
    assert!(normal
        .import(&row)
        .unwrap_err()
        .contains("cancelled_native_completion"));
    let separate =
        CompletedOrigins::new([(f.slot.original_id.clone(), f.raw.method.clone())]).unwrap();
    let token =
        finish(&f).expect("closed cancellation plus complete final negative acceptance proof");
    assert!(separate.audit_overload(&token).is_empty());
    assert!(
        separate
            .audit()
            .join(";")
            .contains("missing_native_completion"),
        "normal audit remains strict"
    );
    assert!(
        !normal.audit_overload(&token).is_empty(),
        "already imported failure cannot be erased"
    );
    assert_eq!(
        token.originals().collect::<Vec<_>>(),
        [f.slot.original_id.as_str()]
    );
    let zero = verified_zero_refusals();
    assert_eq!(zero.token.originals().count(), 0);
}
#[test]
fn refusal_partition_rejects_actual_ack_or_executed_native_service() {
    crate::git_fixture::environment::isolated_git_test!();
    for (service, ack) in [(true, false), (false, true)] {
        let f = native_fixture_with(service, ack);
        assert!(finish(&f).is_err());
        assert!(f.audit.verified().is_none());
    }
}
#[test]
fn refusal_partition_rejects_missing_acceptance_and_late_duplicate_native_root() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    assert!(f
        .audit
        .finish(
            &f.registry.snapshot(),
            &f.registry.completed_intervals(),
            &f.observe,
            None
        )
        .is_err());
    let f = native_fixture();
    let duplicate = f
        .registry
        .begin(&f.slot.original_id, &f.raw.method, f.registry.now());
    duplicate.admitted();
    duplicate.cancel();
    assert!(finish(&f).unwrap_err().contains("duplicate final native"));
    assert!(f.audit.verified().is_none());
}
#[test]
fn refusal_partition_rejects_unbound_nonbusy_duplicate_and_mcp_candidates() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    f.audit
        .candidate(
            &f.slot,
            &f.raw.method,
            &f.raw,
            f.candidate.clone(),
            f.intended,
        )
        .unwrap();
    assert!(finish(&f)
        .unwrap_err()
        .contains("duplicate refusal candidates"));
    let f = native_fixture();
    f.audit
        .state
        .lock()
        .unwrap()
        .candidates
        .get_mut(&f.slot.original_id)
        .unwrap()[0]
        .refusal
        .decoded_response["retryable"] = serde_json::json!(false);
    assert!(finish(&f).is_err());
    let mut mcp = f.slot.clone();
    mcp.route = Route::AgentMcp;
    assert!(RefusalAudit::new([(mcp, f.raw.method.clone())]).is_err());
}
#[test]
fn refusal_partition_rejects_missing_or_replayed_cursor_candidate() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    f.audit.state.lock().unwrap().quarantined.clear();
    assert!(finish(&f)
        .unwrap_err()
        .contains("lacks actual terminal cursor"));
    let f = native_fixture();
    let row = f.registry.snapshot().pop().unwrap();
    assert!(f.audit.quarantine(&row));
    assert!(finish(&f)
        .unwrap_err()
        .contains("duplicate cancelled cursor"));
}
#[test]
fn refusal_partition_does_not_exclude_unknown_or_accepted_native_roots() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    let unknown = f
        .registry
        .begin("unknown-refusal", "tasks.update", f.registry.now());
    unknown.admitted();
    unknown.cancel();
    let accepted = f
        .registry
        .begin("accepted-unrelated", "tasks.update", f.registry.now());
    accepted.admitted();
    accepted.acknowledged();
    accepted.complete();
    for row in f
        .registry
        .snapshot()
        .iter()
        .filter(|row| row.original_id != f.slot.original_id)
    {
        assert!(!f.audit.quarantine(row));
    }
    let token = finish(&f).unwrap();
    assert_eq!(token.originals().count(), 1);
    assert!(token.get("unknown-refusal").is_none());
    assert!(token.get("accepted-unrelated").is_none());
}

#[test]
fn refusal_partition_rejects_missing_foreign_and_divergent_raw_reply_identity() {
    crate::git_fixture::environment::isolated_git_test!();
    for id in [
        serde_json::Value::Null,
        serde_json::json!("foreign-original"),
    ] {
        let f = native_fixture();
        {
            let mut state = f.audit.state.lock().unwrap();
            let stored = &mut state.candidates.get_mut(&f.slot.original_id).unwrap()[0];
            stored.raw["id"] = id;
        }
        assert!(finish(&f).unwrap_err().contains("raw busy identity"));
    }
    let f = native_fixture();
    f.audit
        .state
        .lock()
        .unwrap()
        .candidates
        .get_mut(&f.slot.original_id)
        .unwrap()[0]
        .refusal
        .decoded_response["different"] = serde_json::json!(true);
    assert!(finish(&f).unwrap_err().contains("raw busy identity"));
}

#[test]
fn refusal_partition_revokes_final_capability_on_late_or_repeated_audit() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    finish(&f).unwrap();
    let row = f.registry.snapshot().pop().unwrap();
    assert!(f.audit.quarantine(&row));
    assert!(f.audit.verified().is_none());
    let f = native_fixture();
    finish(&f).unwrap();
    assert!(finish(&f).is_err());
    assert!(f.audit.verified().is_none());
}

#[test]
fn refusal_partition_cannot_confirm_parser_only_or_changed_final_slot() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    // The actual cancellation/raw parser inputs exist, but no final proof yet.
    assert!(f.audit.confirm(&f.slot, &f.raw.method).is_err());
    finish(&f).unwrap();
    f.audit.confirm(&f.slot, &f.raw.method).unwrap();
    let mut changed = f.slot.clone();
    changed.owner += 1;
    assert!(f.audit.confirm(&changed, &f.raw.method).is_err());
    assert!(f.audit.confirm(&f.slot, "foreign.method").is_err());
    let row = f.registry.snapshot().pop().unwrap();
    assert!(f.audit.quarantine(&row));
    assert!(f.audit.confirm(&f.slot, &f.raw.method).is_err());
}

#[test]
fn refusal_partition_terminal_lookup_uses_only_actual_cursor_candidates() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = native_fixture();
    let rows = f.audit.terminal_rows(&f.slot.original_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, f.registry.snapshot()[0].id);
    assert!(f.audit.terminal_rows("unknown-original").is_empty());
    let unobserved = RefusalAudit::new([(f.slot.clone(), f.raw.method.clone())]).unwrap();
    assert!(
        unobserved.terminal_rows(&f.slot.original_id).is_empty(),
        "native state alone cannot replace the missing bounded cursor event"
    );
}
