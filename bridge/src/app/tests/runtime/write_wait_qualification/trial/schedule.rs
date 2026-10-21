//! Immutable offered-load construction and structural validation.
use super::*;

pub fn make_plan(name: TrialName, catalog: &ReadCatalog) -> Result<Vec<Slot>, String> {
    validate_catalog(catalog)?;
    let prefix = name.label();
    let mut plan = Vec::with_capacity(WRITES + READS + EXEMPTS);
    plan.extend((0..WRITES).map(|id| write_slot(&prefix, id)));
    append_reads(&mut plan, &prefix, catalog);
    plan.extend((0..EXEMPTS).map(|sequence| exempt_slot(&prefix, sequence)));
    plan.sort_by(|a, b| (a.due_us, a.traffic, a.sequence).cmp(&(b.due_us, b.traffic, b.sequence)));
    validate_plan(&plan)?;
    Ok(plan)
}
fn validate_catalog(catalog: &ReadCatalog) -> Result<(), String> {
    if catalog.rpc.len() != 45 || catalog.mcp.len() != 14 {
        return Err(format!(
            "read catalog changed: {} RPC, {} MCP; review denominator before running",
            catalog.rpc.len(),
            catalog.mcp.len()
        ));
    }
    if catalog.router_only_mcp.len() != 3
        || !catalog
            .router_only_mcp
            .iter()
            .all(|name| catalog.mcp.contains(name))
    {
        return Err("exact three Router-only MCP read recipes required".into());
    }
    for methods in [&catalog.rpc, &catalog.mcp] {
        if methods.iter().collect::<BTreeSet<_>>().len() != methods.len() {
            return Err("read catalog contains duplicate method names".into());
        }
    }
    // Cardinality NEVER implies Ready. Actual GlobalCase.form/McpCase form is
    // mapped by backend.recipe to expected_read_cohort, including task.doc and
    // MCP list_tasks LegacyFull with zero Ready quota but1000 real replies.
    Ok(())
}
fn write_caller(position: usize) -> usize {
    match position {
        0..8 => position / 2,
        8..32 => 4 + (position - 8) % 8,
        32..36 => position - 32,
        _ => position - 36,
    }
}
fn owner_for(caller: usize, second: usize, position: usize, burst: bool) -> usize {
    if burst {
        0
    } else if caller >= 4 {
        caller - 4
    } else {
        caller * 2 + (second + position) % 2
    }
}
fn finite_class(position: usize) -> (WriteClass, Option<u64>, Option<u64>) {
    match position {
        0..32 => (WriteClass::State, None, None),
        32..36 => (
            WriteClass::FilesystemProcess,
            Some([10_000, 25_000, 50_000, 150_000][position - 32]),
            Some(250_000),
        ),
        _ => (
            WriteClass::GitProvider,
            Some([25_000, 50_000, 100_000, 475_000][position - 36]),
            Some(500_000),
        ),
    }
}
fn write_slot(prefix: &str, id: usize) -> Slot {
    let second = id / 40;
    let position = id % 40;
    let caller = write_caller(position);
    let burst = second % 10 == 0 && position < 8;
    let (class, target, ceiling) = finite_class(position);
    Slot {
        original_id: format!("qual/{prefix}/write/{id:05}"),
        sequence: id,
        traffic: Traffic::Write,
        route: if caller < 4 {
            Route::SpaRpc
        } else {
            Route::AgentMcp
        },
        caller,
        owner: owner_for(caller, second, position, burst),
        due_us: second as u64 * 1_000_000 + if burst { 0 } else { position as u64 * 25_000 },
        class: Some(class),
        finite_target_us: target,
        finite_ceiling_us: ceiling,
        method: None,
    }
}
fn append_reads(plan: &mut Vec<Slot>, prefix: &str, catalog: &ReadCatalog) {
    for round in 0..1_000 {
        for (route, methods) in [
            (Route::SpaRpc, &catalog.rpc),
            (Route::AgentMcp, &catalog.mcp),
        ] {
            for (index, method) in methods.iter().enumerate() {
                plan.push(read_slot(prefix, round, index, route, method, catalog));
            }
        }
    }
}
fn read_slot(
    prefix: &str,
    round: usize,
    index: usize,
    route: Route,
    method: &str,
    catalog: &ReadCatalog,
) -> Slot {
    let sequence = round * 59
        + if route == Route::SpaRpc {
            index
        } else {
            45 + index
        };
    let caller = read_caller(route, index, method, catalog);
    let owner = if caller < 4 {
        caller * 2
    } else if caller == ROUTER_CALLER {
        0
    } else {
        caller - 4
    };
    Slot {
        original_id: format!("qual/{prefix}/read/{sequence:05}"),
        sequence,
        traffic: Traffic::Read,
        route,
        caller,
        owner,
        due_us: round as u64 * 600_000,
        class: None,
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some(method.into()),
    }
}
fn read_caller(route: Route, index: usize, method: &str, catalog: &ReadCatalog) -> usize {
    if route == Route::SpaRpc {
        index % 4
    } else if catalog.router_only_mcp.contains(method) {
        ROUTER_CALLER
    } else {
        4 + index % 8
    }
}
fn exempt_slot(prefix: &str, sequence: usize) -> Slot {
    let methods = [
        "term.input",
        "term.ack",
        "changes.subscribe",
        "changes.unsubscribe",
        "ping",
    ];
    Slot {
        original_id: format!("qual/{prefix}/exempt/{sequence:05}"),
        sequence,
        traffic: Traffic::Exempt,
        route: Route::SpaRpc,
        caller: sequence % 4,
        owner: (sequence % 4) * 2,
        due_us: sequence as u64 * 10_000,
        class: None,
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some(methods[sequence % 5].into()),
    }
}
struct PlanCounts {
    totals: BTreeMap<Traffic, usize>,
    seconds: Vec<[usize; 5]>,
    ids: BTreeSet<String>,
    bursts: BTreeMap<u64, [usize; CALLERS]>,
}
impl PlanCounts {
    fn add(&mut self, slot: &Slot) -> Result<(), String> {
        if !self.ids.insert(slot.original_id.clone())
            || slot.caller >= CALLERS
            || slot.owner >= 8
            || slot.due_us >= SECONDS * 1_000_000
        {
            return Err(format!("invalid or duplicate slot {}", slot.original_id));
        }
        *self.totals.entry(slot.traffic).or_insert(0) += 1;
        let second = (slot.due_us / 1_000_000) as usize;
        match slot.traffic {
            Traffic::Write => self.add_write(slot, second)?,
            Traffic::Exempt => self.seconds[second][4] += 1,
            Traffic::Read => {}
        }
        Ok(())
    }
    fn add_write(&mut self, slot: &Slot, second: usize) -> Result<(), String> {
        if slot.caller >= WRITE_CALLERS {
            return Err("Router cannot submit semantic writes".into());
        }
        self.seconds[second][class_index(slot)?] += 1;
        if second % 10 == 0 && slot.sequence % 40 < 8 {
            if slot.owner != 0 || slot.due_us != second as u64 * 1_000_000 {
                return Err("burst changed owner or timestamp".into());
            }
            self.bursts.entry(second as u64).or_insert([0; CALLERS])[slot.caller] += 1;
        }
        Ok(())
    }
    fn valid(&self) -> bool {
        let totals_ok = [
            (Traffic::Write, WRITES),
            (Traffic::Read, READS),
            (Traffic::Exempt, EXEMPTS),
        ]
        .iter()
        .all(|(traffic, expected)| self.totals.get(traffic) == Some(expected));
        totals_ok
            && self
                .seconds
                .iter()
                .all(|counts| *counts == [8, 24, 4, 4, 100])
            && self.bursts.len() == 60
            && self.bursts.values().all(burst_counts_valid)
    }
}
fn class_index(slot: &Slot) -> Result<usize, String> {
    match slot.class {
        Some(WriteClass::State) if slot.route == Route::SpaRpc => Ok(0),
        Some(WriteClass::State) => Ok(1),
        Some(WriteClass::FilesystemProcess) => Ok(2),
        Some(WriteClass::GitProvider) => Ok(3),
        None => Err("write missing class".into()),
    }
}
fn burst_counts_valid(counts: &[usize; CALLERS]) -> bool {
    counts.iter().sum::<usize>() == 8 && counts.iter().all(|n| *n <= 2)
}
fn validate_plan(plan: &[Slot]) -> Result<(), String> {
    let mut counts = PlanCounts {
        totals: BTreeMap::new(),
        seconds: vec![[0; 5]; SECONDS as usize],
        ids: BTreeSet::new(),
        bursts: BTreeMap::new(),
    };
    for slot in plan {
        counts.add(slot)?;
    }
    if !counts.valid() {
        return Err("exact offered schedule, per-second mix or burst contract changed".into());
    }
    Ok(())
}

/// Explicit NoQualification control only. Reuses the immutable normal write
/// and exempt slot constructors; never relaxes make_plan's45+14/600s gates.
pub fn make_pipeline_control_plan(
    name: TrialName,
    catalog: &ReadCatalog,
) -> Result<Vec<Slot>, String> {
    let expected = [
        "tasks.get",
        "tasks.waiting_on.blocks",
        "tasks.review.get",
        "tasks.review.diff",
        "tasks.attachment",
        "thread.attachment",
        "task.doc",
        "task.stage_doc",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    if catalog.rpc.iter().cloned().collect::<BTreeSet<_>>() != expected
        || catalog.rpc.len() != 8
        || !catalog.mcp.is_empty()
        || !catalog.router_only_mcp.is_empty()
    {
        return Err("NoQualification control requires exactly declared native8 RPC cases and zero MCP reads".into());
    }
    let prefix = name.label();
    let mut plan = (0..80)
        .map(|id| write_slot(&prefix, id))
        .collect::<Vec<_>>();
    plan.extend((0..200).map(|sequence| exempt_slot(&prefix, sequence)));
    for round in 0..4 {
        for (index, method) in catalog.rpc.iter().enumerate() {
            plan.push(read_slot(
                &prefix,
                round,
                index,
                Route::SpaRpc,
                method,
                catalog,
            ));
        }
    }
    plan.sort_by(|a, b| (a.due_us, a.traffic, a.sequence).cmp(&(b.due_us, b.traffic, b.sequence)));
    Ok(plan)
}
/// Complete semantic chain used to prepare deterministic FS revisions and Git
/// refs before selecting the control prefix. Not a normal qualification plan.
pub fn make_semantic_slots(name: TrialName) -> Vec<Slot> {
    let prefix = name.label();
    (0..WRITES).map(|id| write_slot(&prefix, id)).collect()
}
