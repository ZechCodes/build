use super::*;

#[test]
fn missing_config_is_the_only_absent_config_case() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-config.json");
    assert_eq!(read_config(&missing).unwrap(), None);
}

#[test]
fn a_saved_projects_directory_wins_over_the_device_default_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let environment_default = directory.path().join("environment-projects");
    let chosen = directory.path().join("chosen-projects");
    let new_app = || {
        AppState::new_unrooted(
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
    };

    let mut first_start = new_app()
        .with_projects_dir_default(environment_default.clone())
        .with_config(&config_path)
        .unwrap();
    assert_eq!(
        first_start.handle(req("settings.get", json!({})))["result"]["projects_dir"],
        environment_default.to_string_lossy().as_ref()
    );
    let saved = first_start.handle(req("settings.set", json!({ "projects_dir": chosen })));
    assert_eq!(saved["ok"], true, "{saved:?}");

    let mut restarted = new_app()
        .with_projects_dir_default(environment_default)
        .with_config(&config_path)
        .unwrap();
    assert_eq!(
        restarted.handle(req("settings.get", json!({})))["result"]["projects_dir"],
        chosen.to_string_lossy().as_ref()
    );
}

#[test]
fn malformed_config_fails_with_its_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("malformed-config.json");
    std::fs::write(&path, "{not json").unwrap();
    let error = read_config(&path).unwrap_err().to_string();
    assert!(error.contains(path.to_str().unwrap()), "{error}");
    assert!(error.contains("parse"), "{error}");
}

#[test]
fn config_read_failure_fails_with_its_path() {
    let directory = tempfile::tempdir().unwrap();
    let error = read_config(directory.path()).unwrap_err().to_string();
    assert!(
        error.contains(directory.path().to_str().unwrap()),
        "{error}"
    );
    assert!(error.contains("read"), "{error}");
}

#[test]
fn settings_write_failure_is_reported_and_leaves_account_modes_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let (_repo_dir, repo) = init_repo();
    let config = directory.path().join("config.json");
    let mut state = AppState::new(
        repo,
        directory.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    std::fs::create_dir(&config).unwrap();

    let before_modes = state.agent_modes;
    let response = state.handle(req(
        "settings.set",
        json!({
            "default_harness": "pi",
            "agent_modes": { "claude": "tui", "codex": "headless" }
        }),
    ));

    assert_eq!(response["ok"], false, "{response:?}");
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|error| error.contains(config.to_str().unwrap())),
        "{response:?}"
    );
    assert_eq!(state.default_harness, DEFAULT_HARNESS);
    assert_eq!(state.agent_modes, before_modes);
    assert!(
        !config.with_extension("tmp").exists(),
        "a failed atomic write leaves no temporary config behind"
    );
}

/// The account's answer to "which harness does a new agent open on" is a
/// bridge setting, so every device gets the same answer. A bridge nobody
/// has configured answers with the default, and still answers the older
/// keys a step-13 client reads.
#[test]
fn settings_report_the_default_harness_and_the_compat_modes() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let settings = state.handle(req("settings.get", json!({})))["result"].clone();
    assert_eq!(settings["default_harness"], "claude_adk");
    assert_eq!(settings["claude_mode"], "headless");
    assert_eq!(settings["codex_mode"], "headless");
    assert_eq!(settings["agent_modes"]["claude"], "headless");
    assert_eq!(settings["agent_modes"]["codex"], "tui");
}

#[test]
fn independent_agent_modes_merge_persist_and_ignore_fallback_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let config = tmp.path().join("config.json");
    let mut state = AppState::new(
        repo.clone(),
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();

    let claude = state.handle(req(
        "settings.set",
        json!({ "agent_modes": { "claude": "tui" } }),
    ));
    assert_eq!(claude["result"]["agent_modes"]["claude"], "tui");
    assert_eq!(claude["result"]["agent_modes"]["codex"], "tui");
    let codex = state.handle(req(
        "settings.set",
        json!({ "agent_modes": { "codex": "headless" } }),
    ));
    assert_eq!(codex["result"]["agent_modes"]["claude"], "tui");
    assert_eq!(codex["result"]["agent_modes"]["codex"], "headless");

    let fallback = state.handle(req("settings.set", json!({ "default_harness": "pi" })));
    assert_eq!(fallback["result"]["default_harness"], "pi");
    assert_eq!(
        fallback["result"]["agent_modes"],
        codex["result"]["agent_modes"]
    );
    for legacy_patch in [
        json!({ "claude_mode": "headless" }),
        json!({ "codex_mode": "tui" }),
    ] {
        let legacy = state.handle(req("settings.set", legacy_patch));
        assert_eq!(
            legacy["result"]["agent_modes"], codex["result"]["agent_modes"],
            "legacy aliases must not overwrite independent preferences"
        );
    }

    let mut reloaded = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let settings = reloaded.handle(req("settings.get", json!({})))["result"].clone();
    assert_eq!(settings["agent_modes"]["claude"], "tui");
    assert_eq!(settings["agent_modes"]["codex"], "headless");
    let persisted: Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    assert_eq!(persisted["agent_modes"], settings["agent_modes"]);
}

#[test]
fn legacy_config_derives_modes_once_and_all_normal_writes_materialize_them() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let config = tmp.path().join("config.json");
    std::fs::write(
        &config,
        json!({ "default_harness": "codex_app_server" }).to_string(),
    )
    .unwrap();
    let mut state = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let initial = state.handle(req("settings.get", json!({})))["result"].clone();
    assert_eq!(initial["agent_modes"]["claude"], "headless");
    assert_eq!(initial["agent_modes"]["codex"], "headless");

    let changed = state.handle(req("settings.set", json!({ "default_harness": "claude" })));
    assert_eq!(changed["result"]["agent_modes"], initial["agent_modes"]);
    let persisted: Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    assert_eq!(persisted["agent_modes"], initial["agent_modes"]);
}

#[test]
fn malformed_agent_mode_patches_are_atomic() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let before = state.handle(req("settings.get", json!({})))["result"].clone();
    for invalid in [
        json!(null),
        json!({}),
        json!({ "gemini": "tui" }),
        json!({ "claude": "future" }),
        json!({ "codex": 1 }),
    ] {
        let untouched = dir.path().join("must-not-exist");
        let response = state.handle(req(
            "settings.set",
            json!({ "projects_dir": untouched, "default_harness": "pi", "agent_modes": invalid }),
        ));
        assert_eq!(response["ok"], false, "{response:?}");
        assert_eq!(
            state.handle(req("settings.get", json!({})))["result"],
            before
        );
        assert!(
            !untouched.exists(),
            "validation must precede filesystem mutation"
        );
    }
}

#[test]
fn models_list_serves_independent_agent_modes() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    state.handle(req(
        "settings.set",
        json!({ "agent_modes": { "claude": "tui", "codex": "headless" } }),
    ));
    let models = state.handle(req("models.list", json!({})));
    assert_eq!(models["result"]["agent_modes"]["claude"], "tui");
    assert_eq!(models["result"]["agent_modes"]["codex"], "headless");
}

/// The default outlives the process it was chosen in — it is an account
/// setting, not a session's mood — and choosing it moves nothing else.
#[test]
fn a_chosen_default_harness_survives_a_reload_and_leaves_the_projects_dir_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let cfg = tmp.path().join("config.json");
    {
        let mut state = AppState::new(
            repo.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&cfg)
        .unwrap();
        state.handle(req(
            "settings.set",
            json!({ "projects_dir": tmp.path().join("myprojects").to_str().unwrap() }),
        ));
        let set = state.handle(req("settings.set", json!({ "default_harness": "claude" })));
        assert_eq!(set["ok"], true, "{set:?}");
        assert_eq!(set["result"]["default_harness"], "claude");
        assert_eq!(set["result"]["claude_mode"], "tui");
    }
    let mut reloaded = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&cfg)
    .unwrap();
    let settings = reloaded.handle(req("settings.get", json!({})))["result"].clone();
    assert_eq!(settings["default_harness"], "claude");
    assert_eq!(settings["claude_mode"], "tui");
    assert!(
        settings["projects_dir"]
            .as_str()
            .unwrap()
            .contains("myprojects"),
        "setting one field moves no other: {settings:?}"
    );
}

#[test]
fn pi_round_trips_as_the_persisted_coding_default() {
    let temp = tempfile::tempdir().unwrap();
    let (_repo_dir, repo) = init_repo();
    let config = temp.path().join("config.json");
    let mut state = AppState::new(
        repo.clone(),
        temp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(&config)
    .unwrap();
    let set = state.handle(req("settings.set", json!({ "default_harness": "pi" })));
    assert_eq!(set["result"]["default_harness"], "pi");
    let mut reloaded = AppState::new(
        repo,
        temp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    )
    .with_config(config)
    .unwrap();
    assert_eq!(
        reloaded.handle(req("settings.get", json!({})))["result"]["default_harness"],
        "pi"
    );
}

/// A harness the bridge cannot run is refused, and a refusal applies
/// nothing: the settings are exactly what they were.
#[test]
fn an_unknown_default_harness_is_refused_and_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let before = state.handle(req("settings.get", json!({})))["result"].clone();
    let refused = state.handle(req(
        "settings.set",
        json!({
            "projects_dir": tmp.path().join("elsewhere").to_str().unwrap(),
            "default_harness": "telepathy",
        }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"].as_str().unwrap(),
        "unknown default_harness \"telepathy\" (expected \"claude_adk\", \"claude\", \
         \"codex\", \"codex_app_server\" or \"pi\")"
    );
    assert_eq!(
        state.handle(req("settings.get", json!({})))["result"],
        before,
        "a refused set leaves every field where it was"
    );
}

/// Step 13's `claude_mode` said the same thing in an older vocabulary, so
/// a client that still speaks it still lands — and a refusal it would have
/// got, it still gets.
#[test]
fn a_step_13_client_still_sets_the_default_harness_through_claude_mode() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let set = state.handle(req("settings.set", json!({ "claude_mode": "tui" })));
    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(set["result"]["default_harness"], "claude");
    assert_eq!(state.default_harness, AgentProvider::Claude);

    let back = state.handle(req("settings.set", json!({ "claude_mode": "headless" })));
    assert_eq!(back["result"]["default_harness"], "claude_adk");

    let refused = state.handle(req("settings.set", json!({ "claude_mode": "telepathy" })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("unknown claude_mode"),
        "{refused:?}"
    );
}

/// One setting, two words for it: a client mid-upgrade sends both, and the
/// word this bridge writes back is the one it keeps.
#[test]
fn a_client_that_sends_both_harness_words_is_read_by_the_newer_one() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );

    let set = state.handle(req(
        "settings.set",
        json!({ "claude_mode": "tui", "default_harness": "codex" }),
    ));

    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(set["result"]["default_harness"], "codex", "{set:?}");
    assert_eq!(state.default_harness, AgentProvider::Codex);

    let refused = state.handle(req(
        "settings.set",
        json!({ "claude_mode": "telepathy", "default_harness": "codex" }),
    ));
    assert_eq!(
        refused["ok"], false,
        "and a word it cannot read is still refused, whichever key carries it: {refused:?}"
    );
}

/// A bridge upgraded in place keeps the provider its human chose, with no
/// migration step: the old key is read when the new one is absent, and
/// never written again.
#[test]
fn a_config_holding_only_the_old_key_loads_the_harness_it_named() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let load = |cfg: &std::path::Path| {
        let mut state = AppState::new(
            repo.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(cfg)
        .unwrap();
        state.handle(req("settings.get", json!({})))["result"].clone()
    };

    let old = tmp.path().join("old.json");
    std::fs::write(&old, json!({ "claude_mode": "tui" }).to_string()).unwrap();
    assert_eq!(load(&old)["default_harness"], "claude");

    let both = tmp.path().join("both.json");
    std::fs::write(
        &both,
        json!({ "claude_mode": "tui", "default_harness": "codex" }).to_string(),
    )
    .unwrap();
    assert_eq!(
        load(&both)["default_harness"],
        "codex",
        "the new key is the one that is written, so it is the one believed"
    );
}

/// The old Codex mode field remains an alias for the concrete provider.
#[test]
fn codex_mode_selects_each_concrete_codex_carrier() {
    let (dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        dir.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let accepted = state.handle(req("settings.set", json!({ "codex_mode": "tui" })));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
    assert_eq!(accepted["result"]["codex_mode"], "tui");
    assert_eq!(accepted["result"]["default_harness"], "codex");

    let headless = state.handle(req("settings.set", json!({ "codex_mode": "headless" })));
    assert_eq!(headless["ok"], true, "{headless:?}");
    assert_eq!(headless["result"]["codex_mode"], "headless");
    assert_eq!(headless["result"]["default_harness"], "codex_app_server");

    let refused = state.handle(req("settings.set", json!({ "codex_mode": "future" })));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"].as_str().unwrap(),
        "unknown codex_mode \"future\" (expected \"headless\" or \"tui\")"
    );
}

/// The fields are additive: a client written before the modes existed
/// still sets the projects folder the way it always has. A set that names
/// nothing the bridge knows is a no-op dressed as a mutation, and says so.
#[test]
fn settings_set_stays_field_wise_for_an_old_client_and_refuses_an_empty_set() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let mut state = AppState::new(
        repo,
        tmp.path().join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let old_client = state.handle(req(
        "settings.set",
        json!({ "projects_dir": tmp.path().join("projects").to_str().unwrap() }),
    ));
    assert_eq!(old_client["ok"], true, "{old_client:?}");
    assert!(old_client["result"]["projects_dir"]
        .as_str()
        .unwrap()
        .contains("projects"));
    assert_eq!(
        old_client["result"]["claude_mode"], "headless",
        "an old client's set leaves the mode at the account's answer"
    );

    let empty = state.handle(req("settings.set", json!({})));
    assert_eq!(empty["ok"], false, "{empty:?}");
    assert_eq!(
        empty["error"].as_str().unwrap(),
        "settings.set: nothing to set"
    );
}

/// What a project agent starts on is a DEVICE setting, held beside the default
/// harness: the browser asks nobody at first use, because the answer is already
/// on the machine that will run the agent.
///
/// Every field is optional and set on its own — a `null` clears the one it
/// names, a `null` object clears all three — and what the device holds outlives
/// the process, the way `default_harness` does.
#[test]
fn the_project_agent_choice_round_trips_field_by_field_and_outlives_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.json");
    let new_app = || {
        AppState::new_unrooted(
            directory.path().join("worktrees"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&config_path)
        .unwrap()
    };

    let mut app = new_app();
    // Nothing chosen is nothing said: an absent provider means the device's own
    // default harness, so a bridge nobody has asked answers an empty object
    // rather than guessing one.
    assert_eq!(
        app.handle(req("settings.get", json!({})))["result"]["project_agent"],
        json!({})
    );

    let set = app.handle(req(
        "settings.set",
        json!({ "project_agent": {
            "provider": "codex", "model": "gpt-5.6-sol", "effort": "high"
        } }),
    ));
    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(
        set["result"]["project_agent"],
        json!({ "provider": "codex", "model": "gpt-5.6-sol", "effort": "high" }),
        "the set answers with what the device now holds"
    );

    // Partial: a field the object leaves out stands, and a `null` clears the
    // one it names.
    let narrowed = app.handle(req(
        "settings.set",
        json!({ "project_agent": { "effort": null } }),
    ));
    assert_eq!(narrowed["ok"], true, "{narrowed:?}");
    assert_eq!(
        narrowed["result"]["project_agent"],
        json!({ "provider": "codex", "model": "gpt-5.6-sol" })
    );

    let mut restarted = new_app();
    assert_eq!(
        restarted.handle(req("settings.get", json!({})))["result"]["project_agent"],
        json!({ "provider": "codex", "model": "gpt-5.6-sol" }),
        "the config carries the choice across a restart"
    );

    // A model the chosen harness cannot be asked for is refused, and the device
    // keeps what it had.
    let refused = restarted.handle(req(
        "settings.set",
        json!({ "project_agent": { "effort": "not-a-level" } }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        restarted.handle(req("settings.get", json!({})))["result"]["project_agent"],
        json!({ "provider": "codex", "model": "gpt-5.6-sol" })
    );

    let cleared = restarted.handle(req("settings.set", json!({ "project_agent": null })));
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    assert_eq!(cleared["result"]["project_agent"], json!({}));
}

/// How big an agent's context may grow before its next turn is preceded by a
/// compaction: 200k until the device says otherwise, 0 for never, and what it
/// says outlives a restart.
#[test]
fn compact_above_tokens_defaults_sets_and_survives_a_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let (_dir, repo) = init_repo();
    let cfg = tmp.path().join("config.json");
    let open = || {
        AppState::new(
            repo.clone(),
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&cfg)
        .unwrap()
    };
    {
        let mut state = open();
        let settings = state.handle(req("settings.get", json!({})))["result"].clone();
        assert_eq!(settings["compact_above_tokens"], 200_000, "{settings:?}");

        let refused = state.handle(req(
            "settings.set",
            json!({ "compact_above_tokens": "lots" }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");

        let set = state.handle(req(
            "settings.set",
            json!({ "compact_above_tokens": 150_000 }),
        ));
        assert_eq!(set["ok"], true, "{set:?}");
        assert_eq!(set["result"]["compact_above_tokens"], 150_000);
    }
    let mut reloaded = open();
    let settings = reloaded.handle(req("settings.get", json!({})))["result"].clone();
    assert_eq!(settings["compact_above_tokens"], 150_000, "{settings:?}");

    let off = reloaded.handle(req("settings.set", json!({ "compact_above_tokens": 0 })));
    assert_eq!(off["result"]["compact_above_tokens"], 0, "0 turns it off");
}
