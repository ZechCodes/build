//! The wire contract, as `fixtures/api/` states it (wire spec Part 2, step
//! 2.3): every fixture's `params` parses into the typed params of the method
//! it names, its `result` round-trips through the typed result unchanged, every
//! method `api::v1` serves has a fixture, every fixture names a method the
//! bridge serves, and `API_VERSION` is what `versions.json` calls current.
//!
//! The SPA reads the same files (`spa/test/apiContract.test.js`), so a shape
//! change is one edit both ends are held to.

use build_bridge::api::v1;
use build_bridge::api::{
    capabilities, API_VERSION, FEATURE_CAPABILITIES, LEGACY_METHODS, QA_METHODS,
};
use build_bridge::changes;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/api")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every `fixtures/api/v1/<method>.json`, keyed by the method the file names.
fn method_fixtures() -> Vec<(String, Value)> {
    let dir = fixtures_root().join("v1");
    let mut fixtures: Vec<(String, Value)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.expect("readable entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| path.file_stem().is_some_and(|stem| stem != "events"))
        .map(|path| {
            let fixture = read_json(&path);
            let method = fixture["method"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: no method", path.display()))
                .to_string();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert_eq!(
                stem,
                method,
                "{}: file is named after its method",
                path.display()
            );
            (method, fixture)
        })
        .collect();
    fixtures.sort_by(|a, b| a.0.cmp(&b.0));
    fixtures
}

#[test]
fn api_version_is_what_versions_json_calls_current() {
    let versions = read_json(&fixtures_root().join("versions.json"));
    assert_eq!(versions["current"], API_VERSION);
    let major: u64 = API_VERSION.split('.').next().unwrap().parse().unwrap();
    let supported: Vec<u64> = versions["supported_majors"]
        .as_array()
        .expect("supported_majors is a list")
        .iter()
        .map(|v| v.as_u64().expect("a major"))
        .collect();
    assert!(supported.contains(&major), "{supported:?} lacks {major}");
    assert!(versions["deprecated"].is_array());
}

#[test]
fn every_fixture_has_the_spec_shape() {
    for (method, fixture) in method_fixtures() {
        assert!(fixture["since"].is_string(), "{method}: since");
        assert!(fixture["params"].is_object(), "{method}: params");
        assert!(fixture.get("result").is_some(), "{method}: result");
        let errors = fixture["errors"]
            .as_array()
            .unwrap_or_else(|| panic!("{method}: errors"));
        for code in errors {
            let code = code
                .as_str()
                .unwrap_or_else(|| panic!("{method}: error code"));
            assert!(
                build_bridge::api::ApiError::CODES.contains(&code),
                "{method}: {code} is not an ApiError code"
            );
        }
    }
}

#[test]
fn every_v1_method_has_a_fixture_and_every_fixture_names_a_served_method() {
    let registered: BTreeSet<&str> = v1::methods().iter().map(|(name, _)| *name).collect();
    let fixtures: BTreeSet<String> = method_fixtures().into_iter().map(|(m, _)| m).collect();
    for method in &registered {
        assert!(
            fixtures.contains(*method),
            "{method}: no fixture in fixtures/api/v1/"
        );
    }
    for method in &fixtures {
        assert!(
            registered.contains(method.as_str())
                || LEGACY_METHODS.contains(&method.as_str())
                || QA_METHODS.contains(&method.as_str()),
            "{method}: fixture names a method the bridge does not serve"
        );
    }
    for method in LEGACY_METHODS.iter().chain(QA_METHODS) {
        assert!(
            !registered.contains(method),
            "{method}: served by v1, drop it from LEGACY_METHODS"
        );
    }
}

#[test]
fn every_fixture_verb_has_an_advertised_capability() {
    let advertised: BTreeSet<&str> = capabilities(true).into_iter().collect();
    for (method, _) in method_fixtures() {
        assert!(
            advertised.contains(method.as_str()),
            "{method}: fixture verb absent from session.hello capabilities"
        );
    }
    let normal: BTreeSet<&str> = capabilities(false).into_iter().collect();
    let greeting_fixture = read_json(&fixtures_root().join("v1/session.hello.json"));
    assert_eq!(
        greeting_fixture["result"]["capabilities"],
        serde_json::json!(capabilities(false)),
        "session.hello fixture must list every production capability"
    );
    for method in QA_METHODS {
        assert!(
            !normal.contains(method),
            "{method}: QA verb advertised in production"
        );
    }
    for feature in FEATURE_CAPABILITIES {
        assert!(
            normal.contains(feature),
            "{feature}: feature absent from greeting"
        );
    }
}

#[test]
fn media_page_features_are_announced_together() {
    let advertised: BTreeSet<&str> = capabilities(false).into_iter().collect();
    assert_eq!(API_VERSION, "3.11.0");
    assert!(advertised.contains("thread.attachmentChunks"));
    assert!(advertised.contains("fs.mediaRawPages"));
    let greeting = read_json(&fixtures_root().join("v1/session.hello.json"));
    assert_eq!(greeting["result"]["api_version"], API_VERSION);
    assert!(greeting["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry == "thread.attachmentChunks"));
}

#[test]
fn review_snapshots_and_selected_git_actions_have_separate_capabilities() {
    let advertised = capabilities(false);
    for method in ["snapshot", "get", "diff", "complete"] {
        let method = format!("tasks.review.{method}");
        assert!(advertised.contains(&method.as_str()));
        let fixture = read_json(&fixtures_root().join("v1").join(format!("{method}.json")));
        assert_eq!(fixture["since"], "3.6.0");
    }
    assert!(advertised.contains(&"tasks.review.act"));
    let action = read_json(&fixtures_root().join("v1/tasks.review.act.json"));
    assert_eq!(action["since"], "3.8.0");
    assert_eq!(action["params"]["sources"][0]["merge"]["branch"], "main");
    let read = read_json(&fixtures_root().join("v1/tasks.review.diff.json"));
    assert_eq!(read["result"]["files_truncated"], false);
    let examples = read["examples"].as_array().unwrap();
    assert!(examples
        .iter()
        .any(|example| example["params"]["mode"] == "tree"));
    let blob = examples
        .iter()
        .find(|example| example["params"]["mode"] == "blob")
        .unwrap();
    assert_eq!(blob["result"]["editable"], false);
    assert_eq!(
        blob["result"]["range"]["version"].as_str().unwrap().len(),
        40
    );
}

#[test]
fn conditional_task_body_writes_have_a_named_capability_and_distinct_refusal() {
    assert!(capabilities(false).contains(&"tasks.bodyPrecondition"));
    let update = read_json(&fixtures_root().join("v1/tasks.update.json"));
    assert_eq!(update["body_precondition"]["since"], "3.10.0");
    assert_eq!(
        update["body_precondition"]["capability"],
        "tasks.bodyPrecondition"
    );
    assert_eq!(
        update["body_precondition"]["params"],
        serde_json::json!(["expected_body_hash"])
    );
    assert!(update["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|code| code == "stale_body"));
    let handler = v1::methods()
        .iter()
        .find(|(method, _)| *method == "tasks.update")
        .unwrap();
    assert!(handler.1.parse_params(&update["params"]).is_ok());
    let mut wrong = update["params"].clone();
    wrong["expected_body_hash"] = serde_json::json!(7);
    assert!(handler.1.parse_params(&wrong).is_err());
}

#[test]
fn review_action_params_are_typed_and_accept_no_caller_paths() {
    let handler = v1::methods()
        .iter()
        .find(|(name, _)| *name == "tasks.review.act")
        .unwrap();
    let valid = serde_json::json!({
        "task_id":"task-1", "expected_version":1, "snapshot_id":"snapshot-1",
        "sources":[{"directory_id":"dir-api", "merge":{"branch":"main"},
                    "push":{"remote":"origin", "branch":"release", "merge_action_id":"action-1"}}]
    });
    assert!(handler.1.parse_params(&valid).is_ok());
    for extra in ["source_path", "repo_path", "workspace_id", "actor"] {
        let mut injected = valid.clone();
        injected[extra] = serde_json::json!("/tmp/forged");
        assert!(
            handler.1.parse_params(&injected).is_err(),
            "accepted {extra}"
        );
    }
    let mut nested = valid.clone();
    nested["sources"][0]["merge"]["path"] = serde_json::json!("/tmp/forged");
    assert!(handler.1.parse_params(&nested).is_err());
    let mut wrong = valid;
    wrong["expected_version"] = serde_json::json!(-1);
    assert!(handler.1.parse_params(&wrong).is_err());
}

#[test]
fn every_fixture_parses_and_its_result_round_trips_through_the_typed_result() {
    let by_name: std::collections::BTreeMap<&str, &v1::Handler> = v1::methods()
        .iter()
        .map(|(name, handler)| (*name, handler))
        .collect();
    for (method, fixture) in method_fixtures() {
        let Some(handler) = by_name.get(method.as_str()) else {
            continue; // legacy: tolerated until its family converts
        };
        for (at, example) in examples_of(&fixture) {
            let method = format!("{method}{at}");
            handler
                .parse_params(&example["params"])
                .unwrap_or_else(|e| panic!("{method}: params do not parse: {e}"));
            let round_tripped = handler
                .round_trip_result(&example["result"])
                .unwrap_or_else(|e| panic!("{method}: result does not parse: {e}"));
            assert_eq!(
                serde_json::to_string(&round_tripped).unwrap(),
                serde_json::to_string(&example["result"]).unwrap(),
                "{method}: result changes shape through the typed result"
            );
        }
    }
}

/// A fixture's own params and result, then each of its further `examples`
/// (a paged `tasks.list`, #85), named by where it sits in the file.
fn examples_of(fixture: &Value) -> Vec<(String, &Value)> {
    let further = fixture["examples"].as_array().into_iter().flatten();
    std::iter::once((String::new(), fixture))
        .chain(further.enumerate().map(|(index, example)| {
            assert!(example["params"].is_object(), "examples[{index}]: params");
            assert!(example.get("result").is_some(), "examples[{index}]: result");
            (format!(" examples[{index}]"), example)
        }))
        .collect()
}

/// A param absent from a verb's declared shape cannot be swallowed: each
/// fixture's params, plus one field no verb declares, are refused naming that
/// field. Every fixture's own params parsing (above) is the other half — no
/// fixture sends a field its verb does not declare.
#[test]
fn every_v1_verb_refuses_a_param_its_shape_does_not_declare() {
    let fixtures: std::collections::BTreeMap<String, Value> =
        method_fixtures().into_iter().collect();
    for (method, handler) in v1::methods() {
        let mut params = fixtures[*method]["params"].clone();
        params["undeclared_by_any_verb"] = Value::Bool(false);
        let refused = handler
            .parse_params(&params)
            .expect_err(&format!("{method}: an undeclared param was swallowed"));
        assert!(
            refused.contains("unknown param: undeclared_by_any_verb"),
            "{method}: the refusal names the param it did not know: {refused}"
        );
    }
}

// --------------------------------------------------- when a verb arrived ---

/// `1.24.0` as `(1, 24, 0)`.
fn version_parts(version: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = version
        .split('.')
        .map(|part| part.parse().unwrap_or_else(|e| panic!("{version}: {e}")))
        .collect();
    assert_eq!(parts.len(), 3, "{version} is not major.minor.patch");
    (parts[0], parts[1], parts[2])
}

fn string_set(manifest: &Value, key: &str) -> BTreeSet<String> {
    manifest[key]
        .as_array()
        .unwrap_or_else(|| panic!("manifest: {key} is a list"))
        .iter()
        .map(|name| name.as_str().expect("a name").to_string())
        .collect()
}

/// The one `fixtures/api/verbs-<minor>.json` there is, written by
/// `scripts/api-verbs-manifest.mjs`: the release before this one.
fn previous_release_manifest() -> Value {
    let names: Vec<String> = std::fs::read_dir(fixtures_root())
        .expect("fixtures/api")
        .map(|entry| entry.expect("readable entry").file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.starts_with("verbs-") && name.ends_with(".json"))
        .collect();
    assert_eq!(
        names.len(),
        1,
        "one manifest, the previous release's: {names:?}"
    );
    read_json(&fixtures_root().join(&names[0]))
}

/// Whether `previous` is the release right before `current`: the minor before
/// it, or for a new major's first minor, any release of the major before — or,
/// for a later patch of that first minor, its own first release.
fn directly_precedes(previous: (u64, u64, u64), current: (u64, u64, u64)) -> bool {
    match current {
        (major, 0, patch) => {
            previous.0 + 1 == major || (previous.0, previous.1) == (major, 0) && previous.2 < patch
        }
        (major, minor, _) => (previous.0, previous.1 + 1) == (major, minor),
    }
}

/// Whether `current` may serve less than `previous` did: only the first
/// release of a new major, which is what makes it one. A later patch of it
/// compared against the major before would slip removals through unchecked.
fn may_remove(previous: (u64, u64, u64), current: (u64, u64, u64)) -> bool {
    previous.0 != current.0 && (current.1, current.2) == (0, 0)
}

#[test]
fn a_new_major_follows_any_release_of_the_one_before() {
    assert!(directly_precedes((1, 30, 0), (2, 0, 0)));
    assert!(directly_precedes((1, 29, 2), (1, 30, 0)));
    assert!(!directly_precedes((1, 28, 0), (1, 30, 0)));
    assert!(!directly_precedes((1, 30, 0), (3, 0, 0)));
    assert!(!directly_precedes((1, 30, 0), (2, 1, 0)));
    assert!(directly_precedes((2, 0, 0), (2, 0, 1)));
    assert!(!directly_precedes((2, 0, 1), (2, 0, 1)));
}

#[test]
fn only_a_new_majors_first_release_may_remove() {
    assert!(may_remove((1, 30, 0), (2, 0, 0)));
    assert!(!may_remove((1, 30, 0), (2, 0, 1)), "2.0.1 is held to 2.0.0");
    assert!(!may_remove((2, 0, 0), (2, 0, 1)));
    assert!(!may_remove((1, 29, 0), (1, 30, 0)));
}

/// The registry against the previous release's manifest: a minor only adds,
/// so everything the previous minor served and announced is still here (a new
/// major may remove, which is what makes it one); and every verb or capability
/// it did not have declares, in its fixture, the release that introduced it.
/// The SPA's `apiContract.test.js` holds the fixtures to the same manifest.
#[test]
fn the_registry_adds_to_the_previous_minor_and_dates_what_it_added() {
    let (major, minor, patch) = version_parts(API_VERSION);
    let release = format!("{major}.{minor}.0");
    let manifest = previous_release_manifest();
    let previous = manifest["api_version"]
        .as_str()
        .expect("api_version")
        .to_string();
    let previous_parts = version_parts(&previous);
    assert!(
        directly_precedes(previous_parts, (major, minor, patch)),
        "{previous} is not the release before {API_VERSION}"
    );
    let removals_allowed = may_remove(previous_parts, (major, minor, patch));

    let served: BTreeSet<&str> = v1::methods()
        .iter()
        .map(|(name, _)| *name)
        .chain(LEGACY_METHODS.iter().copied())
        .chain(QA_METHODS.iter().copied())
        .collect();
    let announced: BTreeSet<&str> = capabilities(false).into_iter().collect();
    let verbs_before = string_set(&manifest, "verbs");
    let capabilities_before = string_set(&manifest, "capabilities");
    if !removals_allowed {
        for verb in &verbs_before {
            assert!(
                served.contains(verb.as_str()),
                "{verb}: served at {previous}, gone at {API_VERSION} (past a new major's \
                 first release, write the manifest from that release)"
            );
        }
        for capability in &capabilities_before {
            assert!(
                announced.contains(capability.as_str()),
                "{capability}: announced at {previous}, gone at {API_VERSION}"
            );
        }
    }

    let fixtures: std::collections::BTreeMap<String, Value> =
        method_fixtures().into_iter().collect();
    let since = |method: &str| {
        fixtures[method]["since"]
            .as_str()
            .expect("since")
            .to_string()
    };
    for (method, _) in v1::methods() {
        if !verbs_before.contains(*method) {
            assert_eq!(since(method), release, "{method}: new since {previous}");
        }
    }
    for capability in &announced {
        if !fixtures.contains_key(*capability) {
            continue; // a feature or a legacy verb: the SPA test covers these
        }
        let stated = since(capability);
        if capabilities_before.contains(*capability) {
            assert!(
                version_parts(&stated) < version_parts(&release),
                "{capability}: announced at {previous} but says since {stated}"
            );
        } else {
            assert_eq!(stated, release, "{capability}: announced since {previous}");
        }
    }
}

// ------------------------------------------------------------- the pushes ---

/// Every push the bridge sends on a session, as `fixtures/api/v1/events.json`
/// states them. The SPA reads the same list through `v1.parseEvent`.
fn event_examples() -> Vec<Value> {
    let path = fixtures_root().join("v1/events.json");
    let fixture = read_json(&path);
    assert!(fixture["since"].is_string(), "events.json: since");
    fixture["events"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: events is a list", path.display()))
        .clone()
}

/// The receipt the intake pushes the moment it admits a request, as
/// `events.json` states it: the request's id, `accepted: true`, and no
/// verdict. The absence of `ok` is load-bearing — it is what tells a client
/// that this settles nothing and its answer is still coming.
#[test]
fn a_receipt_names_a_request_and_settles_nothing() {
    let fixture = read_json(&fixtures_root().join("v1/events.json"));
    let receipt = &fixture["receipt"];
    assert!(
        receipt["id"].is_u64() || receipt["id"].is_string(),
        "{receipt:?}"
    );
    assert_eq!(receipt["accepted"], Value::Bool(true), "{receipt:?}");
    assert!(
        receipt.get("ok").is_none(),
        "a receipt carries no verdict: {receipt:?}"
    );
    assert!(receipt.get("result").is_none(), "{receipt:?}");
    assert_eq!(
        receipt.as_object().expect("an object").len(),
        2,
        "a receipt is the id and the word, and nothing else: {receipt:?}"
    );
}

fn typed<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{what}: {e}"))
}

/// The `git` facts an item carries: the two keys a client compares against,
/// the surfaces it writes into its cache, and the size of the diff whether or
/// not the diff itself came with it.
fn check_git_facts(entity_id: &str, git: &Value) {
    for (key, value) in git.as_object().expect("git is an object") {
        let stated = match key.as_str() {
            "status_key" | "head" => value.is_string(),
            "status" | "log" | "unpushed" => value.is_object(),
            "diff" => value.is_object() || value.is_null(),
            "diff_bytes" => value.is_u64(),
            _ => panic!("{entity_id}: git.{key} is not a git fact"),
        };
        assert!(stated, "{entity_id}: git.{key} is the wrong shape");
    }
    if git.get("diff").is_some() {
        assert!(
            git["diff_bytes"].is_u64(),
            "{entity_id}: a diff says how big it is"
        );
    }
}

/// The `files` item: paths under the cap, and the flag that says the list
/// stopped naming them.
fn check_files(entity_id: &str, files: &Value) {
    let paths = files["paths"].as_array().expect("files.paths is a list");
    assert!(
        paths.iter().all(Value::is_string),
        "{entity_id}: files.paths are strings"
    );
    assert!(
        paths.len() <= changes::FILES_PER_FLUSH,
        "{entity_id}: files.paths is capped"
    );
    assert!(
        files["truncated"].is_boolean(),
        "{entity_id}: files.truncated is a bool"
    );
    if let Some(root) = files.get("root") {
        assert!(root["path"].is_string(), "{entity_id}: files.root.path");
        assert!(
            root["entries"].is_array(),
            "{entity_id}: files.root.entries"
        );
    }
}

/// The `state` item: an object always, and the board's own row carries the
/// revision a client compares against.
fn check_state(entity_id: &str, state: &Value) {
    assert!(state.is_object(), "{entity_id}: state is an object");
    if entity_id != changes::BOARD_ITEM_ID {
        return;
    }
    assert!(state["revision"].is_u64(), "board: state.revision");
    for (key, value) in state.as_object().expect("state is an object") {
        let stated = match key.as_str() {
            "revision" => value.is_u64(),
            // Which entities left the board, and the lists a client caches
            // whole — each present only when the change that noted it moved
            // one. `usage_limits` since 1.11.0: the harnesses out of usage on
            // this device.
            "removed" | "projects" | "workspaces" | "usage_limits" => value.is_array(),
            _ => panic!("board: state.{key} is not part of the board item"),
        };
        assert!(stated, "board: state.{key} is the wrong shape");
    }
}

/// Every key beside `entity_id` is named after a [`changes::Kind`].
fn check_item_keys(entity_id: &str, item: &serde_json::Map<String, Value>) {
    let kinds: Vec<&str> = changes::KindSet::all()
        .iter()
        .map(changes::Kind::as_str)
        .collect();
    for key in item.keys() {
        assert!(
            key == "entity_id" || kinds.contains(&key.as_str()),
            "{entity_id}: {key} is not a kind"
        );
    }
}

/// One item of a `changes` frame: `entity_id`, and beside it only the kinds
/// that moved, each carrying what that kind serialises as.
fn check_changes_item(item: &Value) {
    let object = item.as_object().expect("an item is an object");
    let entity_id = object["entity_id"].as_str().expect("entity_id is a string");
    check_item_keys(entity_id, object);
    if let Some(state) = object.get("state") {
        check_state(entity_id, state);
    }
    if let Some(thread) = object.get("thread") {
        let _: Vec<changes::ThreadTip> = typed(thread, "thread");
    }
    if let Some(git) = object.get("git") {
        check_git_facts(entity_id, git);
    }
    if let Some(files) = object.get("files") {
        check_files(entity_id, files);
    }
    if let Some(tasks) = object.get("tasks") {
        check_tasks(entity_id, tasks);
    }
}

/// The `tasks` item: the ids that moved, under the cap, and the flag that
/// says the list stopped naming them.
///
/// Its `entity_id` is a PROJECT — the one item whose entity is not a work item,
/// because a tracker belongs to a project and not to anything inside it.
fn check_tasks(entity_id: &str, tasks: &Value) {
    assert!(
        entity_id.starts_with("proj-"),
        "{entity_id}: a tasks item is about a project"
    );
    let ids = tasks["task_ids"]
        .as_array()
        .expect("tasks.task_ids is a list");
    assert!(
        ids.iter().all(Value::is_string),
        "{entity_id}: tasks.task_ids are strings"
    );
    assert!(
        ids.len() <= changes::TASKS_PER_FLUSH,
        "{entity_id}: tasks.task_ids is capped"
    );
    assert!(
        tasks["truncated"].is_boolean(),
        "{entity_id}: tasks.truncated is a bool"
    );
}

/// The terminal and signalling frames, which are `json!` literals rather than
/// typed structs (`screen.rs`, `rtc.rs`): their required keys are the contract.
fn check_untyped_push(event: &Value, keys: &[&str]) {
    for key in keys {
        assert!(
            event.get(*key).is_some_and(|value| !value.is_null()),
            "{}: {key}",
            event["type"]
        );
    }
}

#[test]
fn every_event_example_is_what_the_bridge_serialises() {
    let examples = event_examples();
    assert!(!examples.is_empty(), "events.json states no examples");
    for event in &examples {
        match event["type"].as_str().expect("an event names its type") {
            changes::CHANGES_EVENT => {
                assert!(event["subscription_id"].is_string(), "changes: id");
                let items = event["items"].as_array().expect("changes: items");
                assert!(!items.is_empty(), "changes: items is never empty");
                items.iter().for_each(check_changes_item);
            }
            "term.output" => check_untyped_push(event, &["term_id", "data", "cursor"]),
            "term.reset" => check_untyped_push(event, &["term_id", "data", "cursor"]),
            "term.closed" => check_untyped_push(event, &["term_id", "reason"]),
            "rtc.ice" => check_untyped_push(event, &["candidate"]),
            "rtc.diagnostics" => check_untyped_push(event, &["event", "reason", "candidates"]),
            "bridge.update_status" => {
                let mut status = event.clone();
                status.as_object_mut().unwrap().remove("type");
                let parsed: build_bridge::update::UpdateStatus = typed(&status, "bridge update");
                assert_eq!(
                    serde_json::to_value(parsed).unwrap(),
                    status,
                    "bridge update event is the status wire shape plus type"
                );
            }
            changes::MODELS_CHANGED_EVENT => {
                assert_eq!(*event, changes::models_changed_payload());
            }
            other => panic!("{other}: the bridge sends no such push"),
        }
    }
}

/// Every push the bridge sends has an example. The announced change events are
/// the list the greeting carries; the rest are the session's own frames.
#[test]
fn every_push_the_bridge_sends_has_an_example() {
    let seen: BTreeSet<String> = event_examples()
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect();
    let sent = changes::ANNOUNCED_EVENTS.iter().copied().chain([
        "term.output",
        "term.reset",
        "term.closed",
        "rtc.ice",
    ]);
    for kind in sent {
        assert!(seen.contains(kind), "{kind}: no example in events.json");
    }
}

#[test]
fn candidate_diagnostics_are_announced_without_exposing_peer_addresses() {
    assert!(capabilities(false).contains(&"rtc.candidateDiagnostics"));
    assert!(changes::ANNOUNCED_EVENTS.contains(&"rtc.diagnostics"));
    let diagnostics = event_examples()
        .into_iter()
        .find(|event| event["type"] == "rtc.diagnostics")
        .expect("candidate diagnostics fixture");
    assert_eq!(diagnostics["reason"], "mdns-unresolved");
    assert_eq!(diagnostics["candidates"]["host_mdns"], 1);
    assert_eq!(diagnostics["candidates"]["mdns_unresolved"], 1);
    assert!(diagnostics.get("address").is_none());
    assert!(diagnostics.get("candidate").is_none());
}

#[test]
fn candidate_diagnostics_use_the_existing_wire_fixture_envelopes() {
    for name in ["session.hello", "events"] {
        let fixture = read_json(&fixtures_root().join("v1").join(format!("{name}.json")));
        assert!(
            fixture.get("candidate_diagnostics").is_none(),
            "feature version documentation belongs with the capability, not as a wire fixture field"
        );
    }
}

#[test]
fn project_file_sources_are_announced_with_scoped_contract_examples() {
    assert!(capabilities(false).contains(&"fs.projectSources"));
    for method in ["fs.tree", "fs.read", "fs.write"] {
        let fixture = read_json(&fixtures_root().join("v1").join(format!("{method}.json")));
        assert_eq!(fixture["project_sources"]["since"], "3.9.0");
        assert_eq!(
            fixture["project_sources"]["capability"],
            "fs.projectSources"
        );
        let example = fixture["examples"]
            .as_array()
            .unwrap()
            .iter()
            .find(|example| example["params"]["project_id"] == "proj-1")
            .unwrap();
        assert_eq!(example["params"]["source_id"], "source-2");
        let handler = &v1::methods()
            .iter()
            .find(|(name, _)| *name == method)
            .unwrap()
            .1;
        handler.parse_params(&example["params"]).unwrap();
        assert_eq!(
            handler.round_trip_result(&example["result"]).unwrap(),
            example["result"]
        );
    }
}
