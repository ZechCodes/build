//! Read-only history/review tools retain their envelope without mutation turns.
use super::*;

fn comment(fixture: &Fixture) -> String {
    let reply = fixture.state.lock().unwrap().handle(req(
        "tasks.comment",
        json!({
            "task_id":fixture.task,"body":"history reader comment body"
        }),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"]["comment"]["id"].as_str().unwrap().into()
}

fn history(fixture: &Fixture) {
    let mut app = fixture.state.lock().unwrap();
    let mut active = app.take_run(&fixture.owner).unwrap();
    let thread = &mut active.agents.by_id_mut(&fixture.agent).unwrap().thread;
    thread.post_user("buried history witness", None, "2026-10-20T12:00:00Z");
    for index in 0..260 {
        thread.post_user(
            format!("newer history {index}"),
            None,
            "2026-10-20T12:01:00Z",
        );
    }
    app.finish_run_mutation(fixture.owner.clone(), active)
        .unwrap();
    // A real Store reload leaves the witness below the resident tail.
    let record = app
        .require_store()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == fixture.owner)
        .unwrap();
    let active = crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into());
    let thread = &active.agents.by_id(&fixture.agent).unwrap().thread;
    assert!(thread.items.len() < thread.total_item_count() as usize);
    assert!(!thread.items.iter().any(|item| serde_json::to_string(item)
        .unwrap()
        .contains("buried history witness")));
    app.runs.insert(fixture.owner.clone(), active);
    app.publish_read_owner(&fixture.owner, true);
    app.delivery_queue.clear_queued();
}

fn sentry(fixture: &Fixture) {
    let mut app = fixture.state.lock().unwrap();
    app.delivery_queue.clear_queued();
    app.delivery_queue.enqueue(PendingAgentTurn {
        operation_id: None,
        root: fixture._root.path().join("missing-sentry-root"),
        owner: "missing-history-read-sentry".into(),
        agent_id: "missing-history-read-sentry-agent".into(),
        conversation_id: "missing-history-read-sentry-agent".into(),
        model_choice: ModelChoice::default(),
        choice_revision: 0,
        interrupt: false,
        say: None,
        phase: "history_read_sentry",
        wants_catch_up: false,
        survives_refusal: false,
    });
}

fn assert_no_drain(fixture: &Fixture, before: &Arc<crate::app::read_view::ReadView>) {
    let app = fixture.state.lock().unwrap();
    assert!(
        Arc::ptr_eq(before, &app.read_view.load()),
        "independent read published application state"
    );
    assert_eq!(
        app.delivery_queue.queued_len(),
        1,
        "independent read drained unrelated queued work"
    );
    assert_eq!(
        app.delivery_queue.queued_nth(0).unwrap().owner,
        "missing-history-read-sentry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_reads_search_below_resident_tail_and_read_comment_with_both_turns_held() {
    let fixture = Fixture::new();
    history(&fixture);
    let actions = [
        BridgeAction::SearchConversation {
            query: crate::thread::ConversationQuery {
                text: Some("buried history witness".into()),
                ..Default::default()
            },
        },
        BridgeAction::TrackerReadComment {
            comment_id: comment(&fixture),
        },
    ];
    let expected = actions
        .iter()
        .cloned()
        .map(|a| fixture.legacy(a))
        .collect::<Vec<_>>();
    assert_eq!(expected[0]["ok"], true, "{}", expected[0]);
    assert_eq!(expected[0]["result"]["hits"].as_array().unwrap().len(), 1);
    assert_eq!(expected[1]["result"]["body"], "history reader comment body");
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        assert_eq!(
            fixture.reply(fixture.frame(action.clone())).await,
            expected,
            "{}",
            action.tool_name()
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_router_read_matches_full_transcript_and_errors_without_writer_turns() {
    let fixture = Fixture::new();
    history(&fixture);
    let (capture, router) = {
        let mut app = fixture.state.lock().unwrap();
        let pair = crate::app::tests::routing::router::captured(&mut app, "history-only router");
        app.session_registry
            .test_install_token(pair.1.clone(), TOKEN.into());
        app.delivery_queue.clear_queued();
        pair
    };
    let actions = [
        BridgeAction::ReadConversation {
            entity_id: fixture.owner.clone(),
            agent_id: Some(fixture.agent.clone()),
            limit: 300,
        },
        BridgeAction::ReadConversation {
            entity_id: "missing-conversation".into(),
            agent_id: None,
            limit: 3,
        },
        BridgeAction::SearchConversation {
            query: Default::default(),
        },
    ];
    let expected = actions
        .iter()
        .cloned()
        .map(|action| {
            let result = fixture
                .state
                .lock()
                .unwrap()
                .router_action(&capture, action);
            crate::app::mcp::mcp_action_response(result)
        })
        .collect::<Vec<_>>();
    assert_eq!(expected[0]["ok"], true, "{}", expected[0]);
    assert!(expected[0]["result"]["transcript"]
        .as_str()
        .unwrap()
        .contains("buried history witness"));
    assert_eq!(expected[1]["ok"], false);
    assert_eq!(expected[2]["ok"], false);
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        let mut frame = fixture.frame(action.clone());
        frame["task_id"] = json!(router);
        assert_eq!(
            fixture.reply(frame).await,
            expected,
            "{}",
            action.tool_name()
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(held.len(), 2);
}

async fn legacy_review(fixture: &Fixture, action: BridgeAction) -> Value {
    // Keep the pre-read-lane MCP path as an independent oracle, including its
    // deferred external job. Fixture::legacy alone returns its initial Null.
    let (clock, generation) = {
        let app = fixture.state.lock().unwrap();
        (
            app.frame_clock.clone(),
            app.mcp_conversation_generation(&fixture.owner, &fixture.agent)
                .unwrap(),
        )
    };
    let timer = clock.frame(MCP_CONTROL_METHOD);
    let mut turn = ControlTurn::take(&fixture.plane, &timer).await;
    crate::app::mcp::handle_coding_mcp_frame(
        &fixture.state,
        &fixture.frame(action),
        &generation,
        &timer,
        &mut turn,
    )
    .await
    .unwrap()
}

fn review_actions(fixture: &Fixture) -> Vec<BridgeAction> {
    let reply = fixture.state.lock().unwrap().handle(req(
        "tasks.review.snapshot",
        json!({
            "task_id":fixture.task,"workspace_id":fixture.workspace,"expected_version":0
        }),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    let snapshot = &reply["result"]["review"]["snapshots"][0];
    vec![
        BridgeAction::TrackerGetReview {
            task_id: fixture.task.clone(),
        },
        BridgeAction::TrackerReadReview {
            task_id: fixture.task.clone(),
            snapshot_id: snapshot["id"].as_str().unwrap().into(),
            directory_id: snapshot["directories"][0]["id"].as_str().unwrap().into(),
            mode: crate::reviews::read::ReviewReadMode::Blob,
            path: Some("README.md".into()),
            paths: Vec::new(),
            patch: false,
            range: Some(crate::body_page::FileRange {
                offset: 0,
                bytes: 4,
                raw: Some(true),
            }),
        },
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_review_reads_preserve_saved_blob_ranges_without_writer_turns_or_drain() {
    let fixture = Fixture::new();
    let actions = review_actions(&fixture);
    let mut expected = Vec::new();
    for action in &actions {
        expected.push(legacy_review(&fixture, action.clone()).await);
    }
    for answer in &expected {
        assert_eq!(answer["ok"], true, "{answer}");
    }
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        assert_eq!(
            fixture.reply(fixture.frame(action.clone())).await,
            expected,
            "{}",
            action.tool_name()
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_review_read_refusals_keep_tool_specific_error_envelopes() {
    let fixture = Fixture::new();
    let mut actions = review_actions(&fixture);
    actions.push(BridgeAction::TrackerReadComment {
        comment_id: "tc-missing-history".into(),
    });
    actions.push(BridgeAction::TrackerGetReview {
        task_id: "task-missing-review".into(),
    });
    if let BridgeAction::TrackerReadReview { snapshot_id, .. } = &mut actions[1] {
        *snapshot_id = "snapshot-missing-review".into();
    }
    actions.remove(0);
    let expected = actions
        .iter()
        .cloned()
        .map(|action| fixture.legacy(action))
        .collect::<Vec<_>>();
    for answer in &expected {
        assert_eq!(answer["ok"], false, "{answer}");
    }
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        assert_eq!(
            fixture.reply(fixture.frame(action.clone())).await,
            expected,
            "{}",
            action.tool_name()
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_review_external_work_releases_sql_and_app_with_both_turns_held() {
    let fixture = Fixture::new();
    let action = review_actions(&fixture).remove(1);
    let expected = legacy_review(&fixture, action.clone()).await;
    assert_eq!(expected["ok"], true, "{expected}");
    let (gate, external) = OffLockGate::new();
    let pool = {
        let mut app = fixture.state.lock().unwrap();
        app.off_lock_gate = Some(gate);
        // Install real mixed inputs with the external-work barrier captured.
        let inputs = Arc::new(crate::app::observation_reads::ObservationContext::capture(
            &app,
        ));
        let delta = crate::app::read_view::SnapshotDelta::new(app.read_view.next_version())
            .set_observations(inputs);
        app.publish_prepared_snapshot_projection(delta);
        app.require_store().unwrap().read_pool()
    };
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    let reading = tokio::spawn(socket_reply(
        Arc::clone(&fixture.state),
        Arc::clone(&fixture.plane),
        fixture.frame(action),
    ));
    tokio::time::timeout(WATCHDOG, async {
        while !external.has_pending_arrival() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("review never reached its owned external work without a writer turn");
    assert!(
        fixture.state.try_lock().is_ok(),
        "review external work retained app mutex"
    );
    let connections = tokio::time::timeout(WATCHDOG, async {
        let mut held = Vec::new();
        for _ in 0..SQL_READERS {
            held.push(pool.acquire().await);
        }
        held
    })
    .await
    .expect("review retained SQL connection during external work");
    external.release();
    assert_eq!(reading.await.unwrap(), expected);
    assert_no_drain(&fixture, &before);
    assert_eq!(turns.len(), 2);
    drop(connections);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_review_foreign_project_ids_match_unknown_refusals() {
    let fixture = Fixture::new();
    let (_other_home, other_repo) = init_repo();
    let (foreign_task, foreign_comment) = {
        let mut app = fixture.state.lock().unwrap();
        let project = crate::app::tests::project_agent::added_project(&mut app, &other_repo);
        let task = crate::app::tests::tracker::filed(&mut app, &project, "foreign history scope");
        let comment = app.handle(req(
            "tasks.comment",
            json!({"task_id":task["id"],"body":"foreign comment body"}),
        ));
        assert_eq!(comment["ok"], true, "{comment}");
        (
            task["id"].as_str().unwrap().to_owned(),
            comment["result"]["comment"]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        )
    };
    let actions = [
        BridgeAction::TrackerReadComment {
            comment_id: foreign_comment,
        },
        BridgeAction::TrackerGetReview {
            task_id: foreign_task.clone(),
        },
        BridgeAction::TrackerReadReview {
            task_id: foreign_task,
            snapshot_id: "snapshot-foreign".into(),
            directory_id: "directory-foreign".into(),
            mode: crate::reviews::read::ReviewReadMode::Changes,
            path: None,
            paths: Vec::new(),
            patch: false,
            range: None,
        },
    ];
    let expected = actions
        .iter()
        .cloned()
        .map(|action| fixture.legacy(action))
        .collect::<Vec<_>>();
    for answer in &expected {
        assert_eq!(answer["ok"], false, "{answer}");
    }
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        assert_eq!(
            fixture.reply(fixture.frame(action.clone())).await,
            expected,
            "{}",
            action.tool_name()
        );
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(turns.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_reads_resolve_cross_owner_aliases_to_canonical_stored_history() {
    let fixture = Fixture::new();
    history(&fixture);
    let (alias_owner, alias_agent, capture, router) = {
        let mut app = fixture.state.lock().unwrap();
        let owner = file_legacy_task(&mut app, "alias reader history");
        let agent = primary_agent_id(&app, &owner);
        app.edit_agent_record("test canonical history alias", &owner, &agent, |record| {
            record.bind_conversation(&fixture.agent);
        });
        app.session_registry
            .test_install_token(agent.clone(), TOKEN.into());
        let (capture, router) =
            crate::app::tests::routing::router::captured(&mut app, "alias history router");
        app.session_registry
            .test_install_token(router.clone(), TOKEN.into());
        (owner, agent, capture, router)
    };
    let search = BridgeAction::SearchConversation {
        query: crate::thread::ConversationQuery {
            text: Some("buried history witness".into()),
            ..Default::default()
        },
    };
    let catchup = BridgeAction::ReadConversation {
        entity_id: alias_owner.clone(),
        agent_id: Some(alias_agent.clone()),
        limit: 300,
    };
    let (expected_search, expected_catchup) = {
        let mut app = fixture.state.lock().unwrap();
        (
            crate::app::mcp::mcp_action_response(app.on_agent_mcp_action(
                &alias_owner,
                &alias_agent,
                search.clone(),
            )),
            crate::app::mcp::mcp_action_response(app.router_action(&capture, catchup.clone())),
        )
    };
    assert_eq!(
        expected_search["result"]["hits"].as_array().unwrap().len(),
        1
    );
    assert_eq!(expected_catchup["result"]["agent_id"], alias_agent);
    assert_eq!(expected_catchup["result"]["conversation_id"], fixture.agent);
    assert!(expected_catchup["result"]["transcript"]
        .as_str()
        .unwrap()
        .contains("buried history witness"));
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    for (agent, action, expected) in [
        (alias_agent, search, expected_search),
        (router, catchup, expected_catchup),
    ] {
        let mut frame = fixture.frame(action);
        frame["task_id"] = json!(agent);
        assert_eq!(fixture.reply(frame).await, expected);
        assert_no_drain(&fixture, &before);
    }
    assert_eq!(turns.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_router_query_rejects_target_reset_or_rebind_after_admission() {
    for reset in [false, true] {
        let fixture = Fixture::new();
        history(&fixture);
        let (router, replacement) = {
            let mut app = fixture.state.lock().unwrap();
            let replacement = file_legacy_task(&mut app, "replacement target history");
            let replacement_agent = primary_agent_id(&app, &replacement);
            let (_, router) =
                crate::app::tests::routing::router::captured(&mut app, "frozen target router");
            app.session_registry
                .test_install_token(router.clone(), TOKEN.into());
            (router, replacement_agent)
        };
        let action = BridgeAction::ReadConversation {
            entity_id: fixture.owner.clone(),
            agent_id: Some(fixture.agent.clone()),
            limit: 300,
        };
        let mut frame = fixture.frame(action);
        frame["task_id"] = json!(router);
        let (gate, paused) = OffLockGate::new();
        fixture.state.lock().unwrap().off_lock_gate = Some(gate);
        sentry(&fixture);
        let turns = fixture.held_turns().await;
        let reading = tokio::spawn(socket_reply(
            fixture.state.clone(),
            fixture.plane.clone(),
            frame,
        ));
        tokio::time::timeout(WATCHDOG, async {
            while !paused.has_pending_arrival() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("history query never reached admitted off-lock work");
        assert!(fixture.state.try_lock().is_ok());
        let after_mutation = {
            let mut app = fixture.state.lock().unwrap();
            if reset {
                let thread = app
                    .agent_conversation(&fixture.owner, Some(&fixture.agent))
                    .unwrap();
                let params = json!({"project_id":fixture.project,"entity_id":fixture.owner,
                    "agent_id":fixture.agent,"conversation_id":fixture.agent,"expected_thread_id":thread.id});
                let reply = app.handle(req("conversation.reset", params));
                assert_eq!(reply["ok"], true, "{reply}");
            } else {
                app.edit_agent_record(
                    "test rebind while history query waits",
                    &fixture.owner,
                    &fixture.agent,
                    |agent| {
                        agent.bind_conversation(replacement);
                    },
                );
            }
            app.read_view.load()
        };
        paused.release();
        let answer = reading.await.unwrap();
        assert_eq!(
            answer["ok"], false,
            "obsolete target payload escaped: {answer}"
        );
        assert!(
            answer["error"].as_str().unwrap().contains(if reset {
                "stale thread_id"
            } else {
                "stale conversation_id"
            }),
            "{answer}"
        );
        assert_no_drain(&fixture, &after_mutation);
        assert_eq!(turns.len(), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_store_free_search_keeps_existing_no_project_admission() {
    let fixture = Fixture::new();
    {
        let mut app = fixture.state.lock().unwrap();
        let mut active = app.take_run(&fixture.owner).unwrap();
        active
            .agents
            .by_id_mut(&fixture.agent)
            .unwrap()
            .thread
            .post_user(
                "committed in-memory history witness",
                None,
                "2026-10-20T12:00:00Z",
            );
        app.finish_run_mutation(fixture.owner.clone(), active)
            .unwrap();
        app.store = None;
        app.projects.unbind_entity(&fixture.owner);
        app.read_context.store(None);
        app.initialize_read_context();
    }
    let action = BridgeAction::SearchConversation {
        query: crate::thread::ConversationQuery {
            text: Some("in-memory history witness".into()),
            ..Default::default()
        },
    };
    let expected = fixture.legacy(action.clone());
    assert_eq!(expected["ok"], true, "{expected}");
    assert_eq!(expected["result"]["hits"].as_array().unwrap().len(), 1);
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    assert_eq!(fixture.reply(fixture.frame(action)).await, expected);
    assert_no_drain(&fixture, &before);
    assert_eq!(turns.len(), 2);
}
