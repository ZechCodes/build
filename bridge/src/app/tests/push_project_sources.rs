//! Source-qualified push facts (#529): a subscription naming one original
//! project source hears that folder's git, files and terminals, addressed by
//! project, source and captured path, and nothing of its siblings. A client
//! that has not asked for sources hears exactly what it heard before.

use super::push::{greeted_push_session, pushes_until, settled_pushes};
use super::*;

/// The project the push session's QA daemon opened, given two more original
/// sources: a Git clone with no remote and a plain folder.
struct SourcedProject {
    project_id: String,
    second: PathBuf,
    plain: PathBuf,
}

fn add_sources(state: &Arc<Mutex<AppState>>, dir: &Path, repo: &Path) -> SourcedProject {
    let second = dir.join("second");
    git_in(dir, &["clone", "-q", repo.to_str().unwrap(), "second"]);
    git_in(&second, &["remote", "remove", "origin"]);
    crate::git_fixture::configure_repo(&second);
    let plain = dir.join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.md"), "plain\n").unwrap();
    let (second, plain) = (
        std::fs::canonicalize(second).unwrap(),
        std::fs::canonicalize(plain).unwrap(),
    );
    let mut app = state.lock().unwrap();
    let project_id = app.project_at(0).id.clone();
    let mut sources = app.project_at(0).sources.clone();
    for (id, name, path, is_git) in [
        ("source-2", "Second", &second, true),
        ("source-3", "Plain", &plain, false),
    ] {
        sources.push(crate::app::projects::ProjectSource::added(
            id.into(),
            name.into(),
            name.to_lowercase(),
            path.clone(),
            is_git,
            "main".into(),
        ));
    }
    assert!(app.projects.set_sources(&project_id, sources));
    SourcedProject {
        project_id,
        second,
        plain,
    }
}

fn source_path(state: &Arc<Mutex<AppState>>, project_id: &str, source_id: &str) -> String {
    let app = state.lock().unwrap();
    let project = app.projects.get(project_id).unwrap();
    let source = project.sources.iter().find(|s| s.id == source_id).unwrap();
    source.path.display().to_string()
}

fn source_scope(state: &Arc<Mutex<AppState>>, project_id: &str, source_id: &str) -> Value {
    json!({
        "kind": "project_source",
        "project_id": project_id,
        "source_id": source_id,
        "source_path": source_path(state, project_id, source_id),
    })
}

fn subscribe(
    handler: &FrameHandler,
    sender: &SessionSender,
    id: &str,
    scope: Value,
    kinds: Value,
) -> Value {
    handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({ "subscription_id": id, "scope": scope, "kinds": kinds }),
        ),
    )
}

/// Every item one subscription was sent.
fn items_of(pushes: &[Value], subscription_id: &str) -> Vec<Value> {
    pushes
        .iter()
        .filter(|push| push["type"] == "changes" && push["subscription_id"] == subscription_id)
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .collect()
}

fn names_path(item: &Value, path: &str) -> bool {
    item["files"]["paths"]
        .as_array()
        .is_some_and(|paths| paths.iter().any(|named| named == path))
}

fn root_lists(item: &Value, name: &str) -> bool {
    item["files"]["root"]["entries"]
        .as_array()
        .is_some_and(|entries| entries.iter().any(|entry| entry["name"] == name))
}

/// A secondary Git source and a plain folder each push their own git, files
/// and terminal facts to the subscription that names them, carrying the
/// source's identity and the same keys its own verbs answer with. Neither
/// root's facts reach the other's subscription, and a subscription scoped
/// `all` — every client before `changes.projectSources` — hears none of it.
#[tokio::test]
async fn project_source_changes_keep_secondary_and_plain_roots_distinct() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let sourced = add_sources(&state, dir.path(), &repo);
    let project_id = sourced.project_id.clone();
    for (id, source_id, kinds) in [
        ("s-first", "source-1", json!(["terminals"])),
        ("s-second", "source-2", json!(["git", "files", "terminals"])),
        ("s-plain", "source-3", json!(["files", "terminals"])),
    ] {
        let scope = source_scope(&state, &project_id, source_id);
        let subscribed = subscribe(&handler, &sender, id, scope, kinds);
        assert_eq!(subscribed["ok"], true, "{id}: {subscribed:?}");
    }
    let legacy = subscribe(
        &handler,
        &sender,
        "s-all",
        json!({ "kind": "all" }),
        json!(["state", "thread", "git", "files", "terminals"]),
    );
    assert_eq!(legacy["ok"], true, "{legacy:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(sourced.second.join("second.txt"), "second\n").unwrap();
    std::fs::write(sourced.plain.join("plain.txt"), "plain\n").unwrap();
    let pushes = pushes_until(&mut rx, &key, |pushes| {
        let second = items_of(pushes, "s-second");
        items_of(pushes, "s-plain")
            .iter()
            .any(|item| names_path(item, "plain.txt"))
            && second.iter().any(|item| names_path(item, "second.txt"))
            && second
                .iter()
                .any(|item| item["git"]["status_key"].is_string())
    })
    .await;

    let second_scope = source_scope(&state, &project_id, "source-2");
    let identity = |scope: &Value| {
        json!({
            "project_id": scope["project_id"],
            "source_id": scope["source_id"],
            "source_path": scope["source_path"],
        })
    };
    let second_items = items_of(&pushes, "s-second");
    for item in &second_items {
        assert_eq!(item["project_source"], identity(&second_scope), "{item:?}");
        assert!(!names_path(item, "plain.txt"), "{item:?}");
    }
    let git_item = second_items
        .iter()
        .rfind(|item| item["git"]["status_key"].is_string())
        .unwrap();
    let status = call(
        &handler,
        "git.status",
        json!({
            "project_id": project_id, "source_id": "source-2",
            "source_path": second_scope["source_path"],
        }),
    );
    assert_eq!(
        git_item["git"]["status_key"], status["result"]["status_key"],
        "a pushed key is the key the source's own git.status answers"
    );
    assert_eq!(git_item["git"]["status"]["path"], status["result"]["path"]);
    let file_item = second_items
        .iter()
        .find(|item| names_path(item, "second.txt"))
        .unwrap();
    assert!(root_lists(file_item, "second.txt"), "{file_item:?}");
    assert!(!root_lists(file_item, "plain.txt"), "{file_item:?}");

    let plain_scope = source_scope(&state, &project_id, "source-3");
    let plain_items = items_of(&pushes, "s-plain");
    for item in &plain_items {
        assert_eq!(item["project_source"], identity(&plain_scope), "{item:?}");
        assert!(
            item.get("git").is_none(),
            "a plain folder has no git: {item:?}"
        );
        assert!(!names_path(item, "second.txt"), "{item:?}");
    }
    let plain_files = plain_items
        .iter()
        .find(|item| names_path(item, "plain.txt"))
        .unwrap();
    assert!(root_lists(plain_files, "plain.txt"), "{plain_files:?}");

    // A shell in the primary checkout is a tab of source-1 alone.
    let created = call(&handler, "term.create", json!({ "project_id": project_id }));
    assert_eq!(created["ok"], true, "{created:?}");
    let tabs = pushes_until(&mut rx, &key, |pushes| {
        items_of(pushes, "s-first")
            .iter()
            .any(|item| item["terminals"]["tabs"].as_array().map(Vec::len) == Some(1))
    })
    .await;
    let first_scope = source_scope(&state, &project_id, "source-1");
    let first_tabs = items_of(&tabs, "s-first");
    assert!(first_tabs
        .iter()
        .all(|item| item["project_source"] == identity(&first_scope)));
    for other in ["s-second", "s-plain"] {
        assert!(
            items_of(&tabs, other)
                .iter()
                .all(|item| item.get("terminals").is_none()),
            "{other} heard the primary's tab: {tabs:?}"
        );
    }

    let quiet = settled_pushes(&mut rx, &key).await;
    let everything = [pushes, tabs, quiet].concat();
    let source_entity = |item: &Value| {
        item["entity_id"]
            .as_str()
            .is_some_and(crate::changes::is_project_source_entity)
    };
    assert!(first_tabs.iter().all(source_entity), "{first_tabs:?}");
    assert!(
        items_of(&everything, "s-all")
            .iter()
            .all(|item| item.get("project_source").is_none() && !source_entity(item)),
        "an `all` subscription never hears a source item: {everything:?}"
    );
    assert!(
        items_of(&everything, "s-all")
            .iter()
            .any(|item| item["entity_id"] == project_id && item.get("terminals").is_some()),
        "the legacy primary entity still hears its tab: {everything:?}"
    );
    drop(dir);
}

/// A source subscription names a registered source where the client opened
/// it, and asks only for what that folder has. Once the source moves the old
/// subscription goes quiet, and one naming the new folder hears it.
#[tokio::test]
async fn project_source_subscriptions_refuse_stale_or_unknown_sources() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let sourced = add_sources(&state, dir.path(), &repo);
    let project_id = sourced.project_id.clone();
    let second = source_scope(&state, &project_id, "source-2");
    let plain = source_scope(&state, &project_id, "source-3");
    let mut stale = second.clone();
    stale["source_path"] = json!(dir.path().join("elsewhere"));
    let mut unknown = second.clone();
    unknown["source_id"] = json!("source-9");
    let mut unsaid = second.clone();
    unsaid.as_object_mut().unwrap().remove("source_path");
    for (scope, kinds, code) in [
        (stale, json!(["files"]), "conflict"),
        (unknown, json!(["files"]), "not_found"),
        (unsaid, json!(["files"]), "invalid_params"),
        (plain, json!(["git"]), "invalid_params"),
        (second.clone(), json!(["state"]), "invalid_params"),
    ] {
        let refused = subscribe(&handler, &sender, "s-refused", scope.clone(), kinds);
        assert_eq!(refused["error_code"], code, "{scope}: {refused:?}");
    }

    let subscribed = subscribe(&handler, &sender, "s-old", second.clone(), json!(["files"]));
    assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    let moved = dir.path().join("moved");
    git_in(
        dir.path(),
        &["clone", "-q", sourced.second.to_str().unwrap(), "moved"],
    );
    let update = call(
        &handler,
        "project.update_source",
        json!({ "project_id": project_id, "source_id": "source-2", "path": moved }),
    );
    assert_eq!(update["ok"], true, "{update:?}");
    let moved_scope = source_scope(&state, &project_id, "source-2");
    assert_ne!(moved_scope["source_path"], second["source_path"]);
    let renewed = subscribe(&handler, &sender, "s-new", moved_scope, json!(["files"]));
    assert_eq!(renewed["result"]["watch"], "live", "{renewed:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(sourced.second.join("old.txt"), "old\n").unwrap();
    std::fs::write(moved.join("new.txt"), "new\n").unwrap();
    let pushes = pushes_until(&mut rx, &key, |pushes| {
        items_of(pushes, "s-new")
            .iter()
            .any(|item| names_path(item, "new.txt"))
    })
    .await;
    let quiet = settled_pushes(&mut rx, &key).await;
    let everything = [pushes, quiet].concat();
    assert!(
        items_of(&everything, "s-old").is_empty(),
        "a moved source's old subscription hears nothing: {everything:?}"
    );
    assert!(items_of(&everything, "s-new")
        .iter()
        .all(|item| !names_path(item, "old.txt")));
}

/// Removing a source in settings ends its membership at once: its watcher
/// stops without waiting for the next sweep, so the removed folder's writes
/// reach no subscription.
#[tokio::test]
async fn a_removed_source_stops_being_watched() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, _rx, _key) = greeted_push_session(&repo, dir.path());
    let sourced = add_sources(&state, dir.path(), &repo);
    let project_id = sourced.project_id.clone();
    let plain = source_scope(&state, &project_id, "source-3");
    let entity_id = crate::changes::project_source_entity_id(
        &project_id,
        "source-3",
        plain["source_path"].as_str().unwrap(),
    );
    let subscribed = subscribe(&handler, &sender, "s-plain", plain, json!(["files"]));
    assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    assert!(state.lock().unwrap().watchers().is_watching(&entity_id));

    let removed = call(
        &handler,
        "project.remove_source",
        json!({ "project_id": project_id, "source_id": "source-3" }),
    );
    assert_eq!(removed["ok"], true, "{removed:?}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while state.lock().unwrap().watchers().is_watching(&entity_id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the removed source is still watched"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
