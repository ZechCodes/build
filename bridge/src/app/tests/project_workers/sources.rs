//! A worker may write to any of its project's sources, so while one holds
//! them none may move or leave; and while one is moving, no worker starts.

use super::super::project_agent_base::record_spawns;
use super::super::project_coordinator::refused_with;
use super::*;

/// A project over two git sources, `code` (its home) and `docs`, with its
/// owner and one worker.
struct TwoSources {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    state: AppState,
    project_id: String,
    owner: String,
    pin: String,
    worker: String,
}

impl TwoSources {
    fn new() -> Self {
        let (tmp, root) = a_state_root();
        let code = init_repo_named(&root, "code");
        let docs = init_repo_named(&root, "docs");
        std::fs::create_dir_all(root.join("state")).unwrap();
        let mut state = booted(&root.join("state"));
        let opened = state.handle(req(
            "project.create",
            json!({ "name": "pair", "sources": [{ "path": code }, { "path": docs }] }),
        ));
        assert_eq!(opened["ok"], true, "{opened:?}");
        let project_id = opened["result"]["project_id"].as_str().unwrap().to_string();
        let owner = ensure(&mut state, &project_id);
        let pin = state.project_pin(&owner).unwrap().to_string();
        let worker = add_worker(&mut state, &owner, json!({ "name": "Editor" }));
        Self {
            _tmp: tmp,
            root,
            state,
            project_id,
            owner,
            pin,
            worker,
        }
    }

    fn remove_docs(&mut self) -> Value {
        self.state.handle(req(
            "project.remove_source",
            json!({ "project_id": self.project_id, "source_id": "source-2" }),
        ))
    }

    /// Move `docs` to another repository beside it.
    fn move_params(&self) -> Value {
        let moved = self.root.join("docs-moved");
        if !moved.exists() {
            init_repo_named(&self.root, "docs-moved");
        }
        json!({ "project_id": self.project_id, "source_id": "source-2", "path": moved })
    }

    fn move_docs(&mut self) -> Value {
        let params = self.move_params();
        self.state.handle(req("project.update_source", params))
    }

    fn sources(&self) -> Vec<String> {
        self.state
            .projects
            .get(&self.project_id)
            .unwrap()
            .sources
            .iter()
            .map(|source| source.path.display().to_string())
            .collect()
    }
}

/// A turn queued for a worker — the delivery that will start it — holds
/// every source against a move and a removal, by the user or through the
/// pin's tool, and the refusal names the worker. A label is no move. Once
/// nothing holds them, the same move and removal go through.
#[test]
fn a_queued_worker_turn_holds_every_source() {
    let mut f = TwoSources::new();
    let posted = f.state.handle(req(
        "thread.post",
        json!({ "entity_id": f.owner, "agent_id": f.worker, "body": "edit the docs" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let before = f.sources();

    let removed = f.remove_docs();
    refused_with(&removed, "conflict");
    assert!(
        removed["error"]
            .as_str()
            .unwrap()
            .contains("Stop Editor first"),
        "{removed:?}"
    );
    refused_with(&f.move_docs(), "conflict");
    let by_tool = f
        .state
        .on_agent_mcp_action(
            &f.owner.clone(),
            &f.pin.clone(),
            crate::mcp::BridgeAction::RemoveProjectSource {
                source_id: "source-2".into(),
            },
        )
        .unwrap_err();
    assert!(by_tool.contains("Stop Editor first"), "{by_tool}");
    assert_eq!(f.sources(), before, "nothing moved");
    let relabelled = f.state.handle(req(
        "project.update_source",
        json!({ "project_id": f.project_id, "source_id": "source-2", "name": "Docs" }),
    ));
    assert_eq!(relabelled["ok"], true, "a label is no move: {relabelled:?}");

    let worker = f.worker.clone();
    f.state
        .delivery_queue
        .retain_queued(|turn| turn.agent_id != worker);
    let moved = f.move_docs();
    assert_eq!(moved["ok"], true, "{moved:?}");
    let removed = f.remove_docs();
    assert_eq!(removed["ok"], true, "{removed:?}");
}

/// A running worker holds the sources; the pin, which only reads, does not
/// hold them by being there.
#[tokio::test]
async fn a_running_worker_holds_the_sources_until_it_stops() {
    let f = TwoSources::new();
    let scratch = AppState::canonical_root(&f.state.runs[&f.owner].worktree.path);
    let TwoSources {
        _tmp,
        root,
        state,
        project_id,
        owner,
        worker,
        ..
    } = f;
    let shared = state.shared();
    let _spawns = record_spawns(&shared);
    ensure_agent_tab(
        &shared,
        &scratch,
        &owner,
        &worker,
        &ModelChoice::default(),
        "start",
    )
    .expect("the worker starts");
    let moved = init_repo_named(&root, "docs-moved");
    let move_docs = || {
        shared.lock().unwrap().handle(req(
            "project.update_source",
            json!({ "project_id": project_id, "source_id": "source-2", "path": moved }),
        ))
    };
    refused_with(&move_docs(), "conflict");

    shared.lock().unwrap().retire_agent(&scratch, &worker);
    let answered = move_docs();
    assert_eq!(answered["ok"], true, "{answered:?}");
}

/// While a source is moving, no worker is started into the folders changing
/// under it; the pin still starts, and once the move settles the worker does.
#[test]
fn a_moving_source_holds_worker_starts_until_it_settles() {
    let mut f = TwoSources::new();
    let params = f.move_params();
    f.state.project_update_source(&params).unwrap();
    let refused = f
        .state
        .refuse_worker_during_source_move(&f.owner, &f.worker)
        .unwrap_err();
    assert!(refused.contains("folders is moving"), "{refused}");
    f.state
        .refuse_worker_during_source_move(&f.owner, &f.pin)
        .expect("the pin is no worker");

    let deferred = f.state.take_deferred().expect("the move runs off the lock");
    let done = deferred.run();
    let settled = f
        .state
        .apply_deferred("project.update_source", &params, done);
    assert!(settled.is_ok(), "{settled:?}");
    f.state
        .refuse_worker_during_source_move(&f.owner, &f.worker)
        .expect("the move has settled");
}
