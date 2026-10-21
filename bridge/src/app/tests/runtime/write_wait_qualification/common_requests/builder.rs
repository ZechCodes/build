//! Immutable mapping state; only FS/Git create actual dependency chains.
use super::*;
#[path = "builder/finite.rs"]
mod finite;
#[path = "builder/mcp.rs"]
mod mcp;
#[path = "builder/spa.rs"]
mod spa;
#[path = "builder/validation.rs"]
mod validation;

struct Builder<'a> {
    trial: TrialName,
    targets: &'a CommonTargets,
    last_file: [Option<String>; 8],
    last_git: [Option<String>; 8],
    file_revision: [String; 8],
    git_sequence: [usize; 8],
    task_status: [usize; 8],
}
struct SlotContext<'a> {
    slot: &'a Slot,
    target: &'a ActiveTarget,
    task: String,
}
struct Parts {
    request: ActualRequest,
    effect: ExpectedEffect,
    durable_operation_id: Option<String>,
    predecessor: Option<String>,
}

pub fn build_common_requests(
    trial: TrialName,
    targets: &CommonTargets,
    immutable_plan: &[Slot],
) -> Result<BTreeMap<String, CommonRequest>, String> {
    validation::validate_targets(targets)?;
    let mut writes = immutable_plan
        .iter()
        .filter(|slot| slot.traffic == Traffic::Write)
        .collect::<Vec<_>>();
    writes.sort_by_key(|slot| (slot.due_us, slot.sequence));
    if writes.len() != 24_000 {
        return Err("exact24000 immutable semantic slots required".into());
    }
    let mut builder = Builder::new(trial, targets);
    let mut mapped = BTreeMap::new();
    for slot in writes {
        let item = builder.map_slot(slot)?;
        let id = slot.original_id.clone();
        if mapped.insert(id.clone(), item).is_some() {
            return Err(format!("duplicate original {id}"));
        }
    }
    validation::validate_predecessors(&mapped)?;
    Ok(mapped)
}
impl<'a> Builder<'a> {
    fn new(trial: TrialName, targets: &'a CommonTargets) -> Self {
        Self {
            trial,
            targets,
            last_file: std::array::from_fn(|_| None),
            last_git: std::array::from_fn(|_| None),
            file_revision: std::array::from_fn(|i| sha256(&targets.initial_editor_bodies[i])),
            git_sequence: [0; 8],
            task_status: [0; 8],
        }
    }
    fn map_slot(&mut self, slot: &Slot) -> Result<CommonRequest, String> {
        validation::validate_slot(slot)?;
        let targets = self.targets;
        let context = SlotContext {
            slot,
            target: &targets.active[slot.owner],
            task: task_for_owner(targets, slot.owner),
        };
        let parts = match (slot.class, slot.route) {
            (Some(WriteClass::State), Route::SpaRpc) => self.spa(&context)?,
            (Some(WriteClass::State), Route::AgentMcp) => self.mcp(&context)?,
            (Some(WriteClass::FilesystemProcess), Route::SpaRpc) => self.file(&context)?,
            (Some(WriteClass::GitProvider), Route::SpaRpc) => self.git(&context)?,
            _ => {
                return Err(format!(
                    "unsupported exact common slot {}",
                    slot.original_id
                ))
            }
        };
        Ok(CommonRequest {
            original_id: slot.original_id.clone(),
            caller: slot.caller,
            owner: slot.owner,
            request: parts.request,
            effect: parts.effect,
            durable_operation_id: parts.durable_operation_id,
            predecessor: parts.predecessor,
        })
    }
}
