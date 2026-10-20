use super::*;
use crate::app::observation_reads::mixed::MixedReadContext;

async fn prepared(state: &AppState, method: &str, params: &Value) -> crate::app::observation_reads::mixed::MixedJob {
    let context = MixedReadContext::capture(state);
    let mut connection = state.require_store().unwrap().read_pool().acquire().await;
    connection.read(|read| context.prepare(method, params, Some(read))).unwrap().unwrap()
}

#[tokio::test]
async fn mixed_task_attachment_keeps_path_fences_ranges_and_write_visibility() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project = state.project_at(0).id.clone();
    let task = super::tracker::filed(&mut state, &project, "attachment");
    let attached = state.handle(req("tasks.attach", json!({"project_id":project,"filename":"a.txt","content_b64":b64encode(b"abcdef")})));
    assert_eq!(attached["ok"], true, "{attached}");
    let path = attached["result"]["path"].clone();
    let params = json!({"task_id":task["id"],"path":path,"offset":2,"length":2});
    let expected = state.handle(req("tasks.attachment", params.clone()))["result"].clone();
    let job = prepared(&state, "tasks.attachment", &params).await;
    assert_eq!(job.run().unwrap(), expected);
    let forbidden = dir.path().join("private.txt");
    std::fs::write(&forbidden, "secret").unwrap();
    let escaped = prepared(&state, "tasks.attachment", &json!({"task_id":task["id"],"path":forbidden})).await;
    assert!(escaped.run().is_err());
    let home = state.local_attachments_dir();
    let alias = home.join("escape.txt");
    std::os::unix::fs::symlink(&forbidden, &alias).unwrap();
    let escaped = prepared(&state, "tasks.attachment", &json!({"task_id":task["id"],"path":alias})).await;
    assert!(escaped.run().is_err());
}

#[tokio::test]
async fn mixed_thread_attachment_keeps_homes_ranges_and_reset_removal_fences() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "attachment fences");
    let attached = state.handle(req("thread.attach", json!({"entity_id":owner,"filename":"a.txt","content_b64":b64encode(b"abcdef")})));
    assert_eq!(attached["ok"], true, "{attached}");
    let params = json!({"entity_id":owner,"path":attached["result"]["path"],"offset":3,"length":2});
    let expected = state.handle(req("thread.attachment", params.clone()))["result"].clone();
    let job = prepared(&state, "thread.attachment", &params).await;
    let fence = job.fence();
    assert_eq!(job.run().unwrap(), expected);
    let agent = state.entity_agents(&owner).unwrap().primary().unwrap();
    let reset_params = json!({"project_id":state.projects.project_id_of(&owner),"entity_id":owner,"agent_id":agent.id,"conversation_id":agent.conversation_id(),"expected_thread_id":agent.thread.id});
    let reset = state.handle(req("conversation.reset", reset_params));
    assert_eq!(reset["ok"], true, "{reset}");
    assert!(MixedReadContext::capture(&state).validate(&fence).is_err());
    let job = prepared(&state, "thread.attachment", &params).await;
    let fence = job.fence();
    state.runs.remove(&owner);
    assert!(MixedReadContext::capture(&state).validate(&fence).is_err());
}

#[tokio::test]
async fn mixed_plan_document_keeps_canonical_store_and_error_parity() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan, _) = planned_run_in_review(&mut state, "canonical docs");
    let params = json!({"plan_id":plan});
    let expected = state.plan_doc(&params).unwrap();
    let job = prepared(&state, "task.doc", &params).await;
    assert_eq!(job.run().unwrap(), expected);
    state.plans.get_mut(&plan).unwrap().plan_path = "../outside.md".into();
    let job = prepared(&state, "task.doc", &params).await;
    assert_eq!(job.run().unwrap_err().message(), "plan doc not available: ../outside.md");
}

#[tokio::test]
async fn mixed_archive_preserves_finished_registry_rows() {
    let (dir, repo) = init_repo();
    let state = qa_state(&repo, dir.path());
    let expected = state.archived_list();
    assert_eq!(prepared(&state, "archived.list", &json!({})).await.run().unwrap(), expected);
}
