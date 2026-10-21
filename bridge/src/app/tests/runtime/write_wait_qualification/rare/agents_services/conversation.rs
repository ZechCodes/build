use super::provider::FiniteProvider;
use super::*;
use crate::operation::OperationStatus;
use crate::thread::{MessageDeliveryStatus, ThreadItem};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn selector(fixture: &RareFixture, owner: &str, id: &str) -> Value {
    let app = fixture.state().lock().unwrap();
    let agent = app.entity_agents(owner).unwrap().by_id(id).unwrap();
    json!({"entity_id":owner,"agent_id":id,"conversation_id":agent.conversation_id(),"thread_id":agent.thread.id})
}

fn param(mut selector: Value, name: &str, value: Value) -> Value {
    selector[name] = value;
    selector
}

fn roster_selector(fixture: &RareFixture, owner: &str, id: &str) -> Value {
    let mut params = selector(fixture, owner, id);
    params.as_object_mut().unwrap().remove("conversation_id");
    params
}

async fn digest(fixture: &RareFixture, owner: &str, id: &str) -> Value {
    let read = fixture.call("agent.list", json!({"entity_id":owner})).await;
    read.reply["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
        .clone()
}

fn agent_news(fixture: &RareFixture, owner: &str, id: &str, body: &str) -> u64 {
    let mut app = fixture.state().lock().unwrap();
    app.edit_agent_conversation(owner, id, |thread, _| {
        thread.post_agent(body, None, now_rfc3339());
        Ok(thread.last_sequence())
    })
    .unwrap()
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let provider = FiniteProvider::install(directory.path());
    provider.require_enrolled();
    let mut app = qa_state(&repository, directory.path());
    app.default_harness = AgentProvider::Claude;
    let owner = adopted_run(
        &mut app,
        &repository,
        directory.path(),
        "rare-conversations",
    );
    let project = app.projects.project_id_of(&owner).unwrap().to_owned();
    let before_count = app.entity_agents(&owner).unwrap().agents().len();
    let mut fixture = RareFixture::new("agents-conversations-attention", app);
    let cleanup = EndFixtureSessions(Arc::clone(fixture.state()));

    let added = fixture.call("agent.add", json!({"entity_id":owner,"creation_id":"rare-secondary-agent","name":"Qualification","provider":"claude"})).await;
    let id = added.reply["result"]["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    {
        let app = fixture.state().lock().unwrap();
        assert_eq!(
            app.entity_agents(&owner).unwrap().agents().len(),
            before_count + 1
        );
        assert_eq!(
            app.entity_agents(&owner)
                .unwrap()
                .by_id(&id)
                .unwrap()
                .name
                .as_deref(),
            Some("Qualification")
        );
        let stored = app.store.as_ref().unwrap().load_all_runs().unwrap();
        assert!(stored
            .iter()
            .any(|run| run.id == owner && run.agents.iter().any(|agent| agent.id == id)));
    }
    fixture
        .verify(
            added,
            json!({"agent_id":id,"roster_before":before_count,"roster_after":before_count+1}),
        )
        .await;

    let old = digest(&fixture, &owner, &id).await;
    let mut choice = selector(&fixture, &owner, &id);
    choice["model"] = json!("claude-opus-5");
    choice["effort"] = json!("high");
    choice["expected_choice_revision"] = old["choice_revision"].clone();
    let chosen = fixture.call("agent.choose", choice).await;
    let current = digest(&fixture, &owner, &id).await;
    assert_eq!(current["model"], "claude-opus-5");
    assert_eq!(current["effort"], "high");
    assert_eq!(current["provider"], "claude");
    assert_eq!(
        current["choice_revision"].as_u64().unwrap(),
        old["choice_revision"].as_u64().unwrap() + 1
    );
    fixture.verify(chosen, json!({"agent_id":id,"previous_revision":old["choice_revision"],"saved_choice":current})).await;

    let mut start = selector(&fixture, &owner, &id);
    start["id"] = json!(owner);
    let started = fixture.call("agent.start", start).await;
    let published = wait_for(Duration::from_secs(2), || {
        let app = fixture.state().lock().unwrap();
        let root = AppState::canonical_root(&app.runs.get(&owner)?.worktree.path);
        let tab = app.session_registry.test_tab(&TabKey::agent(&root, &id))?;
        app.delivery_queue
            .is_idle()
            .then_some((root, tab.session_instance.clone()?))
    })
    .await;
    let (root, lineage) = published.expect("finite start did not publish its tab and lineage");
    {
        let app = fixture.state().lock().unwrap();
        let tab = app
            .session_registry
            .test_tab(&TabKey::agent(&root, &id))
            .unwrap();
        assert_eq!(app.session_registry.test_counts().claims, 0);
        assert_eq!(json!(tab.wire_id()), started.reply["result"]["term_id"]);
    }
    assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    fixture.verify(started, json!({"agent_id":id,"root":root,"session_instance":lineage.id,"settled_spawn_claims":0})).await;

    let settings = fixture
        .call(
            "conversation.settings",
            param(
                roster_selector(&fixture, &owner, &id),
                "max_context_tokens",
                json!(150_000),
            ),
        )
        .await;
    let limit = digest(&fixture, &owner, &id).await;
    assert_eq!(limit["max_context_tokens"], 150_000);
    fixture
        .verify(
            settings,
            json!({"agent_id":id,"previous_limit":null,"saved_limit":150_000}),
        )
        .await;

    let unwatched = fixture
        .call(
            "conversation.unwatch",
            roster_selector(&fixture, &owner, &id),
        )
        .await;
    assert_eq!(unwatched.reply["result"]["watched"], false);
    {
        let app = fixture.state().lock().unwrap();
        assert!(
            !app.entity_agents(&owner)
                .unwrap()
                .by_id(&id)
                .unwrap()
                .watched
        );
        let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
        assert!(
            !saved
                .iter()
                .find(|run| run.id == owner)
                .unwrap()
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .unwrap()
                .watched
        );
    }
    fixture
        .verify(
            unwatched,
            json!({"agent_id":id,"watched_before":true,"watched_after":false}),
        )
        .await;
    let old_news = agent_news(&fixture, &owner, &id, "old unwatched news");
    let watched = fixture
        .call("conversation.watch", roster_selector(&fixture, &owner, &id))
        .await;
    let baseline = digest(&fixture, &owner, &id).await;
    assert_eq!(watched.reply["result"]["watched"], true);
    {
        let app = fixture.state().lock().unwrap();
        let agent = app.entity_agents(&owner).unwrap().by_id(&id).unwrap();
        assert!(agent.watched);
        assert_eq!(agent.watch_started_after, old_news);
        let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
        let agent = saved
            .iter()
            .find(|run| run.id == owner)
            .unwrap()
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .unwrap();
        assert!(agent.watched);
        assert_eq!(agent.watch_started_after, old_news);
    }
    assert_eq!(baseline["unread_count"], 0);
    fixture.verify(watched,json!({"agent_id":id,"watched_before":false,"watched_after":true,"baseline_unread_count":0})).await;

    let sequence = agent_news(&fixture, &owner, &id, "new watched news");
    let loud = digest(&fixture, &owner, &id).await;
    assert_eq!(loud["unread_count"], 1);
    let muted = fixture
        .call("entity.mute", json!({"entity_id":owner,"muted":true}))
        .await;
    let quiet = digest(&fixture, &owner, &id).await;
    assert_eq!(quiet["unread_count"], 0);
    assert_eq!(
        quiet["read_through_sequence"],
        loud["read_through_sequence"]
    );
    fixture.verify(muted,json!({"owner":owner,"unread_before":1,"unread_after":0,"cursor_unchanged":quiet["read_through_sequence"]})).await;
    fixture
        .call("entity.mute", json!({"entity_id":owner,"muted":false}))
        .await;

    let seen = fixture
        .call("entity.seen", selector(&fixture, &owner, &id))
        .await;
    let read = digest(&fixture, &owner, &id).await;
    assert_eq!(read["unread_count"], 0);
    assert_eq!(read["read_through_sequence"], sequence);
    fixture.verify(seen,json!({"agent_id":id,"unread_before":1,"unread_after":0,"read_through_sequence":sequence})).await;

    let before_dismiss = {
        let mut app = fixture.state().lock().unwrap();
        board_entry(&mut app, &owner)
    };
    assert_eq!(before_dismiss["dismissed"], false);
    let dismissed = fixture
        .call("entity.dismiss", json!({"entity_id":owner}))
        .await;
    let after_dismiss = {
        let mut app = fixture.state().lock().unwrap();
        board_entry(&mut app, &owner)
    };
    assert_eq!(after_dismiss["dismissed"], true);
    assert_eq!(
        digest(&fixture, &owner, &id).await["read_through_sequence"],
        sequence
    );
    fixture.verify(dismissed,json!({"owner":owner,"dismissed_before":false,"dismissed_after":true,"read_cursor":sequence})).await;

    let attach_params = param(
        param(
            selector(&fixture, &owner, &id),
            "filename",
            json!("qualification.txt"),
        ),
        "content_b64",
        json!(b64encode(b"qualification bytes")),
    );
    let attached = fixture.call("thread.attach", attach_params).await;
    let attachment = attached.reply["result"].clone();
    let leaf = attachment["path"].as_str().unwrap().to_owned();
    assert!(leaf.starts_with(".build/attachments/"));
    assert_eq!(
        std::fs::read(root.join(&leaf)).unwrap(),
        b"qualification bytes"
    );
    fixture
        .verify(
            attached,
            json!({"agent_id":id,"path":leaf,"exact_bytes_b64":b64encode(b"qualification bytes")}),
        )
        .await;

    let mut post = selector(&fixture, &owner, &id);
    post["body"] = json!("read the qualification file");
    post["attachments"] = json!([attachment]);
    post["operation_id"] = json!("rare-post-operation");
    let posted = fixture.call("thread.post", post).await;
    wait_for(Duration::from_secs(2), || {
        provider
            .log
            .turns()
            .iter()
            .any(|turn| turn.contains("read the qualification file"))
            .then_some(())
    })
    .await
    .expect("finite provider did not receive the actual posted turn");
    provider.consumed("rare-post-operation");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let consumed = {
            let app = fixture.state().lock().unwrap();
            let operation = app
                .operation_receipt("rare-post-operation")
                .unwrap()
                .unwrap();
            operation.status==OperationStatus::Delivered && app.agent_conversation(&owner,Some(&id)).unwrap().items.iter().any(|item|matches!(item,ThreadItem::Message(message) if message.operation_id.as_deref()==Some("rare-post-operation") && message.delivery_status==Some(MessageDeliveryStatus::Seen)))
        };
        if consumed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "native receipt did not settle posted message"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    fixture.verify(posted,json!({"agent_id":id,"operation_id":"rare-post-operation","delivered_turns":provider.log.turns().len(),"delivery_status":"seen"})).await;

    provider.status(AgentStatus::Working);
    let interrupted = fixture
        .call("agent.interrupt", selector(&fixture, &owner, &id))
        .await;
    assert_eq!(interrupted.reply["result"]["interrupted"], true);
    assert!(provider.log.interrupted());
    provider.status(AgentStatus::Waiting);
    fixture
        .verify(
            interrupted,
            json!({"agent_id":id,"interrupted":true,"session_retained":true}),
        )
        .await;

    let old_thread = selector(&fixture, &owner, &id)["thread_id"].clone();
    let mut reset = selector(&fixture, &owner, &id);
    reset["project_id"] = json!(project);
    reset["expected_thread_id"] = old_thread.clone();
    reset.as_object_mut().unwrap().remove("thread_id");
    let reset = fixture.call("conversation.reset", reset).await;
    let current = selector(&fixture, &owner, &id);
    assert_ne!(current["thread_id"], old_thread);
    {
        let app = fixture.state().lock().unwrap();
        let agent = app.entity_agents(&owner).unwrap().by_id(&id).unwrap();
        assert!(agent.thread.items.is_empty());
        assert_eq!(agent.max_context_tokens, Some(150_000));
        assert!(app
            .store
            .as_ref()
            .unwrap()
            .operation("rare-post-operation")
            .unwrap()
            .is_none());
        assert_eq!(
            app.store.as_ref().unwrap().thread_item_count(&id).unwrap(),
            0
        );
    }
    assert!(!root.join(&leaf).exists());
    fixture.verify(reset,json!({"agent_id":id,"old_thread":old_thread,"new_thread":current["thread_id"],"retained_limit":150_000,"history_items":0,"operation_removed":true})).await;

    let removed = fixture
        .call("agent.remove", roster_selector(&fixture, &owner, &id))
        .await;
    {
        let app = fixture.state().lock().unwrap();
        assert!(app.entity_agents(&owner).unwrap().by_id(&id).is_none());
        assert!(!app.board.attention().has_agent_cursor(&owner, &id));
        assert_eq!(
            app.entity_agents(&owner).unwrap().agents().len(),
            before_count
        );
    }
    assert!(provider.log.ended());
    fixture.verify(removed,json!({"removed_agent":id,"roster_before":before_count+1,"roster_after":before_count,"session_ended":true})).await;
    drop(cleanup);
    let steps = fixture.finish().await;
    drop(provider);
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_conversation_and_attention_mutations_have_fourteen_effect_proofs() {
    let steps = execute().await;
    assert_eq!(steps.len(), 14);
    assert!(steps
        .iter()
        .all(|step| step.reply["ok"] == true && step.mcp_action.is_none()));
}
