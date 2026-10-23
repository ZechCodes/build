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
