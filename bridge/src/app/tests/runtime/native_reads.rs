//! The remaining native reads complete through real intake while AppState is held.
use super::*;
use crate::carrier::testing::{client_request, session_init, ChannelWire};
use crate::carrier::{FrameIntake, OutboundEnvelope};
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod task_history;

const SESSION: &str = "remaining-native-reads";

pub(in crate::app::tests) struct NativeReads {
    pub state: Arc<Mutex<AppState>>,
    pub project: String,
    pub task: String,
    pub capture: String,
    intake: Arc<FrameIntake>,
    wire: ChannelWire,
    key: String,
    marks: Arc<Mutex<Vec<(&'static str, Instant)>>>,
    _directory: tempfile::TempDir,
}

impl NativeReads {
    pub fn new() -> Self {
        Self::configured(true)
    }

    fn configured(qa: bool) -> Self {
        let (directory, repository) = init_repo();
        let mut app = qa_state_before_admission(&repository, directory.path());
        app.qa_agent = qa;
        let project = app.project_at(0).id.clone();
        let task = "plan-native-read".to_string();
        let orchestrator = Orchestrator::new(
            repository.clone(),
            directory.path().join("side"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
            test_bridge_exe(),
        );
        let plan = orchestrator.create_plan(
            PlanId::new(&task),
            "native read goal",
            "main",
            Default::default(),
        );
        app.projects.bind_entity(task.clone(), project.clone());
        app.persist_plan_record(&task, &plan).unwrap();
        app.plans.insert(task.clone(), plan);
        let capture = "capture-native-read".to_string();
        let record = crate::capture::Capture::new(capture.clone(), "native capture", now_rfc3339());
        app.save_capture(record).unwrap();
        app.streams.insert(
            "stream-native-read".into(),
            StreamState {
                count: 0,
                events: Vec::new(),
                complete: true,
                ..Default::default()
            },
        );
        let marks = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&marks);
        app.frame_clock
            .observe_requests(Arc::new(move |frame, phase, at| {
                if frame.payload["id"] == 1 {
                    observed.lock().unwrap().push((phase, at));
                }
            }));
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        let transport = crate::transport::generate_transport_keypair();
        let intake = FrameIntake::with_pool(handler, transport.clone(), 128, 2);
        let mut wire = ChannelWire::open();
        let key = crate::transport::generate_session_key();
        wire.open_session(
            &intake,
            SESSION,
            &session_init(SESSION, &transport.public_key_b64, &key),
        )
        .unwrap();
        assert!(matches!(
            wire.outbound.try_recv(),
            Ok(OutboundEnvelope::SessionAccept { .. })
        ));
        Self {
            state,
            project,
            task,
            capture,
            intake,
            wire,
            key,
            marks,
            _directory: directory,
        }
    }

    pub async fn held_read(&mut self, method: &str, params: Value) -> Value {
        self.marks.lock().unwrap().clear();
        let (ready, arrived) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let state = Arc::clone(&self.state);
        let holder = std::thread::spawn(move || {
            let _held = state.lock().unwrap();
            ready.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(5));
        });
        arrived.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        let reply = tokio::time::timeout(Duration::from_millis(100), async {
            self.wire
                .accept(
                    &self.intake,
                    client_request(
                        &self.key,
                        SESSION,
                        "data",
                        json!({ "id": 1, "method": method, "params": params }),
                    ),
                )
                .await
                .unwrap();
            loop {
                let envelope = self.wire.outbound.recv().await.unwrap();
                let reply = SessionSender::decrypt_push(&self.key, &envelope);
                if reply.get("accepted").is_none() {
                    break reply;
                }
            }
        })
        .await;
        let reply_elapsed = started.elapsed();
        // Release before assertions so a regression cannot leave a blocked worker.
        release.send(()).unwrap();
        holder.join().unwrap();
        let reply = reply.unwrap_or_else(|_| {
            panic!("{method} did not answer within 100 ms while AppState was held")
        });
        assert!(
            reply_elapsed <= Duration::from_millis(100),
            "{method}: reply took {reply_elapsed:?}"
        );
        let marks = self.marks.lock().unwrap();
        assert!(
            marks.iter().any(|(phase, _)| *phase == "intake"),
            "{method}: no intake trace"
        );
        // Snapshot intake has no seal hook. The awaited, decrypted final
        // reply above is the client completion witness for every read lane.
        assert!(
            !marks.iter().any(|(phase, _)| phase.starts_with("app_")),
            "{method} acquired or waited for AppState: {marks:?}"
        );
        let stats = self.state.lock().unwrap().frame_clock.stats();
        let metric = &stats["methods"][method];
        assert!(
            metric["served"].as_u64().is_some_and(|n| n > 0),
            "{method}: missing timed computation: {stats}"
        );
        for phase in ["lock_wait", "held"] {
            assert!(
                metric[phase]["count"].as_u64().is_some_and(|n| n > 0),
                "{method}: missing {phase} computation: {stats}"
            );
            assert_eq!(metric[phase]["sum_ms"], 0.0, "{method}: {stats}");
            assert_eq!(metric[phase]["max_ms"], 0.0, "{method}: {stats}");
        }
        eprintln!(
            "{}",
            json!({"method":method,"app_mutex_held_through_reply":true,
            "reply_us":reply_elapsed.as_micros(),"request_trace_marks":marks.len(),"metric":metric})
        );
        reply
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_get_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read("capture.get", json!({ "capture_id": reads.capture }))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["text"], "native capture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_get_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read(
            "task.get",
            json!({ "task_id": reads.task, "thread_limit": 1 }),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["goal"], "native read goal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_stages_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read("task.stages", json!({ "task_id": reads.task }))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["stages"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_columns_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read("tasks.columns", json!({ "project_id": reads.project }))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(!answer["result"]["columns"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_events_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read(
            "stream.events",
            json!({ "stream_id": "stream-native-read" }),
        )
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["complete"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_state_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read("stream.state", json!({ "stream_id": "stream-native-read" }))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["output_len"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_snapshots_refuse_unknown_and_malformed_scopes_without_app_lock() {
    let mut reads = NativeReads::new();
    for (method, field) in [
        ("capture.get", "capture_id"),
        ("task.get", "task_id"),
        ("task.stages", "task_id"),
        ("tasks.columns", "project_id"),
        ("stream.events", "stream_id"),
        ("stream.state", "stream_id"),
    ] {
        for params in [json!({}), json!({field: "missing"}), json!({field: 9})] {
            let answer = reads.held_read(method, params.clone()).await;
            assert_eq!(answer["ok"], false, "{method} {params}: {answer}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_snapshots_keep_the_qa_only_gate_without_app_lock() {
    let mut reads = NativeReads::configured(false);
    for method in ["stream.events", "stream.state"] {
        let answer = reads
            .held_read(method, json!({"stream_id":"stream-native-read"}))
            .await;
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["error_code"], "unknown_method", "{answer}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capture_snapshot_is_current_after_save_and_retirement() {
    let mut reads = NativeReads::new();
    {
        let mut app = reads.state.lock().unwrap();
        let mut capture = app.captures[&reads.capture].clone();
        capture.text = "saved replacement".into();
        app.save_capture(capture).unwrap();
    }
    let answer = reads
        .held_read("capture.get", json!({"capture_id":reads.capture}))
        .await;
    assert_eq!(answer["result"]["text"], "saved replacement", "{answer}");
    reads
        .state
        .lock()
        .unwrap()
        .capture_cancel(&json!({"capture_id":reads.capture}))
        .unwrap();
    let refused = reads
        .held_read("capture.get", json!({"capture_id":reads.capture}))
        .await;
    assert_eq!(refused["ok"], false, "{refused}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_snapshot_is_current_after_a_committed_mutation() {
    let mut reads = NativeReads::new();
    {
        let mut app = reads.state.lock().unwrap();
        let mut plan = app.plans.remove(&reads.task).unwrap();
        plan.plan.goal = "committed replacement".into();
        app.finish_plan_mutation(reads.task.clone(), plan).unwrap();
    }
    let answer = reads
        .held_read("task.get", json!({"task_id":reads.task}))
        .await;
    assert_eq!(
        answer["result"]["goal"], "committed replacement",
        "{answer}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_snapshot_preserves_named_conversation_pages_and_stale_refusals() {
    let mut reads = NativeReads::new();
    let (agent, conversation) = {
        let mut app = reads.state.lock().unwrap();
        let mut plan = app.plans.remove(&reads.task).unwrap();
        let agent = plan.agents.sole_mut();
        agent
            .thread
            .post_user("first message", None, "2026-10-21T12:00:00Z");
        agent
            .thread
            .post_user("second message", None, "2026-10-21T12:00:01Z");
        let address = (agent.id.clone(), agent.conversation_id().to_string());
        app.finish_plan_mutation(reads.task.clone(), plan).unwrap();
        address
    };
    for window in [
        json!({}),
        json!({"thread_limit":1}),
        json!({"thread_after_sequence":0}),
    ] {
        let mut params =
            json!({"task_id":reads.task,"agent_id":agent,"conversation_id":conversation});
        params
            .as_object_mut()
            .unwrap()
            .extend(window.as_object().unwrap().clone());
        let normalized = crate::api::v1::normalize_read_params("task.get", &params).unwrap();
        let expected = reads.state.lock().unwrap().plan_get(&normalized).unwrap();
        let answer = reads.held_read("task.get", params).await;
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["result"], expected);
    }
    for assertion in [
        json!({"agent_id":"missing"}),
        json!({"agent_id":""}),
        json!({"conversation_id":"stale"}),
        json!({"conversation_id":""}),
    ] {
        let mut params = json!({"task_id":reads.task});
        params
            .as_object_mut()
            .unwrap()
            .extend(assertion.as_object().unwrap().clone());
        let answer = reads.held_read("task.get", params).await;
        assert_eq!(answer["ok"], false, "{answer}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_snapshots_publish_each_append_completion_and_bounded_replay() {
    let mut reads = NativeReads::new();
    let handler = AppState::handler(Arc::clone(&reads.state));
    let started = call(
        &handler,
        "stream.start",
        json!({"count":300,"interval_ms":0}),
    );
    assert_eq!(started["ok"], true, "{started}");
    let stream = started["result"]["stream_id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if reads.state.lock().unwrap().streams[&stream].complete {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture producer completes");
    let first = reads
        .held_read("stream.events", json!({"stream_id":stream,"limit":999}))
        .await;
    assert_eq!(first["ok"], true, "{first}");
    assert_eq!(first["result"]["events"].as_array().unwrap().len(), 256);
    assert_eq!(first["result"]["next"], 256);
    assert_eq!(first["result"]["head"], 301);
    assert_eq!(first["result"]["complete"], true);
    let tail = reads
        .held_read(
            "stream.events",
            json!({"stream_id":stream,"since":256,"limit":256}),
        )
        .await;
    assert_eq!(tail["result"]["events"].as_array().unwrap().len(), 45);
    assert_eq!(tail["result"]["next"], 301);
    assert_eq!(tail["result"]["events"][44]["kind"], "done");
    let output = (0..300)
        .map(crate::app::streams::chunk_text)
        .collect::<Vec<_>>()
        .join("\n");
    let summary = reads
        .held_read("stream.state", json!({"stream_id":stream}))
        .await;
    assert_eq!(summary["result"]["count"], 300);
    assert_eq!(summary["result"]["output_len"], output.len());
    assert_eq!(summary["result"]["checksum"], sha256_hex(output.as_bytes()));
    assert_eq!(summary["result"]["complete"], true);
    let ahead = reads
        .held_read(
            "stream.events",
            json!({"stream_id":stream,"since":u64::MAX}),
        )
        .await;
    assert_eq!(ahead["result"]["events"], json!([]));
    assert_eq!(ahead["result"]["next"], u64::MAX);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_stages_snapshot_refreshes_committed_manifest_and_retires_with_owner() {
    let mut reads = NativeReads::new();
    {
        let mut app = reads.state.lock().unwrap();
        let mut plan = app.plans.remove(&reads.task).unwrap();
        plan.stages.push(crate::plan::StageDoc {
            id: "stage-native".into(),
            title: "Published stage".into(),
            path: ".build/plan/native.md".into(),
            summary: "stage snapshot".into(),
            state: crate::plan::StageDocState::Approved,
        });
        app.finish_plan_mutation(reads.task.clone(), plan).unwrap();
    }
    let answer = reads
        .held_read("task.stages", json!({"task_id":reads.task}))
        .await;
    assert_eq!(
        answer["result"]["stages"][0]["title"], "Published stage",
        "{answer}"
    );
    assert_eq!(answer["result"]["stages"][0]["approval"], "approved");
    assert_eq!(answer["result"]["stages"][0]["execution"], "pending");
    {
        let mut app = reads.state.lock().unwrap();
        app.plans.remove(&reads.task).unwrap();
        app.committed_entities.forget(&reads.task);
        app.forget_read_owner(&reads.task);
    }
    for method in ["task.get", "task.stages"] {
        let answer = reads.held_read(method, json!({"task_id":reads.task})).await;
        assert_eq!(answer["ok"], false, "{method}: {answer}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_columns_snapshot_preserves_project_cleanup_refusal() {
    let mut reads = NativeReads::new();
    {
        let mut app = reads.state.lock().unwrap();
        app.project_deletion_in_progress = true;
        app.publish_read_metadata();
    }
    let refused = reads
        .held_read("tasks.columns", json!({"project_id":reads.project}))
        .await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["error_code"], "unavailable");
    {
        let mut app = reads.state.lock().unwrap();
        app.project_deletion_in_progress = false;
        app.publish_read_metadata();
    }
    let answered = reads
        .held_read("tasks.columns", json!({"project_id":reads.project}))
        .await;
    assert_eq!(answered["ok"], true, "{answered}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn term_list_answers_content_without_app_lock() {
    let mut reads = NativeReads::new();
    let answer = reads
        .held_read("term.list", json!({"project_id":reads.project}))
        .await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["terminals"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_deletion_retires_its_published_captures() {
    let mut reads = NativeReads::new();
    let repository = {
        let mut app = reads.state.lock().unwrap();
        app.config_path = Some(reads._directory.path().join("config.json"));
        let routed = app.captures[&reads.capture].routed_to(crate::capture::CaptureRouting {
            project_id: reads.project.clone(),
            kind: crate::capture::CaptureTarget::Task,
            target_id: reads.task.clone(),
            routed_at: now_rfc3339(),
            rationale: None,
        });
        app.save_capture(routed).unwrap();
        app.project_at(0).repo_path.clone()
    };
    let before = reads
        .held_read("capture.get", json!({"capture_id":reads.capture}))
        .await;
    assert_eq!(before["ok"], true, "{before}");
    let handler = AppState::handler(Arc::clone(&reads.state));
    let deleted = call(
        &handler,
        "project.delete",
        json!({"project_id":reads.project,"confirm":true}),
    );
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(
        repository.is_dir(),
        "deletion preserves the configured source"
    );
    let after = reads
        .held_read("capture.get", json!({"capture_id":reads.capture}))
        .await;
    assert_eq!(after["ok"], false, "{after}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn borrowed_task_keeps_document_availability_across_maintenance() {
    let mut reads = NativeReads::new();
    {
        let mut app = reads.state.lock().unwrap();
        let docs = app.store.as_ref().unwrap().plan_docs_dir(&reads.task);
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("plan.md"), "canonical plan").unwrap();
        app.refresh_owner_snapshot_observations(&reads.task);
        app.publish_owner_snapshot_projection(&reads.task);
    }
    let before = reads
        .held_read("task.get", json!({"task_id":reads.task}))
        .await;
    assert_eq!(before["result"]["docs_available"], true, "{before}");
    {
        let mut app = reads.state.lock().unwrap();
        let borrowed = app.plans.remove(&reads.task).unwrap();
        let _maintenance = app.refresh_snapshot_observations();
        app.plans.insert(reads.task.clone(), borrowed);
        app.publish_owner_snapshot_projection(&reads.task);
    }
    let after = reads
        .held_read("task.get", json!({"task_id":reads.task}))
        .await;
    assert_eq!(after["result"]["docs_available"], true, "{after}");
}
