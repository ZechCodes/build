//! Count actual index edits while independently checking all installed inputs.
use super::*;
use crate::app::observation_reads::capture_metrics;

fn native_root(index: usize) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = format!("root-{index}-").into_bytes();
        bytes.push(0xff);
        PathBuf::from("/scope-membership-fixture").join(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    PathBuf::from("scope-membership-fixture").join(format!("root-{index}"))
}

fn fixture() -> ScopeReadContext {
    let mut patch = ScopePatch::default();
    for index in 0..8 {
        let id = format!("workspace-{index}");
        let root = native_root(index);
        patch.workspaces.insert(
            id.clone(),
            Some(Arc::new(Workspace {
                id: id.clone(),
                project_id: "project".into(),
                name: id.clone(),
                root: root.clone(),
                status: crate::workspace::WorkspaceStatus::Ready,
                archived_at: None,
                directories: Vec::new(),
                isolation: Default::default(),
                managed: true,
                created_by_agent: false,
                locked: false,
            })),
        );
        patch.workspace_roots.insert(id, Some(root));
    }
    for index in 0..32 {
        let root = native_root(index % 8);
        patch.runs.insert(
            format!("run-{index:02}"),
            Some(Arc::new(RunInput {
                plan_id: Some(format!("plan-{}", index % 4)),
                created_at: format!("created-{index:02}"),
                workspace_id: Some(format!("workspace-{}", index % 8)),
                checkout_git: true,
                canonical_checkout: root.clone(),
                project_id: Some("project".into()),
                project_path: native_root(99),
                checkout: root.clone(),
                git_root: root,
                base_sha: Some(format!("base-{index}")),
                base_branch: "main".into(),
                stages: vec![StageProgress::dispatched(format!("stage-{index}"))],
                terminal: false,
            })),
        );
    }
    patch.order = Some((0..32).map(|index| format!("run-{index:02}")).collect());
    let mut context = ScopeReadContext::default();
    context.apply(&patch);
    context
}

/// Overlay only authoritative rows, then derive indices from the entire final
/// input set. This never invokes apply or its incremental membership helper.
fn full_oracle(before: &ScopeReadContext, patch: &ScopePatch) -> ScopeReadContext {
    let mut expected = before.clone();
    for (id, row) in &patch.runs {
        match row {
            Some(row) => {
                expected.runs.insert(id.clone(), row.clone());
            }
            None => {
                expected.runs.remove(id);
            }
        }
    }
    for (id, row) in &patch.workspaces {
        match row {
            Some(row) => {
                expected.workspaces.insert(id.clone(), row.clone());
            }
            None => {
                expected.workspaces.remove(id);
            }
        }
    }
    for (id, root) in &patch.workspace_roots {
        match root {
            Some(root) => {
                expected
                    .workspace_canonical
                    .insert(id.clone(), root.clone());
            }
            None => {
                expected.workspace_canonical.remove(id);
            }
        }
    }
    rebuild_indices(&mut expected, before);
    expected
}

fn rebuild_indices(expected: &mut ScopeReadContext, before: &ScopeReadContext) {
    let mut memberships: BTreeMap<String, BTreeSet<String>> = before
        .workspace_dependents
        .keys()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    let mut lineages: BTreeMap<String, Vec<String>> = before
        .lineages
        .keys()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    for (id, run) in &expected.runs {
        if let Some(workspace) = &run.workspace_id {
            memberships
                .entry(workspace.clone())
                .or_default()
                .insert(id.clone());
        }
        if let Some(plan) = &run.plan_id {
            lineages.entry(plan.clone()).or_default().push(id.clone());
        }
    }
    for ids in lineages.values_mut() {
        ids.sort_by_key(|id| {
            (
                &expected.runs[id].created_at,
                expected.order.get(id).copied().unwrap_or_default(),
            )
        });
    }
    expected.workspace_dependents = memberships
        .into_iter()
        .map(|(id, owners)| (id, owners.into_iter().collect()))
        .collect();
    expected.lineages = lineages
        .into_iter()
        .map(|(id, ids)| (id, Arc::new(ids)))
        .collect();
    let mut roots: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for (id, root) in &expected.workspace_canonical {
        roots.entry(root.clone()).or_default().insert(id.clone());
    }
    expected.workspace_roots = roots
        .into_iter()
        .map(|(root, ids)| (root, ids.into_iter().collect()))
        .collect();
}

fn assert_context(actual: &ScopeReadContext, expected: &ScopeReadContext) {
    assert_eq!(actual.runs.len(), expected.runs.len());
    for (id, row) in &expected.runs {
        assert!(
            Arc::ptr_eq(&actual.runs[id], row),
            "entire RunInput must install for {id}"
        );
    }
    assert_eq!(actual.workspaces.len(), expected.workspaces.len());
    for (id, row) in &expected.workspaces {
        assert!(
            Arc::ptr_eq(&actual.workspaces[id], row),
            "entire workspace must install for {id}"
        );
    }
    assert!(actual.projects.ptr_eq(&expected.projects));
    assert_eq!(actual.plan_projects, expected.plan_projects);
    assert_eq!(actual.lineages, expected.lineages);
    assert_eq!(actual.order, expected.order);
    assert_eq!(actual.workspace_dependents, expected.workspace_dependents);
    assert_eq!(actual.workspace_roots, expected.workspace_roots);
    assert_eq!(actual.workspace_canonical, expected.workspace_canonical);
}

fn measured_apply(context: &mut ScopeReadContext, patch: &ScopePatch) -> (usize, usize) {
    capture_metrics::take();
    capture_metrics::start();
    context.apply(patch);
    let visits = capture_metrics::take();
    let count = |wanted| {
        visits
            .iter()
            .filter(|((family, _), _)| *family == wanted)
            .map(|(_, count)| count)
            .sum()
    };
    (
        count("scope_membership_remove"),
        count("scope_membership_insert"),
    )
}

#[test]
fn unchanged_workspace_owner_patches_do_not_rewrite_membership() {
    let mut context = fixture();
    let before = context.clone();
    let retained = context.clone();
    let mut patch = ScopePatch::default();
    for index in (0..3).flat_map(|workspace| (0..4).map(move |rank| workspace + rank * 8)) {
        let id = format!("run-{index:02}");
        let mut row = (*before.runs[&id]).clone();
        row.checkout_git = false;
        row.canonical_checkout = native_root(100 + index);
        row.checkout = row.canonical_checkout.clone();
        row.git_root = native_root(200 + index);
        row.project_path = native_root(300 + index);
        row.base_sha = Some(format!("new-base-{index}"));
        row.base_branch = format!("new-branch-{index}");
        row.stages
            .push(StageProgress::dispatched(format!("new-stage-{index}")));
        row.terminal = true;
        if index == 0 {
            row.plan_id = Some("changed-plan".into());
            row.created_at = "changed-created".into();
        }
        patch.runs.insert(id, Some(Arc::new(row)));
    }
    let mut workspace = (*before.workspaces["workspace-0"]).clone();
    workspace.root = native_root(400);
    patch
        .workspace_roots
        .insert(workspace.id.clone(), Some(workspace.root.clone()));
    patch
        .workspaces
        .insert(workspace.id.clone(), Some(Arc::new(workspace)));
    let expected = full_oracle(&before, &patch);
    let work = measured_apply(&mut context, &patch);
    // Semantic, lineage, native-root and old-root checks precede work bounds.
    assert_context(&context, &expected);
    assert_context(&retained, &before);
    assert_eq!(context.workspace_canonical["workspace-0"], native_root(400));
    #[cfg(unix)]
    assert!(context.workspace_canonical["workspace-0"]
        .to_str()
        .is_none());
    assert_eq!(patch.runs.len(), 12);
    assert_eq!(work, (0, 0), "same membership rewrote its index");
}

#[test]
fn workspace_membership_moves_and_tombstones_keep_legacy_empty_sets() {
    let cases = [
        (None, Some(Some("workspace-1")), Some((0, 1))),
        (Some("workspace-0"), Some(None), Some((1, 0))),
        (Some("workspace-0"), Some(Some("workspace-1")), Some((1, 1))),
        (Some("exclusive"), None, Some((1, 0))),
        (Some("workspace-0"), Some(Some("workspace-0")), None),
        (None, Some(None), None),
    ];
    for (from, to, expected_work) in cases {
        let mut context = fixture();
        let mut row = (*context.runs["run-00"]).clone();
        row.workspace_id = from.map(str::to_owned);
        let mut initial = ScopePatch {
            runs: BTreeMap::from([("run-00".into(), Some(Arc::new(row.clone())))]),
            ..Default::default()
        };
        if from == Some("exclusive") {
            let mut workspace = (*context.workspaces["workspace-0"]).clone();
            workspace.id = "exclusive".into();
            workspace.root = native_root(500);
            initial
                .workspace_roots
                .insert(workspace.id.clone(), Some(workspace.root.clone()));
            initial
                .workspaces
                .insert(workspace.id.clone(), Some(Arc::new(workspace)));
        }
        context.apply(&initial);
        let before = context.clone();
        let retained = context.clone();
        let next = to.map(|workspace| {
            row.workspace_id = workspace.map(str::to_owned);
            Arc::new(row)
        });
        let patch = ScopePatch {
            runs: BTreeMap::from([("run-00".into(), next)]),
            ..Default::default()
        };
        let expected = full_oracle(&before, &patch);
        let work = measured_apply(&mut context, &patch);
        assert_context(&context, &expected);
        assert_context(&retained, &before);
        assert!(context.workspace_dependents["workspace-0"].contains("run-08"));
        if from == Some("exclusive") {
            assert!(context.workspace_dependents["exclusive"].is_empty());
        }
        if let Some(expected_work) = expected_work {
            assert_eq!(work, expected_work, "from {from:?} to {to:?}");
        }
    }
}
