use super::*;

#[test]
fn failed_plan_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let id = file_legacy_task(&mut app, "committed plan");
    let before = app.plans[&id].agents.clone();
    let clock = app.board.attention().clock(&id);
    let mut candidate = app.take_plan(&id).unwrap();
    candidate.plan.goal = "failed plan".into();
    candidate.plan.state = crate::plan::PlanState::Failed;
    candidate.last_summary = Some("failed summary".into());
    candidate.agents.sole_thread_mut().post_user("failed words", None, "2099-01-01T00:00:00Z");
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_plan_mutation(id.clone(), candidate).is_err());
    assert_eq!(app.plans[&id].plan.goal, "committed plan");
    assert_eq!(app.plans[&id].agents, before);
    assert_eq!(app.board.attention().clock(&id), clock);
    file_legacy_task(&mut app, "unrelated success");
    app.publish_read_metadata();
    let candidate = app.take_plan(&id).unwrap();
    app.finish_plan_mutation(id.clone(), candidate).unwrap();
    let records = app.store.as_ref().unwrap().load_all_plans().unwrap();
    let saved = records.iter().find(|record| record.id == id).unwrap();
    assert_eq!(saved.goal, "committed plan");
    assert_eq!(saved.last_summary, None);
    assert_eq!(saved.roster(), before);
}

#[test]
fn failed_run_save_cannot_leak_through_an_unrelated_success_or_same_owner_retry() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let id = adopted_run(&mut app, &repo, dir.path(), "committed-run");
    let goal = app.runs[&id].run.goal.clone();
    let before = app.runs[&id].agents.clone();
    let clock = app.board.attention().clock(&id);
    let mut candidate = app.take_run(&id).unwrap();
    candidate.run.goal = "failed run".into();
    candidate.run.state = RunState::Failed;
    candidate.last_summary = Some("failed summary".into());
    candidate.agents.sole_thread_mut().post_user("failed words", None, "2099-01-01T00:00:00Z");
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation(id.clone(), candidate).is_err());
    assert_eq!(app.runs[&id].run.goal, goal);
    assert_eq!(app.runs[&id].agents, before);
    assert_eq!(app.board.attention().clock(&id), clock);
    file_legacy_task(&mut app, "unrelated success");
    app.publish_read_metadata();
    let candidate = app.take_run(&id).unwrap();
    app.finish_run_mutation(id.clone(), candidate).unwrap();
    let records = app.store.as_ref().unwrap().load_all_runs().unwrap();
    let saved = records.iter().find(|record| record.id == id).unwrap();
    assert_eq!(saved.goal, goal);
    assert_eq!(saved.last_summary, None);
    assert_eq!(saved.roster(), before);
}

#[tokio::test]
async fn failed_finishers_emit_no_change_or_news() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = push::greeted_push_session(&repo, dir.path());
    push::watch_everything(&handler, &sender);
    let (plan, run) = {
        let mut app = state.lock().unwrap();
        let plan = file_legacy_task(&mut app, "committed plan");
        let run = adopted_run(&mut app, &repo, dir.path(), "committed-run");
        (plan, run)
    };
    push::settled_pushes(&mut rx, &key).await;
    {
        let mut app = state.lock().unwrap();
        let mut active = app.take_plan(&plan).unwrap();
        active.agents.sole_thread_mut().post_user("failed plan news", None, "2099-01-01T00:00:00Z");
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_plan_mutation(plan, active).is_err());
        let mut active = app.take_run(&run).unwrap();
        active.agents.sole_thread_mut().post_user("failed run news", None, "2099-01-01T00:00:00Z");
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_run_mutation(run, active).is_err());
    }
    let pushed = push::settled_pushes(&mut rx, &key).await;
    assert!(push::changed_entities(&pushed).is_empty(), "{pushed:?}");
}

#[test]
fn failed_user_activity_persistence_keeps_its_best_effort_observation() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let saved = app.store.as_ref().unwrap().load_user_session().unwrap();
    app.store.as_ref().unwrap().fail_next_write();
    app.note_user_activity(42);
    assert_eq!(app.user_session().last_activity_ms, Some(42));
    assert_eq!(app.store.as_ref().unwrap().load_user_session().unwrap(), saved);
    app.note_user_activity(43);
    assert_eq!(app.store.as_ref().unwrap().load_user_session().unwrap(), Some(app.user_session()));
}
