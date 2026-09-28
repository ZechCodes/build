use super::*;

/// Naming no provider means "the account's default harness"; naming one
/// means that harness, concretely, whatever the account prefers. `"claude"`
/// is the terminal provider and nothing else — an agent is locked to what it
/// was created on, so no token is left to be re-read later.
#[test]
fn silence_follows_the_default_harness_and_every_token_is_concrete() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let provider_of = |state: &AppState, params: Value| {
        model_choice_from(&params, state.default_harness)
            .expect("the choice resolves")
            .provider
    };

    assert_eq!(provider_of(&state, json!({})), AgentProvider::ClaudeAdk);

    let set = state.handle(req("settings.set", json!({ "default_harness": "codex" })));
    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(provider_of(&state, json!({})), AgentProvider::Codex);

    for (token, provider) in [
        ("claude", AgentProvider::Claude),
        ("claude_adk", AgentProvider::ClaudeAdk),
        ("codex", AgentProvider::Codex),
        ("codex_app_server", AgentProvider::CodexAppServer),
        ("pi", AgentProvider::Pi),
    ] {
        assert_eq!(
            provider_of(&state, json!({ "provider": token })),
            provider,
            "{token} names one harness"
        );
    }
}

/// A client that names a concrete carrier gets that carrier. The setting
/// answers the generic question only — it is not a veto over a caller who
/// already knows what it wants.
#[test]
fn a_concretely_named_provider_is_honored_whatever_the_setting_says() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let provider_of = |state: &AppState, provider: &str| {
        model_choice_from(&json!({ "provider": provider }), state.default_harness)
            .expect("the choice resolves")
            .provider
    };

    for default in ["claude_adk", "claude", "codex", "codex_app_server", "pi"] {
        let set = state.handle(req("settings.set", json!({ "default_harness": default })));
        assert_eq!(set["ok"], true, "{set:?}");
        assert_eq!(provider_of(&state, "claude_adk"), AgentProvider::ClaudeAdk);
        assert_eq!(provider_of(&state, "codex"), AgentProvider::Codex);
        assert_eq!(
            provider_of(&state, "pi"),
            AgentProvider::Pi,
            "a concrete pi under {default}"
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
    let plan_id = file_legacy_task(&mut state, "filed under the old default");
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
