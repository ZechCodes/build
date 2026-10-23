//! Channel isolation uses injected config and cargo-built CLI commands only.
use build_bridge::config;
use build_bridge::store::Store;
use std::path::Path;
use std::process::{Command, Output};

fn cli(home: &Path, channel: Option<&str>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_build-bridge"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin");
    if let Some(channel) = channel {
        command.env("BRIDGE_CHANNEL", channel);
    }
    command.args(args).output().unwrap()
}

#[test]
fn dev_defaults_use_separate_identity_worktrees_socket_and_hosts() {
    let home = tempfile::tempdir().unwrap();
    let cfg = config::resolve(
        |key| (key == "BRIDGE_CHANNEL").then(|| "dev".into()),
        home.path(),
    )
    .unwrap();
    let root = home.path().join(".local/state/build-dev");
    assert_eq!(cfg.api_url, "https://dev.getbuild.ing");
    assert_eq!(cfg.web_url, "https://dev.getbuild.ing");
    assert_eq!(cfg.relay_url, "wss://relay-dev.getbuild.ing");
    assert_eq!(cfg.identity_file, root.join("identity.json"));
    assert_eq!(cfg.worktrees, root.join("worktrees"));
    assert_eq!(cfg.mcp_socket, root.join("mcp.sock"));
    assert!(!home.path().join(".build").exists());
}

#[test]
fn unknown_channel_is_refused_before_dispatch_or_state_access() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--version"][..],
        &["backup", "unused"][..],
        &["update-helper", "unused"][..],
    ] {
        let output = cli(home.path(), Some("preview"), args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("BRIDGE_CHANNEL"));
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
    }
}

#[test]
fn dev_backup_selects_dev_store_and_preserves_production() {
    let home = tempfile::tempdir().unwrap();
    let dev = home.path().join(".local/state/build-dev/tasks");
    let prod = home.path().join(".build/tasks");
    let dev_store = Store::new(&dev).unwrap();
    let prod_store = Store::new(&prod).unwrap();
    let dev_db = dev.join("build.db");
    let prod_db = prod.join("build.db");
    let marker = |path: &Path, value: &str| {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE channel_marker (value TEXT);")
            .unwrap();
        db.execute("INSERT INTO channel_marker VALUES (?1)", [value])
            .unwrap();
    };
    marker(&dev_db, "dev");
    marker(&prod_db, "prod");
    let before = std::fs::read(&prod_db).unwrap();
    let backup = home.path().join("backup.db");
    let output = cli(
        home.path(),
        Some("dev"),
        &["backup", backup.to_str().unwrap()],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db = rusqlite::Connection::open(&backup).unwrap();
    let selected: String = db
        .query_row("SELECT value FROM channel_marker", [], |row| row.get(0))
        .unwrap();
    assert_eq!(selected, "dev");
    assert_eq!(std::fs::read(prod_db).unwrap(), before);
    drop((dev_store, prod_store));
}

#[test]
fn dev_helper_is_refused_before_reading_job_or_creating_state() {
    let home = tempfile::tempdir().unwrap();
    let output = cli(home.path(), Some("dev"), &["update-helper", "/missing-job"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("disabled for BRIDGE_CHANNEL=dev"));
    assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
}

#[test]
fn dev_and_prod_config_paths_and_override_precedence_are_explicit() {
    let home = tempfile::tempdir().unwrap();
    for channel in ["prod", "dev"] {
        let defaults = config::resolve(
            |key| (key == "BRIDGE_CHANNEL").then(|| channel.into()),
            home.path(),
        )
        .unwrap();
        let root = home.path().join(if channel == "dev" {
            ".local/state/build-dev"
        } else {
            ".build"
        });
        assert_eq!(defaults.state_root, root);
        assert_eq!(defaults.tasks_dir, root.join("tasks"));
        assert_eq!(defaults.config_path, root.join("config.json"));
        assert_eq!(defaults.projects_dir, root.join("projects"));
        assert_eq!(defaults.log_dir, root.join("log"));
        let cfg = config::resolve(
            |key| {
                Some(
                    match key {
                        "BRIDGE_CHANNEL" => channel,
                        "BRIDGE_API_URL" => "https://api.example.test",
                        "BRIDGE_WEB_URL" => "https://web.example.test",
                        "BRIDGE_RELAY_URL" => "wss://relay.example.test",
                        "BRIDGE_IDENTITY_FILE" => "~/other/identity.json",
                        "BRIDGE_TASKS_DIR" => "~/other/tasks",
                        "BRIDGE_CONFIG" => "~/other/config.json",
                        "BRIDGE_PROJECTS_DIR" => "~/other/projects",
                        "BRIDGE_WORKTREES" => "~/other/worktrees",
                        "BRIDGE_MCP_SOCKET" => "~/other/mcp.sock",
                        _ => return None,
                    }
                    .into(),
                )
            },
            home.path(),
        )
        .unwrap();
        assert_eq!(cfg.api_url, "https://api.example.test");
        assert_eq!(cfg.web_url, "https://web.example.test");
        assert_eq!(cfg.relay_url, "wss://relay.example.test");
        for (actual, leaf) in [
            (&cfg.identity_file, "identity.json"),
            (&cfg.tasks_dir, "tasks"),
            (&cfg.config_path, "config.json"),
            (&cfg.projects_dir, "projects"),
            (&cfg.worktrees, "worktrees"),
            (&cfg.mcp_socket, "mcp.sock"),
        ] {
            assert_eq!(*actual, home.path().join("other").join(leaf));
        }
    }
}

#[test]
fn config_rejects_unknown_channels_and_keeps_web_override_fallback() {
    let home = tempfile::tempdir().unwrap();
    for channel in ["", "DEV", "preview"] {
        let result = config::resolve(
            |key| (key == "BRIDGE_CHANNEL").then(|| channel.into()),
            home.path(),
        );
        assert!(result.unwrap_err().contains("BRIDGE_CHANNEL"));
    }
    let cfg = config::resolve(
        |key| match key {
            "BRIDGE_CHANNEL" => Some("dev".into()),
            "BRIDGE_API_URL" => Some("https://custom.example.test".into()),
            _ => None,
        },
        home.path(),
    )
    .unwrap();
    assert_eq!(cfg.web_url, "https://custom.example.test");
}

#[test]
fn dev_mints_an_identity_distinct_from_existing_production_keys() {
    let home = tempfile::tempdir().unwrap();
    let prod = config::resolve(|_| None, home.path()).unwrap();
    let dev = config::resolve(
        |key| (key == "BRIDGE_CHANNEL").then(|| "dev".into()),
        home.path(),
    )
    .unwrap();
    let production = build_bridge::identity::load_or_generate(&prod.identity_file, "prod").unwrap();
    let before = std::fs::read(&prod.identity_file).unwrap();
    let development = build_bridge::identity::load_or_generate(&dev.identity_file, "dev").unwrap();
    assert_ne!(production.device_id, development.device_id);
    assert_ne!(
        production.transport.public_key_b64,
        development.transport.public_key_b64
    );
    assert_eq!(std::fs::read(&prod.identity_file).unwrap(), before);
}

fn assert_override_is_isolated(key: &str) {
    let home = tempfile::tempdir().unwrap();
    for value in ["~/.build", "~/.build/missing/leaf", "~/safe/../.build/leaf"] {
        for channel in ["dev", "prod"] {
            let result = config::resolve(
                |name| match name {
                    "BRIDGE_CHANNEL" => Some(channel.into()),
                    name if name == key => Some(value.into()),
                    _ => None,
                },
                home.path(),
            );
            if channel == "dev" {
                let error = result.expect_err(key);
                assert!(error.contains(key), "{error}");
                assert!(error.contains("production state root"), "{error}");
            } else {
                result.expect("production overrides keep their existing behavior");
            }
        }
    }
    assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
}

#[test]
fn dev_refuses_production_identity_override() {
    assert_override_is_isolated("BRIDGE_IDENTITY_FILE");
}

#[test]
fn dev_refuses_production_config_override() {
    assert_override_is_isolated("BRIDGE_CONFIG");
}

#[test]
fn dev_refuses_production_tasks_override() {
    assert_override_is_isolated("BRIDGE_TASKS_DIR");
}

#[test]
fn dev_refuses_production_socket_override() {
    assert_override_is_isolated("BRIDGE_MCP_SOCKET");
}

#[test]
fn dev_refuses_production_worktrees_override() {
    assert_override_is_isolated("BRIDGE_WORKTREES");
}

#[test]
fn dev_refuses_production_projects_dir_override() {
    assert_override_is_isolated("BRIDGE_PROJECTS_DIR");
}

#[test]
fn dev_refuses_production_repo_override() {
    assert_override_is_isolated("BRIDGE_REPO");
}

#[test]
fn dev_refuses_production_extra_projects_override() {
    assert_override_is_isolated("BRIDGE_PROJECTS");
    let home = tempfile::tempdir().unwrap();
    let result = config::resolve(
        |key| match key {
            "BRIDGE_CHANNEL" => Some("dev".into()),
            "BRIDGE_PROJECTS" => Some(format!(
                "{}, {}=main",
                home.path().join("safe").display(),
                home.path().join(".build/missing/repo").display()
            )),
            _ => None,
        },
        home.path(),
    );
    assert!(result.unwrap_err().contains("BRIDGE_PROJECTS"));
}

#[cfg(unix)]
#[test]
fn dev_refuses_symlink_aliases_with_missing_leaves_and_parent_traversal() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let prod = home.path().join(".build");
    std::fs::create_dir_all(prod.join("nested")).unwrap();
    symlink(".build/nested", home.path().join("alias")).unwrap();
    symlink(".build/not-created", home.path().join("dangling")).unwrap();
    for relative in ["alias/new/leaf", "alias/../leaf", "dangling/leaf"] {
        let result = config::resolve(
            |key| match key {
                "BRIDGE_CHANNEL" => Some("dev".into()),
                "BRIDGE_IDENTITY_FILE" => Some(home.path().join(relative).display().to_string()),
                _ => None,
            },
            home.path(),
        );
        assert!(result.unwrap_err().contains("production state root"));
    }
}

#[cfg(unix)]
#[test]
fn dev_refuses_a_symlinked_production_root_and_default_state_or_log_alias() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    let prod = home.path().join("prod-target");
    std::fs::create_dir_all(&prod).unwrap();
    symlink(&prod, home.path().join(".build")).unwrap();
    let result = config::resolve(
        |key| match key {
            "BRIDGE_CHANNEL" => Some("dev".into()),
            "BRIDGE_TASKS_DIR" => Some(prod.join("missing/tasks").display().to_string()),
            _ => None,
        },
        home.path(),
    );
    assert!(result.unwrap_err().contains("BRIDGE_TASKS_DIR"));
    let state = home.path().join(".local/state");
    std::fs::create_dir_all(&state).unwrap();
    symlink(&prod, state.join("build-dev")).unwrap();
    let resolve_dev = || {
        config::resolve(
            |key| (key == "BRIDGE_CHANNEL").then(|| "dev".into()),
            home.path(),
        )
    };
    assert!(resolve_dev().unwrap_err().contains("state_root"));
    std::fs::remove_file(state.join("build-dev")).unwrap();
    std::fs::create_dir(state.join("build-dev")).unwrap();
    symlink(&prod, state.join("build-dev/log")).unwrap();
    assert!(resolve_dev().unwrap_err().contains("log_dir"));
}

#[test]
fn dev_accepts_distinct_paths_with_a_production_name_prefix() {
    let home = tempfile::tempdir().unwrap();
    let cfg = config::resolve(
        |key| match key {
            "BRIDGE_CHANNEL" => Some("dev".into()),
            "BRIDGE_TASKS_DIR" => Some("~/.build-dev/missing/tasks".into()),
            _ => None,
        },
        home.path(),
    )
    .unwrap();
    assert_eq!(cfg.tasks_dir, home.path().join(".build-dev/missing/tasks"));
}

#[test]
fn dev_cli_refuses_inherited_production_paths_before_creating_state() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        vec!["--version"],
        vec!["mcp", "--task", "unused"],
        vec!["backup", "backup.db"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("BRIDGE_CHANNEL", "dev")
            .env("BRIDGE_CONFIG", ".build/config.json")
            .current_dir(home.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("BRIDGE_CONFIG"), "{error}");
        assert!(error.contains("production state root"), "{error}");
        assert!(std::fs::read_dir(home.path()).unwrap().next().is_none());
    }
}

#[cfg(unix)]
#[test]
fn dev_rejects_unresolvable_symlink_loops() {
    use std::os::unix::fs::symlink;
    let home = tempfile::tempdir().unwrap();
    symlink("loop", home.path().join("loop")).unwrap();
    let result = config::resolve(
        |key| match key {
            "BRIDGE_CHANNEL" => Some("dev".into()),
            "BRIDGE_TASKS_DIR" => Some(home.path().join("loop/tasks").display().to_string()),
            _ => None,
        },
        home.path(),
    );
    assert!(result.unwrap_err().contains("BRIDGE_TASKS_DIR"));
}
