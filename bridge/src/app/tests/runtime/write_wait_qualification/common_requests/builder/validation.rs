use super::*;
pub(super) fn validate_targets(targets: &CommonTargets) -> Result<(), String> {
    let mut refs = BTreeSet::new();
    let mut agents = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for (owner, target) in targets.active.iter().enumerate() {
        validate_identity(target, &mut agents, &mut directories)?;
        validate_git(&targets.git[owner], &mut refs)?;
    }
    for project in 0..4 {
        validate_project(targets, project)?;
    }
    Ok(())
}
fn validate_identity<'a>(
    target: &'a ActiveTarget,
    agents: &mut BTreeSet<&'a String>,
    directories: &mut BTreeSet<&'a PathBuf>,
) -> Result<(), String> {
    if target.project >= 4
        || target.owner.is_empty()
        || target.agent.is_empty()
        || target.thread.is_empty()
        || target.conversation.is_empty()
        || target.workspace.is_empty()
        || target.source.is_empty()
        || !target.path.is_absolute()
        || !agents.insert(&target.agent)
        || !directories.insert(&target.path)
    {
        return Err("eight distinct real native targets required".into());
    }
    Ok(())
}
fn validate_git<'a>(git: &'a RootGitRefs, refs: &mut BTreeSet<&'a String>) -> Result<(), String> {
    if git.initial_commit.is_empty()
        || git.initial_head_ref.is_empty()
        || git.full_refs.iter().any(|reference| {
            !reference.starts_with("refs/heads/")
                || reference == &git.initial_head_ref
                || !refs.insert(reference)
        })
    {
        return Err("sixteen unique precreated branches with original commit and distinct starting HEAD required".into());
    }
    Ok(())
}
fn validate_project(targets: &CommonTargets, project: usize) -> Result<(), String> {
    if targets
        .active
        .iter()
        .filter(|target| target.project == project)
        .count()
        != 2
    {
        return Err("two active owners per project required".into());
    }
    let cards = &targets.projects[project].ordinary_tasks;
    if targets.projects[project].project_id.is_empty()
        || cards[0] == cards[1]
        || cards
            .iter()
            .any(|task| task.is_empty() || targets.reserved_read_tasks.contains(task))
    {
        return Err(
            "two ordinary existing tasks/project must exclude reserved read witnesses".into(),
        );
    }
    Ok(())
}

pub(super) fn validate_slot(slot: &Slot) -> Result<(), String> {
    let id = &slot.original_id;
    let owner = slot.owner;
    if owner >= 8 || slot.caller >= 12 {
        return Err(format!("invalid write caller/owner for {id}"));
    }
    if slot.route == Route::SpaRpc && slot.caller >= 4 {
        return Err("SPA common request must use one of four actual openings".into());
    }
    if slot.route == Route::AgentMcp && (slot.caller < 4 || slot.caller - 4 != owner) {
        return Err("Coding caller cannot switch to another owner's identity".into());
    }
    Ok(())
}
pub(super) fn validate_predecessors(
    mapped: &BTreeMap<String, CommonRequest>,
) -> Result<(), String> {
    // All eight owner0 burst calls remain concurrent; only per-file FS and
    // per-root Git predecessors exist. No state oracle convenience ordering.
    if mapped.values().any(|request| {
        request.predecessor.is_some()
            && !matches!(
                request.effect,
                ExpectedEffect::File { .. } | ExpectedEffect::GitHead { .. }
            )
    }) {
        return Err("unexpected state predecessor serialization".into());
    }
    Ok(())
}
