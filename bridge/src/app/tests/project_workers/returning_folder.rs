//! A project whose folder is away when the bridge boots is kept, not
//! forgotten: its configuration survives every save, a worker's start is
//! refused in words that say why, and the moment the folder is back the
//! project, its owner, its pin and its workers answer again with nothing
//! repaired by hand.

use super::super::project_agent_base::only_spawn;
use super::*;

/// A project with its owner, pin and one worker, written down and closed:
/// `(project_id, owner, pin, worker, scratch)`.
fn a_stored_project(state_root: &Path, repo: &Path) -> (String, String, String, String, PathBuf) {
    let mut state = booted(state_root);
    let project_id = added_project(&mut state, repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({ "name": "Editor" }));
    let scratch = AppState::canonical_root(&state.runs[&owner].worktree.path);
    (project_id, owner, pin, worker, scratch)
}

fn configured_ids(state_root: &Path) -> Vec<String> {
    let config: Value =
        serde_json::from_slice(&std::fs::read(state_root.join("config.json")).unwrap()).unwrap();
    config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| project["id"].as_str().unwrap().to_string())
        .collect()
}

fn listed_ids(state: &mut AppState) -> Vec<String> {
    state.project_list()["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| project["project_id"].as_str().unwrap().to_string())
        .collect()
}

/// The folder is away at boot: the project is kept in the configuration
/// through a save, and its worker is refused a start naming the folder.
/// The folder comes back within the same boot: the same worker starts in
/// it, and the project, owner and pin answer as they did, unrepaired.
#[tokio::test]
async fn a_worker_starts_once_its_project_folder_returns() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let (project_id, owner, pin, worker, scratch) = a_stored_project(&state_root, &repo);
    let away = repo.with_extension("away");
    std::fs::rename(&repo, &away).unwrap();

    let mut state = booted(&state_root);
    assert!(listed_ids(&mut state).is_empty(), "nothing to show yet");
    state.persist();
    assert_eq!(
        configured_ids(&state_root),
        vec![project_id.clone()],
        "kept"
    );
    let spawns = record_spawns_of_projects_to_come(&mut state);
    let shared = state.shared();
    let start = || {
        ensure_agent_tab(
            &shared,
            &scratch,
            &owner,
            &worker,
            &ModelChoice::default(),
            "start",
        )
    };
    let refused = start().unwrap_err();
    assert!(refused.contains("project folder"), "{refused}");
    assert!(refused.contains(&repo.display().to_string()), "{refused}");
    assert!(spawns.lock().unwrap().is_empty(), "nothing was started");

    std::fs::rename(&away, &repo).unwrap();
    start().expect("the worker starts once its folder is back");
    assert_eq!(only_spawn(&spawns).cwd, repo);
    let mut app = shared.lock().unwrap();
    assert_eq!(listed_ids(&mut app), vec![project_id.clone()]);
    assert_eq!(
        app.projects.project_id_of(&owner),
        Some(project_id.as_str())
    );
    assert_eq!(ensure(&mut app, &project_id), owner);
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
}

/// Every project loaded from here on launches its agents on a harness that
/// stays up, and every launch is kept: a project still away has no
/// orchestrator to install one on yet.
fn record_spawns_of_projects_to_come(state: &mut AppState) -> Arc<Mutex<Vec<SpawnOptions>>> {
    let spawns: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawns);
    state.agent = Agent::WarmBuilder(Arc::new(
        move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
            recorded.lock().unwrap().push(options.clone());
            Ok(HarnessSpec::new("bash")
                .arg("-c")
                .arg("printf 'started in %s\\n' \"$PWD\"; exec sleep 60"))
        },
    ));
    state.resume_id_probe = Arc::new(|_, _, _| false);
    spawns
}

/// Nobody has to start anything: the project list, the project's own
/// conversation and the sync timer each find the folder back, and the next
/// boot loads the project as it always did.
#[test]
fn a_returned_project_folder_is_found_without_a_start() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let (project_id, owner, pin, _worker, _scratch) = a_stored_project(&state_root, &repo);
    let away = repo.with_extension("away");
    std::fs::rename(&repo, &away).unwrap();

    {
        let mut state = booted(&state_root);
        let refused = state.handle(req(
            "project.ensure_conversation",
            json!({ "project_id": project_id }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        std::fs::rename(&away, &repo).unwrap();
        let listed = state.handle(req("project.list", json!({})));
        assert_eq!(listed["ok"], true, "{listed:?}");
        assert_eq!(listed_ids(&mut state), vec![project_id.clone()]);
        assert_eq!(ensure(&mut state, &project_id), owner);
        state.persist();
    }
    std::fs::rename(&repo, &away).unwrap();
    {
        let mut state = booted(&state_root);
        std::fs::rename(&away, &repo).unwrap();
        assert_eq!(ensure(&mut state, &project_id), owner, "ensure looks again");
    }
    std::fs::rename(&repo, &away).unwrap();
    {
        let state = booted(&state_root).shared();
        std::fs::rename(&away, &repo).unwrap();
        AppState::sync_sources(
            &state,
            crate::app::SyncPass::Due,
            i64::try_from(crate::agent::now_ms()).unwrap(),
        );
        let mut state = state.lock().unwrap();
        assert_eq!(
            listed_ids(&mut state),
            vec![project_id.clone()],
            "the timer looks again"
        );
    }
    let mut state = booted(&state_root);
    assert_eq!(listed_ids(&mut state), vec![project_id.clone()]);
    assert_eq!(ensure(&mut state, &project_id), owner);
    assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
    assert_eq!(state.runs.len(), 1, "no second owner");
}
