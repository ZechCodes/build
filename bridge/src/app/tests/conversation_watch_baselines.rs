use super::project_agent::{added_project, rooted, workspace};
use super::*;

fn owner(state: &mut AppState, repo: &Path) -> String {
    let project = added_project(state, repo);
    let workspace = workspace(state, &project, "watch baseline");
    state.handle(req("workspace.ensure_conversation", json!({"workspace_id": workspace})))
        ["result"]["run_id"].as_str().unwrap().to_string()
}

fn add(state: &mut AppState, owner: &str, watched: bool) -> String {
    let reply = state.handle(req("agent.add", json!({"entity_id": owner, "notify_user": watched})));
    assert_eq!(reply["ok"], true, "{reply:?}");
    reply["result"]["agent"]["id"].as_str().unwrap().to_string()
}

fn say(state: &mut AppState, owner: &str, id: &str) {
    state.edit_agent_conversation(owner, id, |thread, _| {
        thread.post_agent("please review", None, now_rfc3339());
        Ok(())
    }).unwrap();
}

fn watch(state: &mut AppState, owner: &str, id: &str, watching: bool) {
    let method = if watching { "conversation.watch" } else { "conversation.unwatch" };
    let reply = state.handle(req(method, json!({"entity_id": owner, "agent_id": id})));
    assert_eq!(reply["ok"], true, "{reply:?}");
}

fn count(state: &AppState, owner: &str, id: &str) -> u64 {
    let agent = state.entity_agents(owner).unwrap().by_id(id).unwrap();
    let thread = state.agent_conversation(owner, Some(id)).unwrap();
    state.agent_unread(owner, agent, thread).count
}

#[test]
fn conversation_watch_baselines_exclude_unwatched_and_pre_watch_news() {
    let (home, repo) = init_repo();
    let mut state = rooted(home.path());
    let owner = owner(&mut state, &repo);
    let watched = add(&mut state, &owner, true);
    let unwatched = add(&mut state, &owner, false);
    say(&mut state, &owner, &watched);
    say(&mut state, &owner, &unwatched);
    assert_eq!(count(&state, &owner, &unwatched), 0);
    assert_eq!(state.unread_for(&owner, None).count, 1);
    watch(&mut state, &owner, &unwatched, true);
    assert_eq!(count(&state, &owner, &unwatched), 0);
    say(&mut state, &owner, &unwatched);
    assert_eq!(count(&state, &owner, &unwatched), 1);
    watch(&mut state, &owner, &unwatched, true);
    assert_eq!(count(&state, &owner, &unwatched), 1, "repeat watch is idempotent");
    watch(&mut state, &owner, &unwatched, false);
    say(&mut state, &owner, &unwatched);
    watch(&mut state, &owner, &unwatched, true);
    assert_eq!(count(&state, &owner, &unwatched), 0);
    say(&mut state, &owner, &unwatched);
    assert_eq!(state.unread_for(&owner, None).count, 2);
}

#[test]
fn conversation_watch_baselines_survive_restart_and_count_stored_history() {
    let (repo_home, repo) = init_repo();
    let home = tempfile::tempdir().unwrap();
    let mut state = rooted(home.path()).with_task_store(home.path().join("tasks")).unwrap();
    let owner = owner(&mut state, &repo);
    let id = add(&mut state, &owner, false);
    for _ in 0..210 { say(&mut state, &owner, &id); }
    watch(&mut state, &owner, &id, true);
    for _ in 0..210 { say(&mut state, &owner, &id); }
    assert_eq!(count(&state, &owner, &id), 210);
    drop(state);
    let restored = rooted(home.path()).with_task_store(home.path().join("tasks")).unwrap();
    assert!(restored.agent_conversation(&owner, Some(&id)).unwrap().resident_from_sequence() > 0);
    assert_eq!(count(&restored, &owner, &id), 210);
    drop(repo_home);
}

#[test]
fn conversation_watch_baselines_restart_at_zero_when_history_is_reset() {
    let (home, repo) = init_repo();
    let mut state = rooted(home.path());
    let owner = owner(&mut state, &repo);
    let id = add(&mut state, &owner, false);
    say(&mut state, &owner, &id);
    watch(&mut state, &owner, &id, true);
    let thread = state.agent_conversation(&owner, Some(&id)).unwrap();
    let params = json!({"entity_id": owner, "agent_id": id, "conversation_id": id,
        "project_id": state.projects.project_id_of(&owner).unwrap(), "expected_thread_id": thread.id});
    assert_eq!(state.handle(req("conversation.reset", params))["ok"], true);
    say(&mut state, &owner, &id);
    assert_eq!(count(&state, &owner, &id), 1);
}

#[test]
fn conversation_watch_baselines_use_the_canonical_history_for_aliases() {
    let (home, repo) = init_repo();
    let mut state = rooted(home.path());
    let owner = owner(&mut state, &repo);
    let canonical = add(&mut state, &owner, true);
    let alias = add(&mut state, &owner, false);
    state.edit_agent_record("test alias", &owner, &alias, |agent| {
        agent.bind_conversation(&canonical);
    });
    say(&mut state, &owner, &canonical);
    watch(&mut state, &owner, &alias, true);
    assert_eq!(count(&state, &owner, &alias), 0);
    say(&mut state, &owner, &canonical);
    assert_eq!(count(&state, &owner, &alias), 1);
    let thread = state.agent_conversation(&owner, Some(&canonical)).unwrap();
    let params = json!({"entity_id": owner, "agent_id": canonical, "conversation_id": canonical,
        "project_id": state.projects.project_id_of(&owner).unwrap(), "expected_thread_id": thread.id});
    assert_eq!(state.handle(req("conversation.reset", params))["ok"], true);
    say(&mut state, &owner, &canonical);
    assert_eq!(count(&state, &owner, &alias), 1);
}
