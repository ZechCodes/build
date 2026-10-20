use super::*;
use crate::app::snapshot_projection::work_counts;

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    owner: String,
    agents: Vec<String>,
    workspace: String,
    external: String,
}

impl Fixture {
    fn new() -> Self {
        let (directory, repository) = init_repo();
        let mut app = qa_state(&repository, directory.path());
        let owner = adopted_run(&mut app, &repository, directory.path(), "attention-owner");
        let other = adopted_run(&mut app, &repository, directory.path(), "attention-other");
        app.agent_add(&json!({"entity_id":owner})).unwrap();
        let mut run = app.take_run(&owner).unwrap();
        run.run.state = crate::run::RunState::Review;
        let mut agents = Vec::new();
        for agent in run.agents.iter_mut() {
            agent
                .thread
                .post_agent("needs a decision", None, now_rfc3339());
            agents.push(agent.id.clone());
        }
        app.finish_run_mutation(owner.clone(), run).unwrap();
        let project = app.project_at(0).id.clone();
        let workspace = "attention-workspace".to_owned();
        for (id, live) in [
            (workspace.clone(), owner.clone()),
            ("attention-other-workspace".into(), other),
        ] {
            app.workspaces.adopt_root(
                &project,
                id.clone(),
                id,
                app.runs[&live].worktree.path.clone(),
                "source-1".into(),
                false,
            );
        }
        add_external_worktree(
            &repository,
            directory.path(),
            "attention-external",
            "attention-external",
        );
        let external = app
            .scan_external_worktrees_now(&project)
            .unwrap()
            .into_iter()
            .find(|row| row.branch.as_deref() == Some("attention-external"))
            .unwrap()
            .id;
        let created = "2026-10-20T12:00:00Z";
        app.board
            .attention_mut()
            .record_created_if_absent(&external, created.into());
        app.board.attention_mut().seed_anchor(&external, created);
        app.persist_attention();
        app.initialize_snapshot_projection();
        app.publish_all_snapshot_projection();
        Self {
            _directory: directory,
            state: Arc::new(Mutex::new(app)),
            owner,
            agents,
            workspace,
            external,
        }
    }

    fn mutate(&self, action: impl FnOnce(&mut AppState)) -> (work_counts::Counts, Value) {
        let clock = Arc::clone(&self.state.lock().unwrap().frame_clock);
        let samples = clock.capture_phases();
        let timer = clock.frame("agent.deliver");
        let (_, count) = work_counts::measure(|| action(&mut timer.lock(&self.state)));
        drop(timer);
        (count, samples.snapshot())
    }

    fn assert_full_publication_parity(&self) {
        let app = self.state.lock().unwrap();
        let incremental = app.read_view.load();
        app.publish_all_snapshot_projection();
        let full = app.read_view.load();
        let keys = incremental
            .keys()
            .map(|key| (key.method, key.owner.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            full.keys()
                .map(|key| (key.method, key.owner.clone()))
                .collect::<Vec<_>>()
        );
        for (method, owner) in keys {
            assert_eq!(
                incremental.get(method, &owner),
                full.get(method, &owner),
                "{method}/{owner}"
            );
        }
    }

    fn agent(&self, agent: &str) -> Value {
        let app = self.state.lock().unwrap();
        app.read_view.load().get("agent.list", &self.owner).unwrap()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == agent)
            .unwrap()
            .clone()
    }
}

#[test]
fn known_owner_attention_publication_never_requests_all_workspace_details() {
    for action in ["interaction", "user_message", "working", "seen"] {
        let fixture = Fixture::new();
        let revision = fixture.state.lock().unwrap().changes.board_revision();
        let (count, samples) = fixture.mutate(|app| match action {
            "interaction" => app.touch_attention(&fixture.owner),
            "user_message" => app.note_user_message(&fixture.owner),
            "working" => app.observe_working_state(&fixture.owner, true, "2026-10-20T12:00:00Z"),
            "seen" => {
                app.see_attention(&fixture.owner, Some(&fixture.agents[0]), Default::default())
            }
            _ => unreachable!(),
        });
        assert_eq!(
            fixture.state.lock().unwrap().changes.board_revision(),
            revision + 1
        );
        fixture.assert_full_publication_parity();
        eprintln!("{action}: {count:?}; {samples}");
        assert_eq!(
            count.workspace_details_full, 0,
            "{action}: full workspace scan"
        );
        assert!(
            count.workspace_details_scoped > 0,
            "{action}: no owner details"
        );
        assert!(
            samples["agent.deliver"]["entered.publication.global"]["count"]
                .as_u64()
                .unwrap_or(0)
                <= 1
        );
        assert_eq!(
            samples["agent.deliver"]["entered.publication.install"]["count"],
            1
        );
    }
}

#[test]
fn attention_nonowner_ids_keep_the_full_publication_fallback() {
    let fixture = Fixture::new();
    for id in [
        "unknown-attention-row",
        &fixture.workspace,
        &fixture.external,
    ] {
        let (count, samples) = fixture.mutate(|app| app.touch_attention(id));
        assert!(count.workspace_details_full > 0, "{id}: lost full fallback");
        assert_eq!(
            samples["agent.deliver"]["entered.publication.global"]["count"],
            1
        );
        fixture.assert_full_publication_parity();
    }
}

#[test]
fn attention_scoping_keeps_seen_mute_dismiss_and_durable_pruning() {
    let fixture = Fixture::new();
    let before = fixture.state.lock().unwrap().read_view.load();
    fixture.mutate(|app| {
        app.see_attention(&fixture.owner, Some(&fixture.agents[0]), Default::default())
    });
    assert_eq!(fixture.agent(&fixture.agents[0])["unread_count"], 0);
    assert_eq!(fixture.agent(&fixture.agents[1])["unread_count"], 1);
    assert_eq!(
        before.get("agent.list", &fixture.owner).unwrap()["agents"][0]["unread_count"],
        1
    );
    let cursor = fixture.agent(&fixture.agents[0])["read_through_sequence"].clone();
    for (muted, unread) in [(true, 0), (false, 1)] {
        fixture.mutate(|app| {
            app.entity_mute(&json!({"entity_id":fixture.owner,"muted":muted}))
                .unwrap();
        });
        assert_eq!(fixture.agent(&fixture.agents[1])["unread_count"], unread);
        assert_eq!(
            fixture.agent(&fixture.agents[0])["read_through_sequence"],
            cursor
        );
        fixture.assert_full_publication_parity();
    }
    fixture.mutate(|app| {
        app.entity_dismiss(&json!({"entity_id":fixture.owner}))
            .unwrap();
    });
    assert_eq!(fixture.agent(&fixture.agents[1])["unread_count"], 1);
    fixture.assert_full_publication_parity();
    let mut app = fixture.state.lock().unwrap();
    app.board
        .attention_mut()
        .interact("gone-attention-owner", "old");
    app.touch_attention(&fixture.owner);
    let saved = app.store.as_ref().unwrap().load_attention();
    assert!(saved.contains_key(&fixture.owner));
    assert!(!saved.contains_key("gone-attention-owner"));
}
