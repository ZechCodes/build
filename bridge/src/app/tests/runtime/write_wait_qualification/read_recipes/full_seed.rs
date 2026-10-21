//! Native prerequisites for complete independent read cases.
//! Keep the existing bounded seed/native9 control as an independent prefix.
use super::seed::{seed_task_review_files, SeedContext, SeededReadInputs, TaskReviewFiles};
use super::{McpExpected, McpReadInputs};
use crate::app::tests::runtime::write_wait_qualification::fixture::{
    ProjectContext, WorkspaceContext,
};
use crate::app::tests::runtime::write_wait_qualification::rpc::RpcHarness;
use crate::app::tests::*;
use std::path::Path;
use std::sync::Arc;

mod git;
mod provider;
mod qa;
mod services;
mod thread;

pub(in crate::app::tests::runtime::write_wait_qualification) struct ProjectReadInputs {
    pub core: Arc<TaskReviewFiles>,
    pub project_owner: String,
    pub project_agent: String,
}

pub(in crate::app::tests::runtime::write_wait_qualification) struct PreparedReadInputs {
    pub bounded: SeededReadInputs,
    pub projects: Vec<ProjectReadInputs>,
    read_scope: WorkspaceContext,
    archive_scope: WorkspaceContext,
    active: Vec<WorkspaceContext>,
    all_projects: Vec<String>,
    resident_owners: Vec<String>,
    git: git::GitSeed,
    thread: thread::ThreadSeed,
    services: services::Services,
    router: provider::RouterBoot,
    task_title: String,
    columns: Value,
    default_harness: Value,
}

pub(in crate::app::tests::runtime::write_wait_qualification) struct ReadyReadInputs {
    pub projects: Vec<ProjectReadInputs>,
    pub witness: Arc<super::rpc_full::Witnesses>,
    router: provider::ReadyRouter,
    auxiliary: qa::ReadyAuxiliary,
    _router_boot: provider::RouterBoot,
    services: services::Services,
    cleanup: ReadCleanup,
    external_path: PathBuf,
    external_name: String,
    repository: PathBuf,
    expected_registry_residents: usize,
    expected_history_items: u64,
}

fn canonical_counts(app: &AppState) -> std::collections::BTreeMap<String, u64> {
    let mut values = std::collections::BTreeMap::new();
    for owner in app.plans.keys().chain(app.runs.keys()) {
        for agent in app.entity_agents(owner).unwrap().iter() {
            let address = app
                .resolve_conversation_address(owner, Some(&agent.id))
                .unwrap();
            let count = app.conversation_at(&address).unwrap().total_item_count();
            let canonical = agent.conversation_id().to_owned();
            if let Some(previous) = values.insert(canonical, count) {
                assert_eq!(
                    previous, count,
                    "aliases must resolve to the same counted canonical history"
                );
            }
        }
    }
    values
}
fn canonical_items(app: &AppState) -> u64 {
    canonical_counts(app).values().sum()
}

impl PreparedReadInputs {
    /// Input bounded is project0's existing native core/legacy2/router fixture.
    /// Additional three cores use existing last2 task IDs and active0 workspace
    /// per project. Native project pins are metadata owners, not auth endpoints.
    pub(in crate::app::tests::runtime::write_wait_qualification) fn prepare(
        app: &mut AppState,
        root: &Path,
        bounded: SeededReadInputs,
        projects: &[ProjectContext],
        residents: &[WorkspaceContext],
        active: &[WorkspaceContext],
    ) -> Self {
        assert_eq!(projects.len(), 4);
        assert_eq!(active.len(), 8);
        let read_scope = residents
            .iter()
            .filter(|w| w.project == 0)
            .nth(2)
            .unwrap()
            .clone();
        let archive_scope = residents
            .iter()
            .filter(|w| w.project == 1)
            .nth(2)
            .unwrap()
            .clone();
        assert!(!active
            .iter()
            .any(|w| w.owner == read_scope.owner || w.owner == archive_scope.owner));
        let services = services::seed_services(root);
        services.install_into(app);
        let mut reads = vec![ProjectReadInputs {
            core: Arc::new(bounded.core.clone()),
            project_owner: bounded.project_owner.clone(),
            project_agent: bounded.project_agent.clone(),
        }];
        for (index, project) in projects.iter().enumerate().skip(1) {
            let workspace = active.iter().find(|w| w.project == index).unwrap();
            let count = project.tasks.len();
            let context = SeedContext {
                project: &project.project_id,
                workspace: &workspace.workspace_id,
                owner: &workspace.owner,
                agent: &workspace.agent_id,
                task: &project.tasks[count - 2],
                dependent: &project.tasks[count - 1],
            };
            let blob = std::fs::read(workspace.directory.join("README.md")).unwrap();
            let core = Arc::new(seed_task_review_files(app, &context, &blob));
            let reply = app.handle(req(
                "project.ensure_conversation",
                json!({"project_id":project.project_id}),
            ));
            assert_eq!(reply["ok"], true, "{reply}");
            let project_owner = reply["result"]["run_id"].as_str().unwrap().to_owned();
            let project_agent = app.project_pin(&project_owner).unwrap().to_owned();
            reads.push(ProjectReadInputs {
                core,
                project_owner,
                project_agent,
            });
        }
        // Read-only metadata rosters are intentionally not provider writers.
        // A normal task notification must not boot any of these four owners.
        // Their native pins/agents remain present for populated roster reads.
        for project in &reads {
            let mut owner = app.take_run(&project.project_owner).unwrap();
            for agent in owner.agents.iter_mut() {
                agent.watched = false;
            }
            app.finish_run_mutation(project.project_owner.clone(), owner)
                .unwrap();
        }
        let git = git::seed(
            app,
            &read_scope,
            &projects[0].project_id,
            &bounded.staged_plan,
            &bounded.stage,
            root,
        );
        let thread = thread::seed(app, &read_scope.owner, &read_scope.agent_id);
        assert!(
            thread.item_count_after > thread.item_count_before,
            "native thread extras must be budgeted"
        );
        // Give this actual thread its own native attachment copy as well as the
        // durable content-addressed copy. No arbitrary cross-owner file fallback.
        let attached = app.handle(req(
            "thread.attach",
            json!({"entity_id":read_scope.owner,
            "agent_id":read_scope.agent_id,"filename":"qualification.png",
            "content_b64":b64encode(super::seed::IMAGE)}),
        ));
        assert_eq!(attached["ok"], true, "{attached}");
        assert_eq!(attached["result"]["path"], bounded.core.thread_attachment);
        qa::prepare_shell(app, root);
        let router = provider::RouterBoot::prepare(app, root, &bounded.capture, &bounded.router);
        let task_title = app
            .tracker_store()
            .unwrap()
            .load_tracker_task(&bounded.core.task)
            .unwrap()
            .unwrap()
            .title;
        let columns = app
            .tasks_columns(&json!({"project_id":projects[0].project_id}))
            .unwrap()["columns"]
            .clone();
        let default_harness = json!(app.default_harness);
        Self {
            bounded,
            projects: reads,
            read_scope,
            archive_scope,
            active: active.to_vec(),
            all_projects: projects.iter().map(|p| p.project_id.clone()).collect(),
            resident_owners: residents.iter().map(|w| w.owner.clone()).collect(),
            git,
            thread,
            services,
            router,
            task_title,
            columns,
            default_harness,
        }
    }

    /// Generic fill uses total_target - this count. Tail markers are reserved
    /// INSIDE the single pinned total, not appended after filling it.
    pub(in crate::app::tests::runtime::write_wait_qualification) fn reserved_tail_items(
        &self,
    ) -> usize {
        self.active.len()
    }

    pub(in crate::app::tests::runtime::write_wait_qualification) fn finish_history(
        &self,
        app: &mut AppState,
        total_target: u64,
    ) {
        assert_eq!(
            canonical_items(app) + self.reserved_tail_items() as u64,
            total_target
        );
        for workspace in &self.active {
            let mut active = app.take_run(&workspace.owner).unwrap();
            let agent = active.agents.by_id_mut(&workspace.agent_id).unwrap();
            agent
                .thread
                .post_user(thread::MESSAGE, None, "2026-10-21T00:00:00Z");
            agent.thread.read_unread("2026-10-21T00:00:00Z");
            app.finish_run_mutation(workspace.owner.clone(), active)
                .unwrap();
        }
        let mut archived = app.take_run(&self.archive_scope.owner).unwrap();
        archived.run.state = crate::run::RunState::Archived;
        // Historical read fixture: no checkout removal/lifecycle command claim.
        app.finish_run_mutation(self.archive_scope.owner.clone(), archived)
            .unwrap();
        assert!(git2::Repository::open(&self.archive_scope.directory).is_ok());
        assert_eq!(canonical_items(app), total_target);
        let mut expected = self
            .resident_owners
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        expected.extend(self.projects.iter().map(|p| p.project_owner.clone()));
        expected.insert(self.bounded.ordinary_plan.clone());
        expected.insert(self.bounded.staged_plan.clone());
        let actual = app
            .plans
            .keys()
            .chain(app.runs.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            actual, expected,
            "all canonical histories must be ledgered, Router has no canonical thread"
        );
        assert_eq!(actual.len(), self.resident_owners.len() + 4 + 2);
        let expected_heads = actual
            .iter()
            .map(|owner| {
                let roster = app.entity_agents(owner).unwrap();
                assert_eq!(
                    roster.len(),
                    1,
                    "fixture history ledger expects one native canonical head per owner"
                );
                roster.primary().unwrap().conversation_id().to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        let counted = canonical_counts(app);
        assert_eq!(
            counted
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            expected_heads
        );
        assert_eq!(counted.len(), self.resident_owners.len() + 4 + 2);
    }

    /// Caller retains the same RpcHarness/listeners through this setup and the
    /// trial. None of these setup operations may append canonical thread rows.
    pub(in crate::app::tests::runtime::write_wait_qualification) async fn finalize(
        self,
        rpc: &RpcHarness,
    ) -> ReadyReadInputs {
        let cleanup = ReadCleanup::new(
            rpc.state.clone(),
            self.bounded.capture.clone(),
            self.read_scope.directory.clone(),
        );
        let count_before = canonical_items(&rpc.state.lock().unwrap());
        self.services
            .updates
            .check()
            .await
            .expect("finite fake update metadata check");
        let router = self.router.await_ready(&rpc.state).await;
        let auxiliary = qa::finalize(
            rpc,
            &self.read_scope.workspace_id,
            &self.read_scope.source_id,
        )
        .await;
        assert_eq!(
            canonical_items(&rpc.state.lock().unwrap()),
            count_before,
            "post-fill bootstrap appended canonical items"
        );
        let i = super::rpc_full::Inputs {
            project: self.projects[0].core.project.clone(),
            source: self.read_scope.source_id.clone(),
            repository: self.read_scope.directory.clone(),
            workspace: self.read_scope.workspace_id.clone(),
            owner: self.read_scope.owner.clone(),
            agent: self.read_scope.agent_id.clone(),
            task: self.bounded.core.task.clone(),
            dependent: self.bounded.core.dependent.clone(),
            label: super::seed::LABEL.into(),
            operation: self.thread.operation.clone(),
            activity_from: self.thread.from,
            activity_through: self.thread.through,
            revision: self.thread.revision.clone(),
            capture: self.bounded.capture.clone(),
            stream: auxiliary.stream.clone(),
            ordinary_plan: self.bounded.ordinary_plan.clone(),
            staged_plan: self.bounded.staged_plan.clone(),
            stage: self.bounded.stage.clone(),
            stage_run: self.read_scope.owner.clone(),
            archived_run: self.archive_scope.owner.clone(),
            external_worktree: self.git.worktree.clone(),
            commit: self.git.head.clone(),
            stable_file: "README.md".into(),
            diff_file: self.git.diff_file.clone(),
            snapshot: self.bounded.core.snapshot.clone(),
            review_directory: self.bounded.core.directory.clone(),
            task_attachment: self.bounded.core.task_attachment.clone(),
            thread_attachment: self.bounded.core.thread_attachment.clone(),
        };
        let witness = super::rpc_full::Witnesses {
            inputs: i,
            all_projects: self.all_projects,
            task_title: self.task_title,
            comment: self.bounded.core.comment.clone(),
            comment_body: super::seed::COMMENT.into(),
            operation: self.thread.operation_value,
            thread_id: self.thread.thread_id,
            thread_generation: self.thread.generation,
            message_body: thread::MESSAGE.into(),
            activity_summary: thread::ACTIVITY.into(),
            revision_contents: thread::REVISION.into(),
            capture_text: "Qualification immutable router read witness".into(),
            ordinary_doc: "# Ordinary qualification document\n".into(),
            staged_doc: "# Staged qualification document\n".into(),
            stage_start: self.git.base,
            stage_end: self.git.head,
            commit_subject: "Qualification immutable read boundary".into(),
            committed_file: self.git.committed_file,
            diff_needle: self.git.diff_needle,
            branch_ref: self.git.branch_ref,
            upstream_ref: self.git.upstream_ref,
            file_bytes: std::fs::read(self.read_scope.directory.join("README.md")).unwrap(),
            review_bytes: self.bounded.core.blob_bytes.clone(),
            attachment_bytes: self.bounded.core.attachment_bytes.clone(),
            shell_id: auxiliary.shell.clone(),
            model_id: "qualification-model".into(),
            installed_harness: "claude".into(),
            missing_harness: "pi".into(),
            default_harness: self.default_harness,
            expected_columns: self.columns,
            archived_state: "archived".into(),
        };
        ReadyReadInputs {
            projects: self.projects,
            witness: Arc::new(witness),
            router,
            auxiliary,
            _router_boot: self.router,
            services: self.services,
            cleanup,
            external_path: self.git.external_path,
            external_name: self.git.external_name,
            repository: self.read_scope.directory,
            expected_registry_residents: self.resident_owners.len(),
            expected_history_items: count_before,
        }
    }
}

impl ReadyReadInputs {
    pub(in crate::app::tests::runtime::write_wait_qualification) fn assert_registry_residents(
        &self,
        rpc: &RpcHarness,
    ) {
        assert_eq!(rpc.state.lock().unwrap().workspaces.list(None).len(),self.expected_registry_residents,
            "read/adoption must not inflate registry residents, including support external worktree");
    }
    /// Before the semantic write trial, or in the isolated read-only smoke.
    /// Actual trial writes may append real messages; never erase or subtract
    /// them to preserve the starting history footprint.
    pub(in crate::app::tests::runtime::write_wait_qualification) fn assert_pinned_footprint(
        &self,
        rpc: &RpcHarness,
    ) {
        let app = rpc.state.lock().unwrap();
        assert_eq!(app.workspaces.list(None).len(),self.expected_registry_residents,
            "read/adoption must not inflate registry residents, including support external worktree");
        assert_eq!(
            canonical_items(&app),
            self.expected_history_items,
            "read/bootstrap must not silently add canonical history rows"
        );
    }
    pub(in crate::app::tests::runtime::write_wait_qualification) fn router_endpoint(
        &self,
    ) -> (&str, &str) {
        (&self.router.agent, &self.router.token)
    }
    pub(in crate::app::tests::runtime::write_wait_qualification) fn rpc_cases(
        &self,
    ) -> Vec<super::rpc_full::Case> {
        let cases = super::rpc_full::cases(&self.witness.inputs);
        super::rpc_full::assert_registry(&cases);
        cases
    }
    pub(in crate::app::tests::runtime::write_wait_qualification) fn assert_rpc_reply(
        &self,
        case: &super::rpc_full::Case,
        reply: &Value,
    ) -> super::rpc_full::Outcome {
        super::rpc_full::assert_reply(case, reply, &self.witness)
    }
    pub(in crate::app::tests::runtime::write_wait_qualification) fn mcp_for(
        &self,
        workspace: &WorkspaceContext,
    ) -> (McpReadInputs, McpExpected) {
        let project = &self.projects[workspace.project];
        let inputs = McpReadInputs {
            owner: workspace.owner.clone(),
            agent: workspace.agent_id.clone(),
            workspace: workspace.workspace_id.clone(),
            task: project.core.task.clone(),
            comment: project.core.comment.clone(),
            snapshot: project.core.snapshot.clone(),
            directory: project.core.directory.clone(),
            path: "README.md".into(),
            search_text: thread::MESSAGE.into(),
        };
        let expected = McpExpected {
            project: project.core.project.clone(),
            workspace: workspace.workspace_id.clone(),
            workspace_agent: workspace.agent_id.clone(),
            project_agent: project.project_agent.clone(),
            task: project.core.task.clone(),
            dependent: project.core.dependent.clone(),
            comment: project.core.comment.clone(),
            comment_body: super::seed::COMMENT.into(),
            snapshot: project.core.snapshot.clone(),
            blob: project.core.blob_bytes.clone(),
            search_text: thread::MESSAGE.into(),
            harness: "claude".into(),
        };
        (inputs, expected)
    }
}

impl ReadyReadInputs {
    /// Run before RpcHarness::finish and before the temporary fixture root is
    /// dropped. Trial completions are already settled; these are bootstrap
    /// cleanup operations, outside the qualification request populations.
    pub(in crate::app::tests::runtime::write_wait_qualification) async fn shutdown(
        &self,
        rpc: &RpcHarness,
    ) {
        self.assert_registry_residents(rpc);
        let before = canonical_items(&rpc.state.lock().unwrap());
        self.auxiliary
            .shutdown(
                rpc,
                &self.witness.inputs.workspace,
                &self.witness.inputs.source,
            )
            .await;
        self._router_boot.shutdown(&rpc.state, &self.router).await;
        self.services.assert_isolated(true);
        // All trial reads are settled before pruning this nonresident support
        // checkout. No registry/owner mutation or canonical row is involved.
        let path = self.external_path.clone();
        let name = self.external_name.clone();
        let repository = self.repository.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::remove_dir_all(&path).unwrap();
            let repo = git2::Repository::open(repository).unwrap();
            let worktree = repo.find_worktree(&name).unwrap();
            let mut options = git2::WorktreePruneOptions::new();
            options.valid(true).working_tree(true);
            worktree.prune(Some(&mut options)).unwrap();
            assert!(!path.exists());
            assert!(repo.find_worktree(&name).is_err());
        })
        .await
        .expect("support checkout cleanup joined");
        assert_eq!(
            canonical_items(&rpc.state.lock().unwrap()),
            before,
            "read teardown appended canonical history"
        );
        self.cleanup.disarm();
    }

    pub(in crate::app::tests::runtime::write_wait_qualification) fn manifest(&self) -> Value {
        json!({"read_rpc_methods":45,"mcp_read_methods":14,
            "supplemental_project_metadata_owners":self.projects.len(),
            "supplemental_nonresident_legacy_plans":2,
            "supplemental_authenticated_router_endpoints":1,
            "supplemental_authenticated_project_endpoints":0,
            "router_actual_factory_opens":self.router.factory_opens,
            "native_qa_streams":1,"native_finite_shells":1,
            "persisted_completed_failure_operation_receipts":1,
            "nonresident_support_git_worktrees":1,"artifact_revisions":1,
            "tracker_tasks_added":0,"registry_workspaces_added":0,
            "all_canonical_seed_items_inside_pinned_total":true,
            "reserved_active_tail_items":8,
            "provider_network_probes":"isolated fixture factory/CLIs/backends"})
    }
}

// Panic cleanup is independent of success-path assertions. Native retire_tab
// receipts are waited outside the app guard. The outer common fixture retains
// its existing all-session guard as well; this does not replace it.
struct ReadCleanup {
    state: std::sync::Weak<Mutex<AppState>>,
    capture: String,
    read_root: PathBuf,
    done: std::sync::atomic::AtomicBool,
}
impl ReadCleanup {
    fn new(state: Arc<Mutex<AppState>>, capture: String, read_root: PathBuf) -> Self {
        Self {
            state: Arc::downgrade(&state),
            capture,
            read_root,
            done: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn disarm(&self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Drop for ReadCleanup {
    fn drop(&mut self) {
        if self.done.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let (retired, router) = {
            let mut app = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let router = app.router_sessions.get(&self.capture).and_then(|router| {
                let scratch = AppState::canonical_root(router.scratch_dir());
                app.session_registry
                    .test_tab(&TabKey::agent(&scratch, router.agent_id()))
                    .map(|tab| tab.session.clone())
            });
            app.abandon_router_session(&self.capture);
            let keys = app
                .session_registry
                .test_tabs()
                .filter(|(key, tab)| {
                    key.root == self.read_root && matches!(tab.role, TabRole::Shell)
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let retired = keys
                .iter()
                .filter_map(|key| app.retire_tab(key, "qualification_read_fixture_cleanup"))
                .collect::<Vec<_>>();
            (retired, router)
        };
        // Do not panic in Drop while another assertion is unwinding.
        for retirement in retired {
            if !retirement.wait(Duration::from_secs(2)) {
                eprintln!("qualification fixture finite shell reap failed during cleanup");
            }
        }
        if let Some(router) = router {
            router.end();
        }
    }
}
