//! The wire contract, as `fixtures/api/` states it (wire spec Part 2, step
//! 2.3): every fixture's `params` parses into the typed params of the method
//! it names, its `result` round-trips through the typed result unchanged, every
//! method `api::v1` serves has a fixture, every fixture names a method the
//! bridge serves, and `API_VERSION` is what `versions.json` calls current.
//!
//! The SPA reads the same files (`spa/test/apiContract.test.js`), so a shape
//! change is one edit both ends are held to.

use build_bridge::api::v1;
use build_bridge::api::API_VERSION;
use build_bridge::changes;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Methods answered outside `api::v1`, on purpose. Each may have a fixture
/// without a v1 registration, and none may ever gain one: the test below
/// fails the moment a family registers a name listed here.
///
/// Why each stays: `session.hello`, `term.attach`, `term.ack`, `rtc.*` and
/// `agent.attach` need the caller's own `SessionSender` (somewhere to push
/// to); `term.create`, `term.input`, `term.resize`, `agent.start`,
/// `agent.interrupt` and `stream.start` need the shared `Arc` (a producer,
/// pump or delivery runner to spawn); `bridge.stats` is answered from the
/// frame clock so it can never queue behind a wedged lock; `ping` is the
/// probe an old client sends before it knows what version it is talking to;
/// `term.list` and `term.close` are the terminal family's session-free reads,
/// kept beside the rest of `term.*` so the whole family moves together;
/// `stream.events` and `stream.state` are QA fixtures behind
/// `BRIDGE_QA_AGENT=1`, not part of the wire.
///
/// The `workspace.*` family is the one entry here with an expiry date: it was
/// added upstream after the facade; convert in a follow-up. Typing eight
/// verbs of a brand-new surface is its own change, not a merge resolution.
const LEGACY_METHODS: &[&str] = &[
    "agent.attach",
    "agent.interrupt",
    "agent.start",
    "bridge.stats",
    "ping",
    "rtc.close",
    "rtc.ice",
    "rtc.offer",
    "session.hello",
    "stream.events",
    "stream.start",
    "stream.state",
    "term.ack",
    "term.attach",
    "term.close",
    "term.create",
    "term.input",
    "term.list",
    "term.resize",
    // Added upstream after the facade; convert in a follow-up.
    "workspace.create",
    "workspace.ensure_conversation",
    "workspace.finish",
    "workspace.get",
    "workspace.git_init_options",
    "workspace.init_git",
    "workspace.list",
    "workspace.retry",
];

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
            registered.contains(method.as_str()) || LEGACY_METHODS.contains(&method.as_str()),
            "{method}: fixture names a method the bridge does not serve"
        );
    }
    for method in LEGACY_METHODS {
        assert!(
            !registered.contains(method),
            "{method}: served by v1, drop it from LEGACY_METHODS"
        );
    }
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
        handler
            .parse_params(&fixture["params"])
            .unwrap_or_else(|e| panic!("{method}: params do not parse: {e}"));
        let round_tripped = handler
            .round_trip_result(&fixture["result"])
            .unwrap_or_else(|e| panic!("{method}: result does not parse: {e}"));
        assert_eq!(
            serde_json::to_string(&round_tripped).unwrap(),
            serde_json::to_string(&fixture["result"]).unwrap(),
            "{method}: result changes shape through the typed result"
        );
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

fn typed<T: serde::de::DeserializeOwned>(value: &Value, what: &str) -> T {
    serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{what}: {e}"))
}

/// The `git` facts an item carries — the two `git_payload` writes.
fn check_git_facts(entity_id: &str, git: &Value) {
    for (key, value) in git.as_object().expect("git is an object") {
        assert!(
            matches!(key.as_str(), "status_key" | "head"),
            "{entity_id}: git.{key} is not a git fact"
        );
        assert!(value.is_string(), "{entity_id}: git.{key} is a string");
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
}

/// The `state` item: an object always, and the board's own row carries the
/// revision a client compares against.
fn check_state(entity_id: &str, state: &Value) {
    assert!(state.is_object(), "{entity_id}: state is an object");
    if entity_id == changes::BOARD_ITEM_ID {
        assert!(state["revision"].is_u64(), "board: state.revision");
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
            "board.changed" => assert_eq!(*event, changes::ChangeKey::Board.payload()),
            "entity.changed" => {
                let id = event["id"].as_str().expect("entity.changed names an id");
                assert_eq!(*event, changes::ChangeKey::Entity(id.to_string()).payload());
            }
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
