use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_snapshot_full_keeps_reloaded_resident_tail_and_complete_windows() {
    let mut reads = NativeReads::new();
    let (agent, conversation, retained, resident) = {
        let mut app = reads.state.lock().unwrap();
        let mut plan = app.take_plan(&reads.task).unwrap();
        for index in 0..300 {
            plan.agents.sole_thread_mut().post_user(
                format!("retained task message {index}"),
                None,
                "2026-10-21T12:00:00Z",
            );
        }
        app.finish_plan_mutation(reads.task.clone(), plan).unwrap();
        // Use the same bounded loader and reattachment as a daemon restart.
        let saved = app
            .store
            .as_ref()
            .unwrap()
            .load_all_plans()
            .unwrap()
            .into_iter()
            .find(|plan| plan.id == reads.task)
            .unwrap();
        let plan = crate::orchestrator::ActivePlan::reattach(&saved);
        let agent = plan.agents.sole();
        let retained = agent.thread.total_item_count();
        let resident = agent.thread.items.len();
        assert!(
            retained > resident as u64,
            "fixture must leave history on disk"
        );
        let identity = (agent.id.clone(), agent.conversation_id().to_owned());
        app.finish_plan_mutation(reads.task.clone(), plan).unwrap();
        assert_eq!(
            app.read_view
                .load()
                .history(&identity.1)
                .unwrap()
                .render(&json!({}))["items"]
                .as_array()
                .unwrap()
                .len() as u64,
            retained
        );
        (identity.0, identity.1, retained, resident)
    };
    for window in [
        json!({}),
        json!({"agent_id":agent,"conversation_id":conversation}),
        json!({"thread_limit":null}),
        json!({"thread_limit":1}),
        json!({"thread_after_sequence":0}),
    ] {
        let mut params = json!({"task_id":reads.task});
        params
            .as_object_mut()
            .unwrap()
            .extend(window.as_object().unwrap().clone());
        let normalized = crate::api::v1::normalize_read_params("task.get", &params).unwrap();
        let expected = reads.state.lock().unwrap().plan_get(&normalized).unwrap();
        let count = expected["thread"]["items"].as_array().unwrap().len();
        if params.get("thread_after_sequence").is_some() {
            assert_eq!(
                count as u64, retained,
                "delta must still include stored history"
            );
        } else if params["thread_limit"] == 1 {
            assert_eq!(count, 1);
        } else {
            assert_eq!(
                count, resident,
                "unwindowed legacy detail ships its resident tail"
            );
        }
        let answer = reads.held_read("task.get", params.clone()).await;
        assert_eq!(answer["ok"], true, "{params}: {answer}");
        assert_eq!(answer["result"], expected, "{params}");
    }
}
