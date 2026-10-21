use super::*;
use crate::api::effects::{effect, Effect};
use std::collections::BTreeMap;

#[derive(Debug)]
struct AuxiliarySlot {
    due_micros: u64,
    caller: usize,
    method: &'static str,
}

fn read_methods() -> Vec<&'static str> {
    crate::api::v1::methods()
        .iter()
        .map(|(name, _)| *name)
        .chain(crate::api::LEGACY_METHODS.iter().copied())
        .chain(crate::api::QA_METHODS.iter().copied())
        .filter(|name| {
            matches!(
                effect(name),
                Some(
                    Effect::SqlRead
                        | Effect::SnapshotRead
                        | Effect::ExternalRead
                        | Effect::LegacyRead
                )
            )
        })
        .collect()
}

fn read_slots() -> Vec<AuxiliarySlot> {
    let methods = read_methods();
    let mut slots: Vec<_> = (0..1000)
        .flat_map(|round| {
            methods
                .iter()
                .enumerate()
                .map(move |(index, method)| AuxiliarySlot {
                    due_micros: round * 600_000,
                    caller: index % SPA_CALLERS,
                    method,
                })
        })
        .collect();
    slots.sort_by_key(|slot| slot.due_micros);
    slots
}

fn exempt_slots() -> Vec<AuxiliarySlot> {
    const METHODS: [&str; 5] = [
        "term.input",
        "term.ack",
        "changes.subscribe",
        "changes.unsubscribe",
        "ping",
    ];
    (0..TRIAL_SECONDS * EXEMPT_PER_SECOND)
        .map(|index| AuxiliarySlot {
            due_micros: index as u64 * 10_000,
            caller: index % SPA_CALLERS,
            method: METHODS[index % METHODS.len()],
        })
        .collect()
}

#[test]
fn every_read_method_is_offered_1000_times_without_blending_pending_or_legacy_results() {
    let methods = read_methods();
    assert!(methods.contains(&"tasks.list"));
    assert!(methods.contains(&"workspace.get"));
    assert!(methods.contains(&"git.show"));
    let slots = read_slots();
    let mut counts = BTreeMap::new();
    for slot in &slots {
        *counts.entry(slot.method).or_insert(0) += 1;
        assert!(slot.caller < SPA_CALLERS);
        assert!(slot.due_micros < TRIAL_SECONDS as u64 * 1_000_000);
    }
    for method in methods {
        assert_eq!(counts.get(method).copied().unwrap_or(0), 1000, "{method}");
    }
    // Successful bounded values, explicit pending and legacy unbounded answers
    // need separate completion cohorts; offers alone never establish p99.
}

#[test]
fn exempt_load_offers_100_frames_each_second_on_real_terminal_subscription_control_paths() {
    let slots = exempt_slots();
    assert_eq!(slots.len(), 60_000);
    let mut per_second = vec![0; TRIAL_SECONDS];
    let mut methods = BTreeMap::new();
    for slot in slots {
        per_second[(slot.due_micros / 1_000_000) as usize] += 1;
        *methods.entry(slot.method).or_insert(0) += 1;
        assert!(slot.caller < SPA_CALLERS);
    }
    assert!(per_second
        .into_iter()
        .all(|count| count == EXEMPT_PER_SECOND));
    for method in [
        "term.input",
        "term.ack",
        "changes.subscribe",
        "changes.unsubscribe",
        "ping",
    ] {
        assert!(methods.contains_key(method), "missing exempt path {method}");
    }
}
