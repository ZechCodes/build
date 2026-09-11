use super::*;

/// Naming no provider means "the account's default harness"; naming one
/// means that harness, concretely, whatever the account prefers. `"claude"`
/// is the terminal provider and nothing else — an agent is locked to what it
/// was created on, so no token is left to be re-read later.
#[test]
fn silence_follows_the_default_harness_and_every_token_is_concrete() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let provider_of = |state: &mut AppState, params: Value| {
        let filed = state
            .plan_create(&params)
            .expect("the legacy fixture is filed");
        let plan_id = filed["plan_id"].as_str().expect("the fixture has an id");
        state.plans[plan_id].model_choice.provider
    };

    assert_eq!(
        provider_of(
            &mut state,
            json!({ "goal": "silence takes the default", "dispatch": false })
        ),
        AgentProvider::ClaudeAdk
    );

    let set = state.handle(req("settings.set", json!({ "default_harness": "codex" })));
    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(
        provider_of(
            &mut state,
            json!({ "goal": "and follows it when it moves", "dispatch": false })
        ),
        AgentProvider::Codex
    );

    for (token, provider) in [
        ("claude", AgentProvider::Claude),
        ("claude_adk", AgentProvider::ClaudeAdk),
        ("codex", AgentProvider::Codex),
        ("codex_app_server", AgentProvider::CodexAppServer),
        ("pi", AgentProvider::Pi),
    ] {
        assert_eq!(
            provider_of(
                &mut state,
                json!({ "goal": format!("named {token}"), "dispatch": false, "provider": token })
            ),
            provider,
            "{token} names one harness"
        );
    }
}

#[test]
fn pi_setting_drives_omitted_coding_provider_without_overriding_a_concrete_provider() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let set = state.handle(req("settings.set", json!({ "default_harness": "pi" })));
    assert_eq!(set["ok"], true, "{set:?}");

    let omitted = state
        .plan_create(&json!({ "goal": "use the account default", "dispatch": false }))
        .expect("the legacy fixture is filed");
    assert_eq!(omitted["provider"], "pi", "{omitted:?}");
    assert_eq!(omitted["agents"][0]["provider"], "pi");

    let concrete = state
        .plan_create(&json!({
            "goal": "keep the provider displayed by the client",
            "dispatch": false,
            "provider": "codex",
        }))
        .expect("the legacy fixture is filed");
    assert_eq!(concrete["provider"], "codex", "{concrete:?}");
    assert_eq!(concrete["agents"][0]["provider"], "codex");
}

/// A client that names a concrete carrier gets that carrier. The setting
/// answers the generic question only — it is not a veto over a caller who
/// already knows what it wants.
#[test]
fn a_concretely_named_provider_is_honored_whatever_the_setting_says() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let provider_of = |state: &mut AppState, goal: &str, provider: &str| {
        let filed = state
            .plan_create(&json!({ "goal": goal, "dispatch": false, "provider": provider }))
            .expect("the legacy fixture is filed");
        let plan_id = filed["plan_id"].as_str().expect("the fixture has an id");
        state.plans[plan_id].model_choice.provider
    };

    for default in ["claude_adk", "claude", "codex", "codex_app_server", "pi"] {
        let set = state.handle(req("settings.set", json!({ "default_harness": default })));
        assert_eq!(set["ok"], true, "{set:?}");
        assert_eq!(
            provider_of(&mut state, &format!("adk under {default}"), "claude_adk"),
            AgentProvider::ClaudeAdk
        );
        assert_eq!(
            provider_of(&mut state, &format!("codex under {default}"), "codex"),
            AgentProvider::Codex
        );
    }
}

/// Resolution happens when a choice is minted and never again, so changing
/// the account setting moves no work that already exists: every entity
/// keeps the concrete provider its record names.
#[test]
fn changing_the_setting_migrates_no_entity_that_already_exists() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let filed = state
        .plan_create(&json!({ "goal": "filed under the old default", "dispatch": false }))
        .expect("the legacy fixture is filed");
    let plan_id = filed["plan_id"]
        .as_str()
        .expect("the fixture has an id")
        .to_string();
    let before = state.entity_model_choice(&plan_id).unwrap();
    assert_eq!(before.provider, AgentProvider::ClaudeAdk);

    let set = state.handle(req("settings.set", json!({ "default_harness": "claude" })));
    assert_eq!(set["ok"], true, "{set:?}");

    assert_eq!(
        state.entity_model_choice(&plan_id).unwrap(),
        before,
        "the choice a later start and every resume read is untouched"
    );
}

/// Routing remains pinned to the Claude protocol carrier: adding another
/// headless carrier must not silently move existing router work to a
/// different provider or model family.
#[test]
fn the_router_pins_the_headless_provider_under_every_default() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    assert_eq!(
        crate::router::router_model_choice(state.router_choice.as_ref()).provider,
        AgentProvider::ClaudeAdk
    );
    for default in ["claude", "codex", "claude_adk", "codex_app_server", "pi"] {
        state.handle(req("settings.set", json!({ "default_harness": default })));
        assert_eq!(
            crate::router::router_model_choice(state.router_choice.as_ref()).provider,
            AgentProvider::ClaudeAdk,
            "the router does not follow a default of {default}"
        );
    }
}

#[tokio::test]
async fn stream_resume_reconstructs_full_output() {
    let (dir, repo) = init_repo();
    let handler = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .into_handler();
    let call = |method: &str, params: Value| {
        handler.call(SessionSender::detached("s"), req(method, params))
    };

    let started = call("stream.start", json!({ "count": 50, "interval_ms": 0 }));
    let stream_id = started["result"]["stream_id"].as_str().unwrap().to_string();

    // Wait for the producer to finish.
    loop {
        let st = call("stream.state", json!({ "stream_id": stream_id }));
        if st["result"]["complete"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // Resume in bounded batches from seq 0 — exactly how a reconnecting client
    // catches up. Collect the output text in order.
    let mut since = 0u64;
    let mut texts = Vec::new();
    loop {
        let ev = call(
            "stream.events",
            json!({ "stream_id": stream_id, "since": since, "limit": 7 }),
        );
        for e in ev["result"]["events"].as_array().unwrap() {
            if e["kind"] == "output" {
                texts.push(e["data"]["text"].as_str().unwrap().to_string());
            }
            // seqs are contiguous and strictly increasing.
        }
        let next = ev["result"]["next"].as_u64().unwrap();
        let head = ev["result"]["head"].as_u64().unwrap();
        let complete = ev["result"]["complete"].as_bool().unwrap();
        since = next;
        if complete && since >= head {
            break;
        }
    }

    // All 50 deterministic chunks, in order.
    assert_eq!(texts.len(), 50);
    assert_eq!(texts[0], "chunk-000000");
    assert_eq!(texts[49], "chunk-000049");

    // The client's reconstruction matches the bridge's authoritative checksum.
    let reconstructed = texts.join("\n");
    let st = call("stream.state", json!({ "stream_id": stream_id }));
    assert_eq!(
        st["result"]["checksum"].as_str().unwrap(),
        sha256_hex(reconstructed.as_bytes())
    );
}
