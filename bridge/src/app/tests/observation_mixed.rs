use super::*;
use crate::app::observation_reads::mixed::MixedReadContext;

async fn prepared(
    state: &AppState,
    method: &str,
    params: &Value,
) -> crate::app::observation_reads::mixed::MixedJob {
    let context = MixedReadContext::capture(state);
    let mut connection = state.require_store().unwrap().read_pool().acquire().await;
    connection
        .read(|read| context.prepare(method, params, Some(read)))
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn mixed_task_attachment_keeps_path_fences_ranges_and_write_visibility() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project = state.project_at(0).id.clone();
    let task = super::tracker::filed(&mut state, &project, "attachment");
    let attached = state.handle(req(
        "tasks.attach",
        json!({"project_id":project,"filename":"a.txt","content_b64":b64encode(b"abcdef")}),
    ));
    assert_eq!(attached["ok"], true, "{attached}");
    let path = attached["result"]["path"].clone();
    let params = json!({"task_id":task["id"],"path":path,"offset":2,"length":2});
    let expected = state.handle(req("tasks.attachment", params.clone()))["result"].clone();
    let job = prepared(&state, "tasks.attachment", &params).await;
    assert_eq!(job.run().unwrap(), expected);
    let forbidden = dir.path().join("private.txt");
    std::fs::write(&forbidden, "secret").unwrap();
    let escaped = prepared(
        &state,
        "tasks.attachment",
        &json!({"task_id":task["id"],"path":forbidden}),
    )
    .await;
    assert!(escaped.run().is_err());
    let home = state.local_attachments_dir();
    let alias = home.join("escape.txt");
    std::os::unix::fs::symlink(&forbidden, &alias).unwrap();
    let escaped = prepared(
        &state,
        "tasks.attachment",
        &json!({"task_id":task["id"],"path":alias}),
    )
    .await;
    assert!(escaped.run().is_err());
}

#[tokio::test]
async fn mixed_thread_attachment_keeps_homes_ranges_and_reset_removal_fences() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "attachment fences");
    let attached = state.handle(req(
        "thread.attach",
        json!({"entity_id":owner,"filename":"a.txt","content_b64":b64encode(b"abcdef")}),
    ));
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
    let root = state.require_store().unwrap().plan_docs_dir(&plan);
    std::fs::create_dir_all(root.join(".build")).unwrap();
    std::fs::write(root.join(".build/plan.md"), "canonical document").unwrap();
    let active = state.plans.get_mut(&plan).unwrap();
    active.stages.clear();
    active.plan_path = ".build/plan.md".into();
    let params = json!({"plan_id":plan});
    let expected = state.plan_doc(&params).unwrap();
    let job = prepared(&state, "task.doc", &params).await;
    assert_eq!(job.run().unwrap(), expected);
    state.plans.get_mut(&plan).unwrap().plan_path = "../outside.md".into();
    let job = prepared(&state, "task.doc", &params).await;
    assert_eq!(
        job.run().unwrap_err().message(),
        "plan doc not available: ../outside.md"
    );
    state.plans.get_mut(&plan).unwrap().stages = vec![crate::plan::StageDoc {
        id: "unsafe".into(),
        title: "Unsafe".into(),
        path: ".build/plan/../../outside.md".into(),
        summary: String::new(),
        state: crate::plan::StageDocState::Planned,
    }];
    let context = MixedReadContext::capture(&state);
    let error = context
        .prepare(
            "task.stage_doc",
            &json!({"plan_id":plan,"stage_id":"unsafe"}),
            None,
        )
        .err()
        .unwrap();
    assert!(error
        .message()
        .starts_with("stage doc path escapes .build/plan/:"));
}

#[tokio::test]
async fn mixed_archive_preserves_finished_registry_rows() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project = state.project_at(0).id.clone();
    let id = super::project_agent::workspace(&mut state, &project, "finished-observation");
    let mut record = state.workspaces.get(&id).unwrap().clone();
    record.status = crate::workspace::WorkspaceStatus::Finished;
    record.archived_at = Some("2026-10-01T12:00:00Z".into());
    let legacy = state.workspaces.root().join(".legacy");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("finished.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let expected = state.archived_list();
    assert_eq!(expected["items"][0]["workspace_id"], id);
    assert_eq!(
        prepared(&state, "archived.list", &json!({}))
            .await
            .run()
            .unwrap(),
        expected
    );
}

#[tokio::test]
async fn mixed_review_delayed_work_keeps_its_selected_version_and_next_read_is_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut state, project) = super::tracker::tracked(tmp.path());
    let workspace = super::project_agent::workspace(&mut state, &project, "mixed-review");
    let task = super::tracker::filed(&mut state, &project, "review version");
    let first = state.handle(req(
        "tasks.review.snapshot",
        json!({"task_id":task["id"],"workspace_id":workspace,"expected_version":0}),
    ));
    assert_eq!(first["ok"], true, "{first}");
    let params = json!({"task_id":task["id"]});
    let old = prepared(&state, "tasks.review.get", &params).await;
    let snapshot = &first["result"]["review"]["snapshots"][0];
    let diff_params = json!({"task_id":task["id"],"snapshot_id":snapshot["id"],"directory_id":snapshot["directories"][0]["id"],"mode":"changes"});
    let expected_diff =
        state.handle(req("tasks.review.diff", diff_params.clone()))["result"].clone();
    let delayed_diff = prepared(&state, "tasks.review.diff", &diff_params).await;
    let second = state.handle(req(
        "tasks.review.snapshot",
        json!({"task_id":task["id"],"workspace_id":workspace,"expected_version":1}),
    ));
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(old.run().unwrap()["review"]["version"], 1);
    assert_eq!(delayed_diff.run().unwrap(), expected_diff);
    assert_eq!(
        prepared(&state, "tasks.review.get", &params)
            .await
            .run()
            .unwrap()["review"]["version"],
        2
    );
}

#[tokio::test]
async fn mixed_archive_uses_published_branch_without_reopening_git() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "archived branch");
    state.runs.get_mut(&owner).unwrap().run.state = crate::run::RunState::Archived;
    state.record_snapshot_run_branch(&owner, "committed-branch-observation".into());
    let answer = prepared(&state, "archived.list", &json!({}))
        .await
        .run()
        .unwrap();
    let row = answer["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == owner)
        .unwrap();
    assert_eq!(row["branch"], "committed-branch-observation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_attachment_external_gate_releases_every_sql_connection() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project = state.project_at(0).id.clone();
    let task = super::tracker::filed(&mut state, &project, "release SQL before attachment I/O");
    let attached = state.handle(req(
        "tasks.attach",
        json!({"project_id":project,"filename":"a.txt","content_b64":b64encode(b"content")}),
    ));
    let (gate, held) = OffLockGate::new();
    state.off_lock_gate = Some(gate);
    state.publish_snapshot_projection();
    let pool = state.require_store().unwrap().read_pool();
    let connection = pool.acquire().await;
    let read = state.observation_service().prepare(
        req(
            "tasks.attachment",
            json!({"task_id":task["id"],"path":attached["result"]["path"]}),
        ),
        Some(connection),
    );
    let worker = std::thread::spawn(read);
    held.wait_for_arrival();
    let mut borrowed = Vec::new();
    for _ in 0..8 {
        borrowed.push(
            tokio::time::timeout(std::time::Duration::from_secs(2), pool.acquire())
                .await
                .expect("external I/O retained a read connection"),
        );
    }
    held.release();
    assert_eq!(worker.join().unwrap()["ok"], true);
}

#[test]
fn mixed_attachment_read_after_acknowledged_write_uses_the_initialized_handler() {
    let (dir, repo) = init_repo();
    let state = qa_state_timed_by(FrameClock::new(), &repo, dir.path());
    let project = state.lock().unwrap().project_at(0).id.clone();
    let handler = AppState::handler(state);
    let task = call(
        &handler,
        "tasks.create",
        json!({"project_id":project,"title":"post-ack attachment"}),
    );
    assert_eq!(task["ok"], true, "{task}");
    let attached = call(
        &handler,
        "tasks.attach",
        json!({"project_id":project,"filename":"ack.txt","content_b64":b64encode(b"after the write")}),
    );
    assert_eq!(attached["ok"], true, "{attached}");
    let read = call(
        &handler,
        "tasks.attachment",
        json!({"task_id":task["result"]["task"]["id"],"path":attached["result"]["path"]}),
    );
    assert_eq!(read["ok"], true, "{read}");
    assert_eq!(read["result"]["content_b64"], b64encode(b"after the write"));
}

#[test]
fn mixed_plan_documents_retain_committed_paths_after_failed_save_and_unrelated_publication() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (staged, _) = planned_run_in_review(&mut app, "committed staged document");
    let stage = app.plans[&staged].stages[0].id.clone();
    let single = file_legacy_task(&mut app, "committed single document");
    let mut candidate = app.take_plan(&single).unwrap();
    candidate.plan_path = ".build/single.md".into();
    app.finish_plan_mutation(single.clone(), candidate).unwrap();
    let root = app.require_store().unwrap().plan_docs_dir(&single);
    std::fs::create_dir_all(root.join(".build")).unwrap();
    std::fs::write(root.join(".build/single.md"), "committed text").unwrap();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let cases = [
        ("task.doc", json!({"task_id":single})),
        ("task.stage_doc", json!({"task_id":staged,"stage_id":stage})),
    ];
    let before = cases
        .iter()
        .map(|(method, params)| call(&handler, method, params.clone()))
        .collect::<Vec<_>>();
    for reply in &before {
        assert_eq!(reply["ok"], true, "{reply}");
    }
    {
        let mut app = state.lock().unwrap();
        for id in [&single, &staged] {
            let mut candidate = app.take_plan(id).unwrap();
            candidate.plan_path = "../failed-document.md".into();
            if let Some(stage) = candidate.stages.first_mut() {
                stage.path = "../failed-stage.md".into();
            }
            app.store.as_ref().unwrap().fail_next_write();
            assert!(app.finish_plan_mutation(id.clone(), candidate).is_err());
        }
        file_legacy_task(&mut app, "unrelated publication after failed documents");
        app.publish_read_metadata();
    }
    for ((method, params), expected) in cases.into_iter().zip(before) {
        assert_eq!(call(&handler, method, params), expected);
    }
}

#[test]
fn mixed_archive_capture_visits_retained_records_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "archive capture cost");
    state.runs.get_mut(&owner).unwrap().run.state = crate::run::RunState::Archived;
    let records = (0..512)
        .map(|index| {
            let id = format!("archived-{index}");
            (
                id.clone(),
                PersistedArchivedWorktree {
                    status: WorktreeFinishStatus::Archived,
                    project_path: repo.display().to_string(),
                    worktree_id: id.clone(),
                    worktree_name: id.clone(),
                    worktree_path: dir.path().join(&id).display().to_string(),
                    branch: Some(id),
                    head_sha: String::new(),
                    upstream: None,
                    unpushed: None,
                    dirty_files: 0,
                    uncommitted_files: 0,
                    uncommitted_insertions: 0,
                    uncommitted_deletions: 0,
                    action: WorktreeFinishAction::Cleanup,
                    archived_at: None,
                },
            )
        })
        .collect();
    state.board.replace_archived(records);
    crate::app::board_index::take_archive_visits();
    let _captured = MixedReadContext::capture(&state);
    assert_eq!(
        crate::app::board_index::take_archive_visits(),
        512,
        "capture must not rescan the archive for each run"
    );
}

#[test]
fn mixed_review_destinations_publish_reclaim_reservation_and_release() {
    let tmp = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = super::tracker::tracked(tmp.path());
    let workspace = super::project_agent::workspace(&mut app, &project, "reservation-observation");
    let task = super::tracker::filed(&mut app, &project, "review destination reservations");
    let saved = app.handle(req(
        "tasks.review.snapshot",
        json!({"task_id":task["id"],"workspace_id":workspace,"expected_version":0}),
    ));
    assert_eq!(saved["ok"], true, "{saved}");
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let params = json!({"task_id":task["id"]});
    let before = call(&handler, "tasks.review.get", params.clone());
    assert_eq!(before["ok"], true, "{before}");
    assert!(
        before["result"]["review"]["destinations"][0]["error"].is_null(),
        "{before}"
    );
    state.lock().unwrap().reserve_workspace_for_test(&workspace);
    let reserved = call(&handler, "tasks.review.get", params.clone());
    assert_eq!(reserved["ok"], true, "{reserved}");
    assert_eq!(
        reserved["result"]["review"]["destinations"][0]["error"],
        crate::reclaim::RESERVED
    );
    state
        .lock()
        .unwrap()
        .release_workspace_reservation_for_test(&workspace);
    let released = call(&handler, "tasks.review.get", params);
    assert_eq!(released["ok"], true, "{released}");
    assert!(
        released["result"]["review"]["destinations"][0]["error"].is_null(),
        "{released}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_thread_attachment_service_observes_real_reset_and_agent_removal() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut app, "delayed attachment publication");
    let added = app.agent_add(&json!({"entity_id":owner})).unwrap();
    let agent = added["agent"]["id"].as_str().unwrap().to_string();
    let attached = app.handle(req("thread.attach", json!({"entity_id":owner,"agent_id":agent,"filename":"a.txt","content_b64":b64encode(b"old attachment")})));
    assert_eq!(attached["ok"], true, "{attached}");
    let params = json!({"entity_id":owner,"agent_id":agent,"path":attached["result"]["path"]});
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let (service, reset_params) = {
        let app = state.lock().unwrap();
        let record = app.entity_agents(&owner).unwrap().by_id(&agent).unwrap();
        (
            app.observation_service(),
            json!({"project_id":app.projects.project_id_of(&owner),"entity_id":owner,"agent_id":agent,"conversation_id":record.conversation_id(),"expected_thread_id":record.thread.id}),
        )
    };
    let delayed = service.prepare(req("thread.attachment", params.clone()), None);
    let reset = call(&handler, "conversation.reset", reset_params);
    assert_eq!(reset["ok"], true, "{reset}");
    let stale = delayed();
    assert_eq!(stale["ok"], false, "{stale}");
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .starts_with("stale thread_id"),
        "{stale}"
    );
    let fresh = call(&handler, "thread.attachment", params.clone());
    assert_eq!(fresh["ok"], true, "{fresh}");
    let delayed = service.prepare(req("thread.attachment", params), None);
    let removed = call(
        &handler,
        "agent.remove",
        json!({"entity_id":owner,"agent_id":agent}),
    );
    assert_eq!(removed["ok"], true, "{removed}");
    let stale = delayed();
    assert_eq!(stale["ok"], false, "{stale}");
    assert!(
        stale["error"]
            .as_str()
            .unwrap()
            .starts_with("unknown agent_id"),
        "{stale}"
    );
}
