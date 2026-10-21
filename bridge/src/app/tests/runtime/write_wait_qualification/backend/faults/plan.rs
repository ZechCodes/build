use super::*;
use trial_complete::{Route, Traffic, WriteClass};
/// First400 normal semantic slots, same IDs/classes/callers/owners/targets;
/// only intended due offsets are compressed tenfold. No semantic credit grows.
pub(in crate::app::tests::runtime::write_wait_qualification) fn overload_plan(
    name: TrialName,
) -> Result<Vec<Slot>, String> {
    let mut plan = trial_complete::make_semantic_slots(name)
        .into_iter()
        .take(400)
        .collect::<Vec<_>>();
    for slot in &mut plan {
        slot.due_us /= 10;
    }
    plan.sort_by_key(|slot| (slot.due_us, slot.traffic, slot.sequence));
    let mut counts = [0_usize; 4];
    let mut callers = std::collections::BTreeSet::new();
    for slot in &plan {
        check_slot(slot, &mut counts)?;
        callers.insert(slot.caller);
    }
    if counts != [80, 240, 40, 40] || callers.len() != 12 {
        return Err("overload immutable400-slot composition changed".into());
    }
    let burst = plan
        .iter()
        .filter(|slot| slot.due_us == 0)
        .collect::<Vec<_>>();
    if burst.len() != 8 || burst.iter().any(|slot| slot.owner != 0 || slot.caller >= 4) {
        return Err("original concurrent eight-write burst changed".into());
    }
    Ok(plan)
}
fn check_slot(slot: &Slot, counts: &mut [usize; 4]) -> Result<(), String> {
    if slot.traffic != Traffic::Write || slot.due_us >= 1_000_000 || slot.caller >= 12 {
        return Err("invalid400/s semantic intent".into());
    }
    let index = match (slot.class, slot.route) {
        (Some(WriteClass::State), Route::SpaRpc) => 0,
        (Some(WriteClass::State), Route::AgentMcp) => 1,
        (Some(WriteClass::FilesystemProcess), Route::SpaRpc) => 2,
        (Some(WriteClass::GitProvider), Route::SpaRpc) => 3,
        _ => return Err("unsupported400/s slot".into()),
    };
    counts[index] += 1;
    Ok(())
}
