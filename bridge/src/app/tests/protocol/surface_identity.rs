use super::*;

fn agent_digest(state: &AppState, run_id: &str) -> Value {
    state.agent_digests(run_id, DigestScope::Detail)[0].clone()
}

#[test]
fn surface_cache_identity_tracks_the_process_even_without_a_surface_snapshot() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-surface-identity");
    let agent_id = primary_agent_id(&state, &run_id);
    let root = state.entity_agent_root(&run_id).expect("the run checkout");
    let key = TabKey::agent(&root, &agent_id);

    let mut first = dictated_agent_tab(
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    );
    first.session_instance.as_mut().unwrap().id = "process-generation-one".into();
    state.session_registry.test_insert_tab(key.clone(), first);

    let first_digest = agent_digest(&state, &run_id);
    assert_eq!(
        first_digest["surface_session_generation"],
        "process-generation-one"
    );
    assert!(first_digest.get("surfaces").is_none(), "{first_digest:?}");

    let mut replacement = dictated_agent_tab(
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    );
    replacement.session_instance.as_mut().unwrap().id = "process-generation-two".into();
    state
        .session_registry
        .test_insert_tab(key.clone(), replacement);

    let replacement_digest = agent_digest(&state, &run_id);
    assert_eq!(
        replacement_digest["surface_session_generation"],
        "process-generation-two"
    );
    assert!(
        replacement_digest.get("surfaces").is_none(),
        "{replacement_digest:?}"
    );

    state.session_registry.test_remove_tab(&key);
    assert_eq!(
        agent_digest(&state, &run_id)["surface_session_generation"],
        Value::Null,
        "absence tells the client to discard the prior process snapshot"
    );
}

#[test]
fn list_and_detail_digests_carry_the_same_surface_process_identity() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-surface-wire");
    let agent_id = primary_agent_id(&state, &run_id);
    let root = state.entity_agent_root(&run_id).expect("the run checkout");
    let key = TabKey::agent(&root, &agent_id);
    let mut tab = dictated_agent_tab(
        &root,
        &run_id,
        &agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    );
    tab.session_instance.as_mut().unwrap().id = "process-generation-wire".into();
    state.session_registry.test_insert_tab(key, tab);

    for scope in [DigestScope::List, DigestScope::Detail] {
        let digest = state.agent_digests(&run_id, scope)[0].clone();
        assert_eq!(
            digest["surface_session_generation"], "process-generation-wire",
            "{scope:?}: {digest:?}"
        );
    }
}
