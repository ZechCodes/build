use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Fixed offered load, independent of available caller credits.
fn write_slots(seconds: usize) -> Vec<WriteSlot> {
    (0..seconds * WRITES_PER_SECOND).map(write_slot).collect()
}

fn write_slot(id: usize) -> WriteSlot {
    let second = id / WRITES_PER_SECOND;
    let position = id % WRITES_PER_SECOND;
    let caller = match position {
        0..8 => position / 2,
        8..32 => SPA_CALLERS + (position - 8) % MCP_CALLERS,
        32..36 => position - 32,
        _ => position - 36,
    };
    let burst = second % 10 == 0 && position < 8;
    let owner = if burst {
        0
    } else if caller >= SPA_CALLERS {
        caller - SPA_CALLERS
    } else {
        caller * 2 + (second + position) % 2
    };
    let class = match position {
        0..32 => WorkClass::State,
        32..36 => WorkClass::FilesystemProcess,
        _ => WorkClass::GitProvider,
    };
    WriteSlot {
        id,
        caller,
        owner,
        class,
        due_micros: second as u64 * 1_000_000 + if burst { 0 } else { position as u64 * 25_000 },
    }
}

#[derive(Default)]
struct CallerCredits {
    pending: BTreeMap<usize, usize>,
    counts: [usize; CALLERS],
}

impl CallerCredits {
    fn admit(&mut self, slot: &WriteSlot) -> bool {
        assert!(
            !self.pending.contains_key(&slot.id),
            "original is already dispatched"
        );
        if self.counts[slot.caller] == MAX_OUTSTANDING_PER_CALLER {
            return false;
        }
        self.pending.insert(slot.id, slot.caller);
        self.counts[slot.caller] += 1;
        true
    }

    fn complete(&mut self, id: usize) {
        let caller = self
            .pending
            .remove(&id)
            .expect("completion names admitted work");
        self.counts[caller] -= 1;
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
    // Fixed original identities isolate credit retention from routing policy.
    let slots = [0, 12, 24].map(|id| WriteSlot {
        id,
        caller: 0,
        owner: 0,
        due_micros: id as u64 * 25_000,
        class: WorkClass::State,
    });
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
