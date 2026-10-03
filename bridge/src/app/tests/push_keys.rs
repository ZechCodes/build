//! `push.registerKey` / `push.revokeKey` (#200, wire 2.1.0): the browser's
//! notification public key reaches the bridge only over E2EE, is validated
//! when it arrives, and is what push content is sealed to.

use super::*;

const SID: &str = "fktz0HaQPrTBONQORgaYdrkQLm41LU5PVktHaqho6To";
const KEY: &str =
    "BHKs2HtDjA-uaSJ6Pgf6fSznoiTDkYCYcz7DWi9OXLG0awHT6hXC01Suu7R305cuZOrOj_tPPOhZuv3udC929qc";

fn stored(root: &std::path::Path) -> AppState {
    super::project_agent::rooted(root)
        .with_task_store(root.join("store"))
        .expect("the store opens")
}

fn keys(state: &AppState) -> Vec<(String, String)> {
    state
        .store
        .as_ref()
        .unwrap()
        .list_push_keys()
        .unwrap()
        .into_iter()
        .map(|key| (key.subscription_id, key.public_key))
        .collect()
}

fn register(state: &mut AppState, sid: &str, key: &str) -> Value {
    state.handle(req(
        "push.registerKey",
        json!({ "subscription_id": sid, "public_key": key }),
    ))
}

#[test]
fn a_registered_key_is_stored_and_a_revoked_one_forgotten() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = stored(tmp.path());

    let registered = register(&mut state, SID, KEY);
    assert_eq!(registered["ok"], true, "{registered:?}");
    assert_eq!(registered["result"], json!({}));
    assert_eq!(keys(&state), vec![(SID.to_string(), KEY.to_string())]);

    let again = register(&mut state, SID, KEY);
    assert_eq!(
        again["ok"], true,
        "registering again is an upsert: {again:?}"
    );
    assert_eq!(keys(&state).len(), 1);

    let revoked = state.handle(req("push.revokeKey", json!({ "subscription_id": SID })));
    assert_eq!(revoked["ok"], true, "{revoked:?}");
    assert_eq!(revoked["result"], json!({}));
    assert!(keys(&state).is_empty());

    let unknown = state.handle(req("push.revokeKey", json!({ "subscription_id": SID })));
    assert_eq!(
        unknown["ok"], true,
        "revoking twice is no error: {unknown:?}"
    );
}

/// Anything but a 43-character base64url sid and an uncompressed P-256 point
/// is refused, in a sentence, and nothing is stored.
#[test]
fn a_malformed_sid_or_key_is_refused_in_a_sentence() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = stored(tmp.path());
    let compressed = &KEY[..44];
    for (sid, key) in [
        ("too-short", KEY),
        (&format!("{}+", &SID[..42]), KEY),
        (SID, "not a key"),
        (SID, compressed),
        (SID, "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        let refused = register(&mut state, sid, key);
        assert_eq!(refused["ok"], false, "{sid} {key}: {refused:?}");
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
        let message = refused["error"].as_str().unwrap();
        assert!(message.starts_with("Build cannot "), "{message}");
        assert!(message.ends_with('.'), "{message}");
        assert!(!message.contains(key) || key.len() < 12, "the key is not echoed: {message}");
    }
    let refused = state.handle(req("push.revokeKey", json!({ "subscription_id": "nope" })));
    assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    assert!(keys(&state).is_empty());
}

#[test]
fn a_bridge_without_a_store_says_it_cannot_keep_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = super::project_agent::rooted(tmp.path());
    let refused = register(&mut state, SID, KEY);
    assert_eq!(refused["error_code"], "unavailable", "{refused:?}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .starts_with("Build cannot "));
}

#[test]
fn both_verbs_are_announced_by_name() {
    let announced = crate::api::capabilities(false);
    assert!(announced.contains(&"push.registerKey"));
    assert!(announced.contains(&"push.revokeKey"));
    assert_eq!(crate::api::API_VERSION, "3.9.0");
}
