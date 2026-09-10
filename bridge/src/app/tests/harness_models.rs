use super::*;

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: models_list_serves_the_catalog_and_effort_levels is at 16, threshold 15 — bring it under, then remove
fn models_list_serves_the_catalog_and_effort_levels() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock");
    let res = state.handle(req("models.list", json!({})));
    assert_eq!(res["ok"], true, "{res:?}");
    assert_eq!(
        res["result"]["default_provider"], "claude_adk",
        "what a start leads with is the account's default harness: {res:?}"
    );
    let models = res["result"]["models"].as_array().unwrap();
    assert!(models.iter().any(|m| m["id"] == "claude-opus-4-8"));
    assert!(models.iter().any(|m| m["id"] == "claude-opus-5"));
    assert!(models.iter().all(|m| m["supports_effort"].is_boolean()));
    let efforts = res["result"]["efforts"].as_array().unwrap();
    assert!(efforts.iter().any(|e| e == "xhigh"));
    let providers = res["result"]["providers"].as_array().unwrap();
    let codex = providers.iter().find(|p| p["id"] == "codex").unwrap();
    let app_server = providers
        .iter()
        .find(|provider| provider["id"] == "codex_app_server")
        .unwrap();
    assert_eq!(codex["label"], "Codex TUI");
    assert_eq!(app_server["label"], "Codex");
    assert_eq!(codex["models"], app_server["models"]);
    assert_eq!(codex["efforts"], app_server["efforts"]);
    assert!(codex["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["id"] == "gpt-5.6-sol"));
    assert!(codex["efforts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "ultra"));
    let pi = providers
        .iter()
        .find(|provider| provider["id"] == "pi")
        .unwrap();
    assert_eq!(pi["label"], "Pi");
    assert_eq!(pi["models"], json!([]));
    assert_eq!(
        pi["efforts"],
        json!(["off", "minimal", "low", "medium", "high", "xhigh", "max"])
    );
}

#[test]
fn pi_specs_use_the_configured_store_state_root() {
    let state_dir = tempfile::tempdir().unwrap();
    let (_repo_dir, repo) = init_repo();
    let private_state = state_dir.path().join("configured-state");
    let worktree = state_dir.path().join("worktree");
    std::fs::create_dir(&worktree).unwrap();
    let context =
        HarnessContext::resolved(state_dir.path().join("mcp.sock"), private_state.clone()).unwrap();
    let state = AppState::new_configured(
        repo,
        state_dir.path().join("worktrees"),
        "main",
        false,
        context,
    )
    .with_task_store(private_state.join("tasks"))
    .unwrap();
    assert_eq!(
        state.state_root,
        std::fs::canonicalize(&private_state).unwrap()
    );
    let Agent::WarmBuilder(build) = &state.agent else {
        panic!("real agent is provider-aware");
    };
    let spec = build(
        "",
        &ModelChoice {
            provider: AgentProvider::Pi,
            model: None,
            effort: Some("minimal".to_string()),
        },
        &SpawnOptions {
            owner_id: "agent-state-root".to_string(),
            mcp_session_token: "token".to_string(),
            cwd: worktree,
            ..SpawnOptions::default()
        },
    )
    .unwrap();
    let canonical_state = std::fs::canonicalize(&private_state).unwrap();
    assert!(Path::new(&spec.args[3]).starts_with(canonical_state.join("harness/pi")));
    assert!(Path::new(&spec.args[5]).starts_with(canonical_state.join("harness/pi")));
    let bridge = spec
        .env
        .iter()
        .find(|(key, _)| key == "BUILD_PI_MCP_COMMAND")
        .unwrap();
    assert!(Path::new(&bridge.1).is_absolute());
    assert_eq!(
        PathBuf::from(&bridge.1),
        std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
    );
}

#[test]
fn every_harness_path_uses_the_contexts_canonical_bridge_executable() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_dir, repo) = init_repo();
    let bridge_exe = directory.path().join("canonical-build-bridge");
    std::fs::write(&bridge_exe, "test executable").unwrap();
    let bridge_exe = std::fs::canonicalize(bridge_exe).unwrap();
    let state_root = directory.path().join("state");
    std::fs::create_dir(&state_root).unwrap();
    let context = HarnessContext {
        bridge_exe: bridge_exe.clone(),
        mcp_socket: directory.path().join("mcp.sock"),
        state_root: std::fs::canonicalize(state_root).unwrap(),
    };
    let state = AppState::new_configured(
        repo,
        directory.path().join("worktrees"),
        "main",
        false,
        context,
    );
    let Agent::WarmBuilder(build) = &state.agent else {
        panic!("real agent is provider-aware");
    };
    let worktree = directory.path().join("agent-worktree");
    std::fs::create_dir(&worktree).unwrap();
    let options = SpawnOptions {
        owner_id: "agent-one-executable".to_string(),
        mcp_session_token: "token".to_string(),
        cwd: worktree.clone(),
        ..SpawnOptions::default()
    };

    let pi = build(
        "",
        &ModelChoice {
            provider: AgentProvider::Pi,
            model: None,
            effort: None,
        },
        &options,
    )
    .unwrap();
    assert!(pi
        .env
        .iter()
        .any(|(key, value)| { key == "BUILD_PI_MCP_COMMAND" && Path::new(value) == bridge_exe }));

    let codex = build(
        "",
        &ModelChoice {
            provider: AgentProvider::Codex,
            model: None,
            effort: None,
        },
        &options,
    )
    .unwrap();
    let bridge_exe_text = bridge_exe.to_string_lossy();
    assert!(
        codex.args.join(" ").contains(bridge_exe_text.as_ref()),
        "Codex must receive the same executable: {:?}",
        codex.args
    );

    state
        .project_at(0)
        .orch
        .agent_launch()
        .prepare(
            &options.owner_id,
            &worktree,
            &ModelChoice::default(),
            false,
            None,
            &options.mcp_session_token,
        )
        .unwrap();
    let scaffold: Value = serde_json::from_str(
        &std::fs::read_to_string(
            worktree.join(crate::orchestrator::mcp_config_path(&options.owner_id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        scaffold["mcpServers"]["build"]["command"],
        bridge_exe.to_string_lossy().as_ref(),
        "Claude's scaffold must use the same executable fact"
    );
}

#[test]
fn task_store_parent_must_match_the_configured_state_root() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_dir, repo) = init_repo();
    let configured_state = directory.path().join("configured-state");
    let other_state = directory.path().join("other-state");
    std::fs::create_dir(&other_state).unwrap();
    let context =
        HarnessContext::resolved(directory.path().join("mcp.sock"), configured_state.clone())
            .unwrap();

    let error = AppState::new_configured(
        repo,
        directory.path().join("worktrees"),
        "main",
        false,
        context,
    )
    .with_task_store(other_state.join("tasks"))
    .err()
    .expect("a detached task store must be rejected");

    assert!(
        error.contains(&configured_state.display().to_string()),
        "{error}"
    );
    assert!(
        error.contains(&other_state.display().to_string()),
        "{error}"
    );
}

#[test]
fn configured_pi_router_is_rejected_and_the_default_stays_claude_adk() {
    let (dir, repo) = init_repo();
    let config = dir.path().join("config.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({
            "router_model": { "provider": "pi", "effort": "low" },
            "projects": []
        }))
        .unwrap(),
    )
    .unwrap();
    let state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/mcp.sock")
        .with_config(config)
        .unwrap();
    assert!(state.router_choice.is_none());
}

/// Build mints a fresh worktree per run, and an interactive harness gates a
/// directory it has not seen behind a workspace-trust dialog. That dialog
/// owns the keyboard, so the injected prompt lands in it and the trailing
/// submit key answers it — the agent receives nothing and the run parks
/// until the idle sweep demotes it. Codex takes the grant as a per-invocation
/// `--config`, so nothing outside this spawn is touched.
#[test]
fn codex_argv_pre_trusts_the_worktree_it_will_run_in() {
    let Agent::WarmBuilder(build) = test_build_agent("/tmp/m.sock") else {
        panic!("real agent should be a provider-aware warm TUI");
    };
    let choice = ModelChoice {
        provider: AgentProvider::Codex,
        ..ModelChoice::default()
    };
    let spec = build(
        "do the thing",
        &choice,
        &SpawnOptions {
            cwd: std::path::PathBuf::from("/tmp/build worktrees/run-9"),
            ..SpawnOptions::default()
        },
    )
    .unwrap();
    let args = spec.args.join(" ");
    assert!(
        args.contains(r#"projects."/tmp/build worktrees/run-9".trust_level="trusted""#),
        "{args}"
    );
}

/// Build injects a prompt and its submit key back-to-back. Codex otherwise
/// classifies that rapid character stream as a paste burst and consumes the
/// trailing Enter as a newline inside the paste, leaving the prompt visible
/// but unsent. Build's PTY supports bracketed paste, so the fallback burst
/// detector must be disabled for every Codex process it owns.
#[test]
fn codex_argv_disables_the_fallback_paste_burst_detector() {
    let Agent::WarmBuilder(build) = test_build_agent("/tmp/m.sock") else {
        panic!("real agent should be a provider-aware warm TUI");
    };
    let choice = ModelChoice {
        provider: AgentProvider::Codex,
        ..ModelChoice::default()
    };
    let spec = build("one line", &choice, &SpawnOptions::default()).unwrap();

    assert!(
        spec.args
            .windows(2)
            .any(|args| { args[0] == "--config" && args[1] == "disable_paste_burst=true" }),
        "Codex must not swallow Build's immediate submit key: {:?}",
        spec.args
    );
}

#[test]
fn real_tui_argv_includes_the_selected_model_and_effort() {
    let Agent::WarmBuilder(build) = test_build_agent("/tmp/m.sock") else {
        panic!("real agent should be a provider-aware warm TUI");
    };
    let choice = ModelChoice {
        provider: AgentProvider::Claude,
        model: Some("claude-opus-4-8".into()),
        effort: Some("xhigh".into()),
    };
    let spec = build("do the thing", &choice, &SpawnOptions::default()).unwrap();
    let args = spec.args.join(" ");
    assert_eq!(spec.binary, "claude");
    assert!(!args
        .split_whitespace()
        .any(|arg| arg == "-p" || arg == "--print"));
    assert!(!args.contains("do the thing"), "{args}");
    assert!(args.contains("--model claude-opus-4-8"), "{args}");
    assert!(args.contains("--effort xhigh"), "{args}");
    assert!(!args.contains("--continue"), "{args}");
    // Defaults add nothing: the user's harness config decides.
    let spec = build(
        "do the thing",
        &ModelChoice::default(),
        &SpawnOptions::default(),
    )
    .unwrap();
    assert!(!spec.args.join(" ").contains("--model"));
    // A continuation spawn resumes the cwd's conversation, flag placed right
    // after the permission arg and before any model args.
    let spec = build(
        "do the thing",
        &choice,
        &SpawnOptions {
            continue_session: true,
            ..SpawnOptions::default()
        },
    )
    .unwrap();
    let args = spec.args.join(" ");
    assert!(
        args.contains("--dangerously-skip-permissions --continue --model"),
        "{args}"
    );
}

#[test]
fn codex_tui_argv_wires_done_mcp_and_resumes_by_cwd() {
    let Agent::WarmBuilder(build) = test_build_agent("/tmp/build mcp.sock") else {
        panic!("real agent should be a provider-aware warm TUI");
    };
    let choice = ModelChoice {
        provider: AgentProvider::Codex,
        model: Some("gpt-5.6-sol".into()),
        effort: Some("ultra".into()),
    };
    let options = SpawnOptions {
        continue_session: false,
        owner_id: "run-7".into(),
        ..SpawnOptions::default()
    };
    let spec = build("do the thing", &choice, &options).unwrap();
    assert_eq!(spec.binary, "codex");
    let args = spec.args.join(" ");
    assert!(!args.contains("exec"), "{args}");
    assert!(
        args.contains("--dangerously-bypass-approvals-and-sandbox"),
        "{args}"
    );
    assert!(args.contains("--model gpt-5.6-sol"), "{args}");
    assert!(args.contains("model_reasoning_effort=\"ultra\""), "{args}");
    assert!(
        args.contains("mcp_servers.build.args=[\"mcp\",\"--task\",\"run-7\"]"),
        "{args}"
    );
    assert!(
        args.contains("mcp_servers.build.env.BRIDGE_MCP_SOCKET=\"/tmp/build mcp.sock\""),
        "{args}"
    );
    assert!(!args.contains("do the thing"), "{args}");

    let resumed = build(
        "a follow-up",
        &choice,
        &SpawnOptions {
            continue_session: true,
            owner_id: "run-7".into(),
            ..SpawnOptions::default()
        },
    )
    .unwrap();
    assert!(resumed.args.join(" ").ends_with("resume --last"));
}

#[test]
fn unrooted_state_has_no_phantom_project_until_one_is_added() {
    let (dir, repo) = init_repo();
    let mut state =
        AppState::new_unrooted(dir.path().join("wt"), "main", true, "/tmp/test-mcp.sock");
    let listed = state.dispatch("project.list", &json!({})).unwrap();
    assert_eq!(listed["projects"].as_array().unwrap().len(), 0);

    let added = state
        .dispatch("project.add", &json!({"path": repo.to_string_lossy()}))
        .unwrap();
    assert!(added["project_id"].as_str().unwrap().starts_with("proj-"));
    let listed = state.dispatch("project.list", &json!({})).unwrap();
    assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
}
