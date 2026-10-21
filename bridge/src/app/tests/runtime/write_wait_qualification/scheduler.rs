use super::*;
use std::collections::BTreeSet;

/// Initial fixed-rate scaffold. Mixed classes and owner bursts are the next
/// implementation slice; tests below specify those missing obligations.
fn write_slots(seconds: usize) -> Vec<WriteSlot> {
    (0..seconds * WRITES_PER_SECOND)
        .map(|id| WriteSlot {
            id,
            due_micros: id as u64 * 25_000,
            caller: id % CALLERS,
            owner: id % ACTIVE_WORKSPACES,
            class: WorkClass::State,
        })
        .collect()
}

#[derive(Default)]
struct CallerCredits {
    pending: BTreeSet<usize>,
}

impl CallerCredits {
    fn admit(&mut self, slot: &WriteSlot) -> bool {
        // The starting scaffold tracks identities but has not enforced the
        // per-caller credit limit. Refused offers must remain in the plan.
        self.pending.insert(slot.id)
    }

    fn complete(&mut self, id: usize) {
        assert!(self.pending.remove(&id), "completion names admitted work");
    }
}

#[test]
fn intended_schedule_preserves_40_per_second_and_exact_80_10_10_mix() {
    let slots = write_slots(TRIAL_SECONDS);
    assert_eq!(slots.len(), 24_000);
    assert_eq!(slots.last().unwrap().due_micros, 599_975_000);
    assert!(slots
        .windows(2)
        .all(|pair| pair[0].due_micros <= pair[1].due_micros));
    let ids: BTreeSet<_> = slots.iter().map(|slot| slot.id).collect();
    assert_eq!(
        ids.len(),
        slots.len(),
        "every intended offer has a unique identity"
    );
    let mut per_second = vec![0; TRIAL_SECONDS];
    for slot in &slots {
        assert!(slot.caller < CALLERS);
        assert!(slot.owner < ACTIVE_WORKSPACES);
        let second = (slot.due_micros / 1_000_000) as usize;
        per_second[second] += 1;
    }
    assert!(per_second
        .into_iter()
        .all(|count| count == WRITES_PER_SECOND));
    for second in slots.chunks_exact(WRITES_PER_SECOND) {
        let counts = [
            WorkClass::State,
            WorkClass::FilesystemProcess,
            WorkClass::GitProvider,
        ]
        .map(|class| second.iter().filter(|slot| slot.class == class).count());
        assert_eq!(counts, [32, 4, 4], "each second offers the declared mix");
    }
    assert_eq!(WorkClass::FilesystemProcess.external_bound_ms(), 250);
    assert_eq!(WorkClass::GitProvider.external_bound_ms(), 500);
    assert_eq!(TRIAL_SECONDS * EXEMPT_PER_SECOND, 60_000);
}

#[test]
fn same_owner_burst_is_eight_offered_writes_without_increasing_the_rate() {
    let slots = write_slots(10);
    assert_eq!(
        slots.len(),
        400,
        "burst uses declared slots, not extra writes"
    );
    let burst = &slots[..8];
    assert!(burst.iter().all(|slot| slot.due_micros == 0));
    assert!(burst.iter().all(|slot| slot.owner == 0));
    let mut counts = [0; CALLERS];
    for slot in burst {
        counts[slot.caller] += 1;
    }
    assert!(counts.into_iter().all(|n| n <= MAX_OUTSTANDING_PER_CALLER));
    assert!(slots[8..40].iter().any(|slot| slot.owner != 0));
}

#[test]
fn exhausted_caller_credits_do_not_turn_an_offered_slot_into_a_completion() {
    let slots: Vec<_> = write_slots(1)
        .into_iter()
        .filter(|slot| slot.caller == 0)
        .collect();
    let mut credits = CallerCredits::default();
    assert!(credits.admit(&slots[0]));
    assert!(credits.admit(&slots[1]));
    assert!(
        !credits.admit(&slots[2]),
        "third offer waits for caller credit"
    );
    assert_eq!(credits.pending.len(), 2);
    credits.complete(slots[0].id);
    assert!(
        credits.admit(&slots[2]),
        "same offered identity can be dispatched later"
    );
    assert_eq!(credits.pending.len(), 2);
    assert_eq!(
        slots[2].id, 24,
        "credit delay does not rewrite offered identity"
    );
}
