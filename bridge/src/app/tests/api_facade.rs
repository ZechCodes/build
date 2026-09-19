//! The `api::v1` facade as the wire sees it: structured errors beside the
//! string, v1 before legacy, and the QA stream verbs behind the QA flag.

use super::*;

fn project_id_of(state: &mut AppState) -> String {
    let listed = state.handle(req("project.list", json!({})));
    listed["result"]["projects"][0]["project_id"]
        .as_str()
        .expect("a project row")
        .to_string()
}

#[test]
fn an_unknown_method_is_refused_with_a_structured_error() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let refused = state.handle(req("no.such_verb", json!({})));
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "unknown method: no.such_verb");
    assert_eq!(refused["error_code"], "unknown_method");
    assert_eq!(refused["retryable"], false);
    assert_eq!(refused["details"], json!({ "method": "no.such_verb" }));
}

#[test]
fn a_typed_verb_missing_a_required_param_answers_invalid_params() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let refused = state.handle(req("git.diff", json!({ "project_id": project_id })));
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "missing required param: paths");
    assert_eq!(refused["error_code"], "invalid_params");
    assert_eq!(refused["retryable"], false);
    assert!(refused.get("details").is_none(), "{refused}");
}

#[test]
fn a_typed_verb_naming_a_missing_entity_answers_not_found() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let refused = state.handle(req("git.status", json!({ "project_id": "proj-nope" })));
    assert_eq!(refused["error"], "unknown project_id");
    assert_eq!(refused["error_code"], "not_found");
}

#[test]
fn a_success_reply_carries_no_error_fields() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let status = state.handle(req("git.status", json!({ "project_id": project_id })));
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["branch"], "main");
    assert!(status.get("error_code").is_none());
    assert!(status.get("retryable").is_none());
}

#[test]
fn v1_serves_every_family_and_the_legacy_route_answers_none_of_them() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let params = json!({ "project_id": project_id });
    for (method, params) in [
        ("git.status", &params),
        ("fs.list", &json!({})),
        ("board.list", &json!({})),
        ("thread.page", &json!({})),
        ("issue.list", &json!({})),
        ("run.get", &json!({})),
    ] {
        assert!(
            crate::api::v1::dispatch(&mut state, method, params).is_some(),
            "{method}: v1 serves it"
        );
        // ... and the legacy table has no arm left for it, so nothing can
        // answer a verb twice or drift between the two answers.
        assert!(
            state.route_legacy(method, params).is_none(),
            "{method}: the legacy route still has an arm"
        );
    }
    // The probe is the one verb an old client sends before it knows what it
    // is talking to, so it stays where it always was.
    assert!(crate::api::v1::dispatch(&mut state, "ping", &json!({})).is_none());
    assert!(state.route_legacy("ping", &json!({})).is_some());
}

#[test]
fn the_qa_stream_verbs_are_unknown_unless_the_qa_agent_is_on() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        false,
        dir.path().join("mcp.sock").display().to_string(),
    );
    for method in ["stream.events", "stream.state"] {
        let refused = state.handle(req(method, json!({ "stream_id": "stream-1" })));
        assert_eq!(
            refused["error_code"], "unknown_method",
            "{method}: {refused}"
        );
        assert_eq!(refused["error"], format!("unknown method: {method}"));
    }
    let mut qa = qa_state(&dir.path().join("repo"), dir.path());
    let refused = qa.handle(req("stream.state", json!({ "stream_id": "stream-1" })));
    assert_eq!(refused["error"], "unknown stream_id", "{refused}");
    assert_eq!(refused["error_code"], "internal");
}

/// The verbs whose refusal still reads `internal` on at least one of the two
/// probes below, with how many of the probes it did — the census wire spec
/// step 2.4 asks for. A refusal reads `internal` when neither
/// `ApiError::classify` nor the family's own `refine` could name it, so an
/// entry here is a sentence the facade does not yet understand. The list may
/// only shrink: naming a code for one of these removes its line; a new verb
/// or a reworded refusal that lands here fails the test.
const INTERNAL_REFUSALS: &[(&str, usize)] = &[
    ("git.checkout", 1),
    ("git.branch_delete", 1),
    ("issue.diff", 1),
    ("issue.stage_diff", 1),
    ("entity.dismiss", 1),
];

/// Verbs whose fixture params reach outside the state under test — a path
/// under `~`, a remote to clone, a settings write — so only the empty probe
/// is sent to them.
const FIXTURE_PROBE_REACHES_OUTSIDE: &[&str] = &[
    "fs.mkdir",
    "project.add",
    // Its fixture names a folder under `~` to open as a source.
    "project.add_source",
    "project.clone",
    "project.create",
    // Its confirmed fixture would remove the project used by later probes.
    "project.delete",
    "settings.set",
    // Both provision on disk: a create copies every source of the project it
    // names, and an init writes a repository into a directory it resolved.
    "workspace.create",
    "workspace.init_git",
];

fn fixture_params(method: &str) -> Value {
    crate::api::v1::testing::fixture(method)["params"].clone()
}

#[test]
fn the_internal_refusal_census_can_only_shrink() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut census: Vec<(&str, usize)> = Vec::new();
    let mut sentences: Vec<String> = Vec::new();
    for (method, _) in crate::api::v1::methods() {
        let mut probes = vec![json!({})];
        if !FIXTURE_PROBE_REACHES_OUTSIDE.contains(method) {
            probes.push(fixture_params(method));
        }
        let internal = probes
            .into_iter()
            .filter_map(|params| state.dispatch_api(method, &params).err())
            .filter(|refusal| refusal.code() == "internal")
            .inspect(|refusal| sentences.push(format!("{method}: {}", refusal.message())))
            .count();
        if internal > 0 {
            census.push((method, internal));
        }
    }
    assert_eq!(
        census,
        INTERNAL_REFUSALS,
        "the internal refusals the facade cannot name:\n{}",
        sentences.join("\n")
    );
}

/// A `git.*` verb answers through the deferred drain, so the check inside
/// `api::v1::answer` sees only the `Value::Null` placeholder. The declared
/// result type travels with the job instead, and the drain holds the real
/// value to it: an implementation whose shape has drifted is this bridge's
/// bug, and reads as `internal`.
#[test]
fn a_deferred_git_verb_answering_the_wrong_shape_is_reported_as_internal() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let params = json!({ "project_id": project_id });

    let (answered, deferred) = state.dispatch_deferring("git.status", &params);
    assert_eq!(
        answered.expect("git.status resolved under the lock"),
        Value::Null,
        "the deferral placeholder"
    );
    let mut done = deferred.expect("git.status defers its git").run();
    done.answer_instead(json!({ "branch": 7 }));

    let refused = crate::api::ApiError::classify(
        state
            .apply_deferred("git.status", &params, done)
            .expect_err("a wrong-shaped deferred reply is not published"),
    );
    assert_eq!(refused.code(), "internal");
    assert!(
        refused.message().starts_with("git.status: "),
        "the refusal names the method: {}",
        refused.message()
    );
    assert!(
        refused.message().contains("GitStatusResult"),
        "the refusal carries what serde refused, naming the declared type: {}",
        refused.message()
    );
}

#[test]
fn a_deferred_git_verb_answering_its_declared_shape_is_published() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let params = json!({ "project_id": project_id });

    let (_, deferred) = state.dispatch_deferring("git.status", &params);
    let done = deferred.expect("git.status defers its git").run();

    let raw = state.dispatch("git.status", &params);
    println!(
        "RAW {}",
        serde_json::to_string_pretty(&raw.unwrap()).unwrap()
    );
    let published = state
        .apply_deferred("git.status", &params, done)
        .expect("the implementation's own shape passes its declared type");
    assert_eq!(published["branch"], "main", "{published}");
}

/// Wire spec step 2.4: structured errors are ADDITIVE in 1.x, so `error`
/// stays the free-text string it always was and `error_code`, `retryable`
/// and the optional `details` sit beside it. A 1.0 client reads `error` and
/// nothing else, so a refusal that answered the code alone — or answered an
/// object there — would break it silently.
///
/// Walked over every road a refusal can take out of the facade, because the
/// shape is a property of the reply and not of any one verb: the unknown
/// method, a typed v1 param check, a legacy-route `Err(String)` with no code
/// of its own, a refusal decided off the lock and applied on the way back,
/// and the retirement guard that runs before either route.
#[test]
fn every_refusal_carries_the_string_error_beside_its_code() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = project_id_of(&mut state);
    let refusals = [
        (
            "unknown method",
            "no.such_verb",
            json!({}),
            "unknown_method",
        ),
        (
            "a v1 typed-params failure",
            "git.diff",
            json!({ "project_id": project_id }),
            "invalid_params",
        ),
        (
            "a legacy-route refusal",
            "stream.state",
            json!({ "stream_id": "stream-1" }),
            "internal",
        ),
        (
            "a refusal decided off the lock",
            "git.checkout",
            json!({ "project_id": project_id, "branch": "no-such-branch" }),
            "internal",
        ),
        (
            "the retirement guard",
            "run.create",
            json!({ "goal": "retired" }),
            "unavailable",
        ),
    ];
    for (road, method, params, code) in refusals {
        let refused = state.handle(req(method, params));
        assert_eq!(refused["ok"], false, "{road}: {refused}");
        assert!(
            refused["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{road}: `error` is the message, as a string: {refused}"
        );
        assert_eq!(refused["error_code"], code, "{road}: {refused}");
        assert_eq!(refused["retryable"], false, "{road}: {refused}");
        assert!(
            refused
                .get("details")
                .is_none_or(serde_json::Value::is_object),
            "{road}: `details` is optional, and an object when it is there: {refused}"
        );
    }

    // The off-lock road above is only that road if the verb does defer: a
    // `git.checkout` that had quietly become synchronous would still have
    // passed the walk. Run the job to its end rather than dropping a claim.
    let params = json!({ "project_id": project_id, "branch": "no-such-branch" });
    let (_, deferred) = state.dispatch_deferring("git.checkout", &params);
    let done = deferred
        .expect("git.checkout decides its git off the lock")
        .run();
    assert!(
        state.apply_deferred("git.checkout", &params, done).is_err(),
        "the refusal walked above is the one the write-back half carries"
    );
}
