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
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Methods still answered by the legacy `route` table rather than `api::v1`.
/// Each may have a fixture without a v1 registration; the list shrinks as the
/// families convert and the integration stage empties it.
const LEGACY_METHODS: &[&str] = &[
    "agent.add",
    "agent.attach",
    "agent.choose",
    "agent.list",
    "agent.remove",
    "agent.start",
    "archive.list",
    "archived.list",
    "board.list",
    "branch.dispatch",
    "branch.finish",
    "branch.get",
    "bridge.stats",
    "capture.answer",
    "capture.cancel",
    "capture.create",
    "capture.get",
    "capture.list",
    "capture.reroute",
    "entity.dismiss",
    "entity.mute",
    "entity.seen",
    "issue.approve",
    "issue.archive",
    "issue.comment_add",
    "issue.comment_delete",
    "issue.create",
    "issue.delete",
    "issue.doc",
    "issue.get",
    "issue.git_action",
    "issue.implement_all",
    "issue.implement_stage",
    "issue.list",
    "issue.request_changes",
    "issue.send_notes",
    "issue.set_auto_advance",
    "issue.stage_approve",
    "issue.stage_doc",
    "issue.stage_fix",
    "issue.stage_revise",
    "issue.stages",
    "models.list",
    "ping",
    "plan.abandon",
    "plan.approve",
    "plan.archive",
    "plan.comment_add",
    "plan.comment_delete",
    "plan.create",
    "plan.delete",
    "plan.doc",
    "plan.get",
    "plan.list",
    "plan.message",
    "plan.send_notes",
    "plan.stage_approve",
    "plan.stage_doc",
    "plan.stage_send_notes",
    "plan.stages",
    "project.add",
    "project.clone",
    "project.create",
    "project.init_git",
    "project.list",
    "project.set_isolation",
    "project.set_remote",
    "rtc.close",
    "rtc.ice",
    "rtc.offer",
    "run.abandon",
    "run.adopt",
    "run.create",
    "run.delete",
    "run.finish",
    "run.get",
    "run.git_action",
    "run.message",
    "run.release",
    "run.request_changes",
    "run.set_auto_advance",
    "run.stage_dispatch",
    "run.stage_fix",
    "run.stage_send_notes",
    "session.hello",
    "settings.get",
    "settings.set",
    "term.ack",
    "term.attach",
    "term.close",
    "term.create",
    "term.input",
    "term.list",
    "term.resize",
    "thread.activity",
    "thread.attach",
    "thread.attachment",
    "thread.operation",
    "thread.page",
    "thread.post",
    "thread.revision",
    "triage.override",
    "worktree.create",
    "worktree.finish",
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
