//! A real plan Complete consumes scratch stage documents over the MCP socket.
use super::super::finite_provider::FiniteProvider;
use super::*;
use crate::app::tests::support::{file_legacy_task, primary_agent_id, qa_state};
use crate::app::AppState;
use crate::git_fixture::init_repo;
use crate::plan::{PlanState, StageDocState, StageManifestEntry};
use crate::templates::STAGES_MANIFEST_PATH;

const TOKEN: &str = "qualification-plan-report-capability";
const SUMMARY: &str = "Qualification staged plan is ready for review";
const DOCUMENTS: [(&str, &str, &str); 2] = [
    (
        "first-stage",
        ".build/plan/01-first-stage.md",
        "# First stage\n\nImplement the first reviewed change.\n",
    ),
    (
        "second-stage",
        ".build/plan/02-second-stage.md",
        "# Second stage\n\nValidate the second reviewed change.\n",
    ),
];

struct DraftingPlan {
    id: String,
    caller: String,
    manifest: String,
}

pub(super) async fn execute() -> VerifiedReport {
    let (root, repo) = init_repo();
    let provider = FiniteProvider::install(root.path());
    provider.require_enrolled();
    let mut app = qa_state(&repo, root.path());
    let drafted = prepare(&mut app);
    let fixture = RareFixture::new("mcp-plan-report", app);
    let proof = fixture
        .report_mcp(
            &drafted.caller,
            TOKEN,
            DoneReport::new(DoneStatus::Completed, SUMMARY),
        )
        .await;
    let effect = assert_completed(&fixture, &drafted);
    assert_eq!(
        provider.opens.load(Ordering::SeqCst),
        0,
        "the report needs no provider start"
    );
    assert!(
        provider.log.turns().is_empty(),
        "no queued setup turn may mask report behavior"
    );
    let report = fixture.verify_report(proof, effect).await;
    assert!(
        fixture.finish().await.is_empty(),
        "a report is separate from action successes"
    );
    report
}

fn prepare(app: &mut AppState) -> DraftingPlan {
    let id = file_legacy_task(app, "Qualification staged planning");
    let project = app.project_of(&id).unwrap();
    let orch = app.orch_for(&project).unwrap().clone();
    let store = app.require_store().unwrap().clone();
    let workspace = orch.prepare_plan_workspace(&id, &store).unwrap();
    let mut active = app.take_plan(&id).unwrap();
    // Keep the dispatch turn out of the delivery queue: the sole operation
    // under qualification is the report that consumes the documents below.
    let _dispatch = orch.open_plan_drafting(&mut active, workspace).unwrap();
    let docs_root = &active.workspace.as_ref().unwrap().docs_dir;
    let manifest = write_documents(docs_root);
    assert_eq!(active.plan.state, PlanState::Drafting);
    assert!(active.stages.is_empty());
    assert!(active.last_summary.is_none());
    assert!(
        !store.has_plan_docs(&id),
        "scratch output has not been ingested yet"
    );
    app.finish_plan_mutation(id.clone(), active).unwrap();
    let caller = primary_agent_id(app, &id);
    app.test_install_mcp_token(caller.clone(), TOKEN.into());
    app.delivery_queue.clear_queued();
    DraftingPlan {
        id,
        caller,
        manifest,
    }
}

fn write_documents(root: &std::path::Path) -> String {
    let entries = DOCUMENTS
        .iter()
        .map(|(id, path, contents)| {
            let target = root.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, contents).unwrap();
            StageManifestEntry {
                id: (*id).into(),
                title: format!("Qualification {id}"),
                path: (*path).into(),
                summary: format!("Reviewed work for {id}"),
            }
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::to_string_pretty(&entries).unwrap();
    std::fs::write(root.join(STAGES_MANIFEST_PATH), &manifest).unwrap();
    manifest
}

fn assert_completed(fixture: &RareFixture, drafted: &DraftingPlan) -> Value {
    let app = fixture.state().lock().unwrap();
    let active = &app.plans[&drafted.id];
    assert_eq!(active.plan.state, PlanState::PlanReview);
    assert_eq!(active.last_summary.as_deref(), Some(SUMMARY));
    let store = app.require_store().unwrap();
    let record = store
        .load_all_plans()
        .unwrap()
        .into_iter()
        .find(|plan| plan.id == drafted.id)
        .unwrap();
    assert_eq!(record.state, PlanState::PlanReview);
    assert_eq!(record.last_summary.as_deref(), Some(SUMMARY));
    assert!(record.last_error.is_none());
    assert_eq!(record.plan_path, STAGES_MANIFEST_PATH);
    assert_eq!(record.stages.len(), DOCUMENTS.len());
    for (stage, (id, path, contents)) in record.stages.iter().zip(DOCUMENTS) {
        assert_eq!(stage.id, id);
        assert_eq!(stage.path, path);
        assert_eq!(stage.state, StageDocState::Planned);
        assert_eq!(
            store.read_plan_doc(&drafted.id, path).as_deref(),
            Some(contents)
        );
    }
    assert_eq!(
        store
            .read_plan_doc(&drafted.id, STAGES_MANIFEST_PATH)
            .as_deref(),
        Some(drafted.manifest.as_str())
    );
    json!({"persisted_plan":drafted.id,"state":"plan_review","summary":record.last_summary,
        "stages":record.stages,"canonical_manifest":drafted.manifest,"verified_stage_docs":DOCUMENTS.len()})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drafting_plan_done_completed_ingests_stage_docs_without_reply() {
    crate::git_fixture::environment::isolated_git_test!();
    let report = execute().await;
    assert_eq!(report.report.status, DoneStatus::Completed);
    assert!(report.wait.root_finished && report.wait.acknowledged_at.is_none());
    assert_eq!(report.effect["state"], "plan_review");
}
