//! Native seed components run before history fill and listener capture.
use crate::app::{tests::req, AppState};
use crate::encoding::b64encode;
use serde_json::{json, Value};

pub(super) const LABEL: &str = "qualification-read-witness";
pub(super) const COMMENT: &str = "qualification immutable comment with saved attachment";
pub(super) const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR qualification file witness";

pub(in crate::app::tests::runtime) struct TaskReviewFiles {
    pub project: String,
    pub workspace: String,
    pub owner: String,
    pub agent: String,
    pub task: String,
    pub dependent: String,
    pub comment: String,
    pub snapshot: String,
    pub directory: String,
    pub task_attachment: String,
    pub thread_attachment: String,
    pub blob_bytes: Vec<u8>,
    pub attachment_bytes: Vec<u8>,
}

fn result(app: &mut AppState, method: &str, params: Value) -> Value {
    let reply = app.handle(req(method, params));
    assert_eq!(reply["ok"], true, "native read seed {method}: {reply}");
    reply["result"].clone()
}

/// `task` and `dependent` are reserved existing rows in `project`, and `owner`
/// has a real canonical agent. `workspace` contains `source` and the immutable
/// file committed before calling this function. No common writer may delete
/// the reserved task/comment/review/blob witness or rewrite that file.
pub(super) fn seed_task_review_files(
    app: &mut AppState,
    context: &SeedContext<'_>,
    expected_blob: &[u8],
) -> TaskReviewFiles {
    let SeedContext {
        project,
        workspace,
        owner,
        agent,
        task,
        dependent,
        ..
    } = *context;
    result(
        app,
        "tasks.update",
        json!({"task_id":task,"labels":[LABEL]}),
    );
    result(
        app,
        "tasks.waiting_on.add",
        json!({"task_id":dependent,"waiting_on":[task]}),
    );
    let attachment = result(
        app,
        "tasks.attach",
        json!({
        "project_id":project,"filename":"qualification.png","content_b64":b64encode(IMAGE)}),
    );
    // Existing tasks cannot receive attachments via tasks.update. A native
    // immutable comment on the reserved existing task associates the descriptor.
    let commented = result(
        app,
        "tasks.comment",
        json!({
        "task_id":task,"body":COMMENT,"attachments":[attachment.clone()]}),
    );
    let comment = commented["comment"]["id"]
        .as_str()
        .expect("saved comment ID")
        .to_owned();
    let task_attachment = attachment["path"]
        .as_str()
        .expect("stored attachment path")
        .to_owned();
    let attached = result(
        app,
        "thread.attach",
        json!({
        "entity_id":owner,"agent_id":agent,"filename":"qualification.png",
        "content_b64":b64encode(IMAGE)}),
    );
    let thread_attachment = attached["path"]
        .as_str()
        .expect("thread attachment path")
        .to_owned();
    let saved = result(
        app,
        "tasks.review.snapshot",
        json!({
        "task_id":task,"workspace_id":workspace,"expected_version":0}),
    );
    let snapshot = saved["review"]["snapshots"]
        .as_array()
        .expect("saved snapshots")
        .last()
        .expect("native snapshot");
    let directory = &snapshot["directories"][0];
    assert_eq!(
        directory["status"], "git",
        "blob must be backed by real saved Git objects"
    );
    assert!(
        expected_blob.len() < 4096,
        "known text fits the real minimum body page"
    );
    TaskReviewFiles {
        project: project.into(),
        workspace: workspace.into(),
        owner: owner.into(),
        agent: agent.into(),
        task: task.into(),
        dependent: dependent.into(),
        comment,
        snapshot: snapshot["id"].as_str().unwrap().into(),
        directory: directory["id"].as_str().unwrap().into(),
        task_attachment,
        thread_attachment,
        blob_bytes: expected_blob.to_vec(),
        attachment_bytes: IMAGE.to_vec(),
    }
}

/// Inert legacy documents: native plan domain constructor + actual canonical
/// doc roots + finishers. Neither prepares an implementation workspace nor
/// queues a provider turn. Caller must add any resulting thread rows to the
/// SINGLE pinned total before generic filling, and report 2 supplemental plans.
pub(super) fn seed_legacy_documents(app: &mut AppState) -> (String, String, String) {
    let single = crate::app::tests::file_legacy_task(app, "Qualification ordinary document");
    let mut active = app.take_plan(&single).unwrap();
    active.stages.clear();
    active.plan_path = ".build/qualification.md".into();
    app.finish_plan_mutation(single.clone(), active).unwrap();
    let root = app.require_store().unwrap().plan_docs_dir(&single);
    std::fs::create_dir_all(root.join(".build")).unwrap();
    std::fs::write(
        root.join(".build/qualification.md"),
        b"# Ordinary qualification document\n",
    )
    .unwrap();

    let staged = crate::app::tests::file_legacy_task(app, "Qualification staged document");
    let stage = "qualification-stage".to_owned();
    let mut active = app.take_plan(&staged).unwrap();
    active.stages = vec![crate::plan::StageDoc {
        id: stage.clone(),
        title: "Qualification stage".into(),
        path: ".build/plan/qualification-stage.md".into(),
        summary: "Pinned read witness".into(),
        state: crate::plan::StageDocState::Approved,
    }];
    app.finish_plan_mutation(staged.clone(), active).unwrap();
    let root = app.require_store().unwrap().plan_docs_dir(&staged);
    std::fs::create_dir_all(root.join(".build/plan")).unwrap();
    std::fs::write(
        root.join(".build/plan/qualification-stage.md"),
        b"# Staged qualification document\n",
    )
    .unwrap();
    (single, staged, stage)
}

/// The caller reserves these existing rows before seeding. Generic history
/// filling follows this step and includes all actual seed items.
#[derive(Clone, Copy)]
pub(in crate::app::tests::runtime) struct SeedContext<'a> {
    pub project: &'a str,
    pub workspace: &'a str,
    pub owner: &'a str,
    pub agent: &'a str,
    pub task: &'a str,
    pub dependent: &'a str,
}

/// Supplemental objects are explicit. Every item in these canonical threads
/// participates in the common envelope; generic filling must subtract it.
#[derive(Debug)]
pub(in crate::app::tests::runtime) struct SeedLedger {
    pub added_tracker_tasks: usize,
    pub added_registry_workspaces: usize,
    pub legacy_plans: usize,
    pub project_owners: usize,
    pub passive_routers: usize,
    pub added_thread_items: usize,
}

pub(in crate::app::tests::runtime) struct SeededReadInputs {
    pub core: TaskReviewFiles,
    pub ordinary_plan: String,
    pub staged_plan: String,
    pub stage: String,
    pub project_owner: String,
    pub project_agent: String,
    pub router: String,
    pub capture: String,
    pub router_token: String,
    pub ledger: SeedLedger,
}

fn item_count(app: &AppState) -> usize {
    app.plans
        .values()
        .map(|p| &p.agents)
        .chain(app.runs.values().map(|r| &r.agents))
        .flat_map(|roster| roster.iter())
        .filter(|agent| agent.id == agent.conversation_id())
        .map(|agent| agent.thread.total_item_count() as usize)
        .sum()
}

impl SeededReadInputs {
    /// One real project read context. Caller reserves two existing task rows
    /// and a populated existing workspace conversation before calling this.
    /// Invoke the global legacy-doc seed only once in the shared fixture;
    /// this initial bounded constructor owns that seed as well.
    pub(in crate::app::tests::runtime) fn seed(
        app: &mut AppState,
        context: SeedContext<'_>,
    ) -> Self {
        let SeedContext {
            project, workspace, ..
        } = context;
        let before_items = item_count(app);
        // Tracker/workspace cardinalities are independently asserted by the caller.
        let directory = app
            .workspaces
            .get(workspace)
            .unwrap()
            .directories
            .first()
            .unwrap()
            .path
            .clone();
        let bytes = std::fs::read(directory.join("README.md"))
            .expect("fixture commits its known README before read seeding");
        let core = seed_task_review_files(app, &context, &bytes);
        let (ordinary_plan, staged_plan, stage) = seed_legacy_documents(app);
        let selected = result(
            app,
            "project.ensure_conversation",
            json!({"project_id":project}),
        );
        let project_owner = selected["run_id"].as_str().unwrap().to_owned();
        let project_agent = app
            .project_pin(&project_owner)
            .expect("native project coordinator pin")
            .to_owned();
        let (capture, router) = crate::app::tests::routing::router::captured(
            app,
            "Qualification immutable router read witness",
        );
        let router_token = format!("qualification-read-router-token-{router}");
        app.test_install_mcp_token(router.clone(), router_token.clone());
        let ledger = SeedLedger {
            added_tracker_tasks: 0,
            added_registry_workspaces: 0,
            legacy_plans: 2,
            project_owners: 1,
            passive_routers: 1,
            added_thread_items: item_count(app) - before_items,
        };
        Self {
            core,
            ordinary_plan,
            staged_plan,
            stage,
            project_owner,
            project_agent,
            router,
            capture,
            router_token,
            ledger,
        }
    }
}
