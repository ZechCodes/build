//! What a new agent runs on (spec: Agent roles).
//!
//! The user declares which models fill which roles and how much direction each
//! needs; an agent making another agent asks for a ROLE and is told both. The
//! effort is nobody's business but the creating agent's — which model reviews
//! is a standing decision, how hard it thinks about one review is not.

use super::project_agent::{project_agent, workspace};
use super::tracker::tracked;
use super::*;
use crate::mcp::BridgeAction;

/// Zech's own example, as a device would hold it.
fn declare_models(state: &mut AppState) {
    let set = state.handle(req(
        "settings.set",
        json!({
            "role_models": [
                { "model": "claude-fable-5-1", "roles": ["planner", "reviewer"], "capability": "generalist" },
                { "model": "claude-opus-5", "roles": ["planner", "reviewer", "implementer"], "capability": "scoped" },
                { "provider": "codex", "model": "gpt-5", "roles": ["implementer", "executor"], "capability": "step_by_step" }
            ]
        }),
    ));
    assert_eq!(set["ok"], true, "{set:?}");
}

fn conversation_on(state: &mut AppState, project_id: &str, name: &str) -> String {
    let ws = workspace(state, project_id, name);
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    ));
    conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The list is saved, read back, and answers a role with the user's model.
#[test]
fn a_role_is_answered_with_the_model_the_user_chose_for_it() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    declare_models(&mut state);

    let read = state.handle(req("settings.get", json!({})));
    assert_eq!(
        read["result"]["role_models"][0]["model"], "claude-fable-5-1",
        "{read:?}"
    );

    let entity_id = conversation_on(&mut state, &project_id, "here");
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "reviewer" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    // Fable and Opus both review; Fable is first in the list, so Fable
    // reviews. The order is the user's preference.
    assert_eq!(added["result"]["agent"]["model"], "claude-fable-5-1");
    assert_eq!(
        added["result"]["agent"]["effort"], "",
        "the user chose the model; the effort is the caller's to judge"
    );
}

/// The answer says how much direction the model it chose wants — which is the
/// whole reason to ask for a role rather than a model.
#[test]
fn the_answer_says_how_much_direction_that_model_needs() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    declare_models(&mut state);
    let entity_id = conversation_on(&mut state, &project_id, "here");

    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "executor" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(added["result"]["agent"]["model"], "gpt-5");
    assert_eq!(added["result"]["agent"]["provider"], "codex");
    assert_eq!(added["result"]["capability"], "step_by_step");
    assert!(
        added["result"]["direction"]
            .as_str()
            .unwrap()
            .contains("give it the steps"),
        "said as the instruction it is: {added:?}"
    );
}

/// A capability narrows the choice, and asking for one nobody is says so
/// rather than quietly starting something else.
#[test]
fn a_capability_narrows_the_choice_and_says_so_when_nothing_fits() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    declare_models(&mut state);
    let entity_id = conversation_on(&mut state, &project_id, "here");

    let stepwise = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "implementer", "capability": "step_by_step" }),
    ));
    assert_eq!(stepwise["ok"], true, "{stepwise:?}");
    assert_eq!(
        stepwise["result"]["agent"]["model"], "gpt-5",
        "not Opus, which implements but is scoped"
    );

    let nobody = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "executor", "capability": "generalist" }),
    ));
    assert_eq!(nobody["ok"], false, "{nobody:?}");
    let said = nobody["error"].as_str().unwrap();
    assert!(
        said.contains("No model on this device is a generalist executor"),
        "{said}"
    );
    assert!(
        said.contains("Roles with a model:"),
        "leaves somewhere to go: {said}"
    );
}

/// A word this bridge does not know is refused by name; a role the user has
/// declared nobody for falls back rather than refusing, because the caller's
/// own choice and the device default are both still good answers.
#[test]
fn an_unknown_role_is_refused_and_an_undeclared_one_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let entity_id = conversation_on(&mut state, &project_id, "here");

    let refused = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "reviewing" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let said = refused["error"].as_str().unwrap();
    assert!(said.contains("no \"reviewing\" role"), "{said}");
    assert!(
        said.contains("\"reviewer\""),
        "names the ones there are: {said}"
    );
    assert!(said.ends_with('.'), "reads as a sentence: {said}");

    // Nothing declared at all: the role is ignored and the device default
    // stands, because the caller asked a question this device has no opinion
    // on rather than a wrong one.
    let fell_back = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "role": "reviewer" }),
    ));
    assert_eq!(fell_back["ok"], true, "{fell_back:?}");
    assert_eq!(fell_back["result"]["agent"]["provider"], "claude_adk");
    assert!(fell_back["result"]["capability"].is_null());
}

/// Naming a model still wins — the user may have asked for one by name — and
/// an effort is always the caller's, never the list's.
#[test]
fn an_explicit_model_overrides_the_role_and_the_effort_is_always_the_callers() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    declare_models(&mut state);
    let entity_id = conversation_on(&mut state, &project_id, "here");

    let added = state.handle(req(
        "agent.add",
        json!({
            "entity_id": entity_id,
            "role": "reviewer",
            // The user asked for this one by name.
            "model": "claude-opus-5",
            "effort": "max",
        }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(added["result"]["agent"]["model"], "claude-opus-5");
    assert_eq!(added["result"]["agent"]["effort"], "max");
}

/// A model this bridge's catalog has never heard of is NOT refused: the user
/// may be naming one newer than the bridge, and the lookup exists so an agent
/// can honour that (Zech, 23:34Z: "Don't gate").
#[test]
fn a_model_the_catalog_does_not_know_is_still_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    let entity_id = conversation_on(&mut state, &project_id, "here");

    let accepted = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "provider": "claude_adk", "model": "claude-opus-7" }),
    ));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(accepted["result"]["agent"]["model"], "claude-opus-7");

    // A harness is a closed set, though: Build can only run what it has.
    let refused = state.handle(req(
        "agent.add",
        json!({ "entity_id": entity_id, "provider": "something-else" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("unknown agent provider"),
        "{refused:?}"
    );
}

/// `list_harnesses` is the lookup: every harness, whether it is installed
/// here, and which model answers each role.
#[test]
fn list_harnesses_is_the_lookup_and_says_what_fills_each_role() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, project_id) = tracked(&state_root);
    declare_models(&mut state);
    let (owner, agent_id) = project_agent(&mut state, &project_id);

    let table = state
        .on_agent_mcp_action(&owner, &agent_id, BridgeAction::ListHarnesses)
        .expect("any working agent may read it");

    let harnesses = table["harnesses"].as_array().unwrap();
    assert!(!harnesses.is_empty(), "{table:?}");
    for harness in harnesses {
        assert!(harness["installed"].is_boolean(), "{harness:?}");
        assert!(harness["binary"].is_string(), "{harness:?}");
        assert!(harness["models"].is_array(), "{harness:?}");
    }
    assert_eq!(table["default_harness"], "claude_adk");
    assert_eq!(table["roles"].as_array().unwrap().len(), 4);
    assert_eq!(table["capabilities"].as_array().unwrap().len(), 3);
    assert_eq!(table["role_models"].as_array().unwrap().len(), 3);

    // Resolved: which model answers each role, and how much direction it
    // wants. An agent reads the answer rather than the rule.
    assert_eq!(
        table["roles_in_effect"]["reviewer"]["model"],
        "claude-fable-5-1"
    );
    assert_eq!(
        table["roles_in_effect"]["reviewer"]["capability"],
        "generalist"
    );
    assert_eq!(
        table["roles_in_effect"]["implementer"]["model"],
        "claude-opus-5"
    );
    assert_eq!(table["roles_in_effect"]["executor"]["model"], "gpt-5");
    assert!(
        table["roles_in_effect"]["executor"]["direction"]
            .as_str()
            .unwrap()
            .contains("steps"),
        "{table:?}"
    );
}

/// A list this device could not honour is refused when it is SAVED, not when
/// an agent is made from it minutes later.
#[test]
fn a_list_that_could_not_be_honoured_is_refused_at_the_set() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (_home, mut state, _project_id) = tracked(&state_root);

    let twice = state.handle(req(
        "settings.set",
        json!({
            "role_models": [
                { "model": "claude-opus-5", "roles": ["planner"], "capability": "scoped" },
                { "model": "claude-opus-5", "roles": ["reviewer"], "capability": "generalist" }
            ]
        }),
    ));
    assert_eq!(twice["ok"], false, "{twice:?}");
    assert!(
        twice["error"]
            .as_str()
            .unwrap()
            .contains("in the list twice"),
        "{twice:?}"
    );

    let unknown_role = state.handle(req(
        "settings.set",
        json!({ "role_models": [{ "model": "m", "roles": ["reviewing"], "capability": "scoped" }] }),
    ));
    assert_eq!(unknown_role["ok"], false, "{unknown_role:?}");
    assert!(
        unknown_role["error"]
            .as_str()
            .unwrap()
            .contains("role_models"),
        "{unknown_role:?}"
    );

    // And nothing was written.
    let read = state.handle(req("settings.get", json!({})));
    assert_eq!(read["result"]["role_models"], json!([]), "{read:?}");
}
