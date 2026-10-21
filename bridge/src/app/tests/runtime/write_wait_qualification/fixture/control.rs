//! A reduced real pipeline fixture; never normal qualification evidence.
use super::*;

impl QualificationFixture {
    /// First eight native RPC cases only. The capture-only ninth case is not
    /// selected: cancel its real setup reservation before Shared so this
    /// control neither depends on nor bypasses the normal Router boot gate.
    pub(in crate::app::tests::runtime::write_wait_qualification) async fn pipeline_control(
    ) -> (Self, super::super::read_recipes::SeededReadInputs) {
        let mut native = NativeFixture::prepare(Envelope::Small, true);
        super::super::read_recipes::prepare_finite_shell(&mut native.app, native.root.path());
        let project = &native.projects[0];
        let workspace = &native.active_workspaces[0];
        let inputs = super::super::read_recipes::SeededReadInputs::seed(
            &mut native.app,
            super::super::read_recipes::SeedContext {
                project: &project.project_id,
                workspace: &workspace.workspace_id,
                owner: &workspace.owner,
                agent: &workspace.agent_id,
                task: &project.tasks[project.tasks.len() - 2],
                dependent: &project.tasks[project.tasks.len() - 1],
            },
        );
        // Read-only metadata is present without starting a ninth provider.
        let mut project_owner = native.app.take_run(&inputs.project_owner).unwrap();
        for agent in project_owner.agents.iter_mut() {
            agent.watched = false;
        }
        native
            .app
            .finish_run_mutation(inputs.project_owner.clone(), project_owner)
            .unwrap();
        let scratch = native.app.router_sessions[&inputs.capture]
            .scratch_dir()
            .to_path_buf();
        let cancelled = native
            .app
            .handle(req("capture.cancel", json!({"capture_id":inputs.capture})));
        assert_eq!(cancelled["ok"], true, "{cancelled}");
        assert_eq!(cancelled["result"]["cancelled"], true);
        assert!(!native.app.captures.contains_key(&inputs.capture));
        assert!(!native.app.router_sessions.contains_key(&inputs.capture));
        assert!(native
            .app
            .session_registry
            .test_token(&inputs.router)
            .is_none());
        wait_cancelled_scratch(&scratch).await;
        super::super::history_fill::fill(&mut native.app, Envelope::Small.thread_items());
        let observed_shape = control_shape(&native);
        let mut fixture = native.finish();
        fixture.pipeline_control_setup = Some(json!({
            "qualification":false,"seconds":2,"semantic_intents":80,"read_intents":32,
            "exempt_intents":200,"all_client_intents":312,"coding_endpoints":8,
            "router_endpoints":0,"selected_native_rpc_prefix":8,
            "excluded_rpc_case":"capture.get","setup_capture_cancel_reply":cancelled,
            "setup_router_token_revoked":true,"setup_router_scratch_removed":true,
            "metadata_project_owners":inputs.ledger.project_owners,"nonresident_legacy_plans":inputs.ledger.legacy_plans,
            "private_exempt_shells":1,"observed_native_shape":observed_shape,
        }));
        (fixture, inputs)
    }
}

async fn wait_cancelled_scratch(scratch: &std::path::Path) {
    let parent = scratch.parent().unwrap();
    let prefix = format!(
        "{}{}",
        scratch.file_name().unwrap().to_string_lossy(),
        crate::reaper::RETIRING_DIR_MARK
    );
    let until = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let retiring = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix));
        if !scratch.exists() && !retiring {
            break;
        }
        assert!(
            std::time::Instant::now() < until,
            "cancelled control setup router did not retire"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn control_shape(native: &NativeFixture) -> Value {
    let mut canonical = std::collections::BTreeMap::new();
    for run in native.app.runs.values() {
        for agent in run.agents.iter() {
            canonical.insert(
                agent.conversation_id().to_string(),
                agent.thread.total_item_count(),
            );
        }
    }
    for plan in native.app.plans.values() {
        for agent in plan.agents.iter() {
            canonical.insert(
                agent.conversation_id().to_string(),
                agent.thread.total_item_count(),
            );
        }
    }
    let task_counts = native
        .projects
        .iter()
        .map(|project| {
            native
                .app
                .tracker_store()
                .unwrap()
                .list_tracker_tasks(project.repository.to_str().unwrap(), Default::default())
                .unwrap()
                .len()
        })
        .collect::<Vec<_>>();
    let residents = native.app.workspaces.list(None).len();
    let thread_items = canonical.values().sum::<u64>();
    assert_eq!(task_counts, vec![25; 4]);
    assert_eq!(residents, 10);
    assert_eq!(native.active_workspaces.len(), 8);
    assert_eq!(thread_items, 1000);
    json!({"projects":native.projects.len(),"task_counts":task_counts,"registry_residents":residents,"active_workspaces":native.active_workspaces.len(),"canonical_histories":canonical.len(),"thread_items":thread_items})
}
