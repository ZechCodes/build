use super::*;

#[test]
fn dev_service_environment_pins_all_resolved_paths_and_keeps_no_private_keys() {
    let home = tempfile::tempdir().unwrap();
    let cfg = config::resolve(
        |key| (key == "BRIDGE_CHANNEL").then(|| "dev".into()),
        home.path(),
    )
    .unwrap();
    let inherited = vec![
        ("BRIDGE_CHANNEL".into(), "prod".into()),
        ("BRIDGE_CONFIG".into(), "~/wrong/config.json".into()),
        ("BRIDGE_IDENTITY_PRIV".into(), "private-fixture".into()),
    ];
    let vars: std::collections::BTreeMap<_, _> = service_environment(inherited, &cfg, || None)
        .into_iter()
        .collect();
    assert_eq!(vars["BRIDGE_CHANNEL"], "dev");
    assert_eq!(vars["BRIDGE_WEB_URL"], cfg.web_url);
    for (name, path) in [
        ("BRIDGE_IDENTITY_FILE", &cfg.identity_file),
        ("BRIDGE_CONFIG", &cfg.config_path),
        ("BRIDGE_TASKS_DIR", &cfg.tasks_dir),
        ("BRIDGE_PROJECTS_DIR", &cfg.projects_dir),
        ("BRIDGE_WORKTREES", &cfg.worktrees),
        ("BRIDGE_MCP_SOCKET", &cfg.mcp_socket),
    ] {
        assert_eq!(vars[name], path.to_string_lossy());
    }
    assert!(!vars.contains_key("BRIDGE_IDENTITY_PRIV"));
}

#[test]
fn dev_never_looks_up_provisioned_identity_keys() {
    assert!(provisioned_identity_from(config::Channel::Dev, |_| panic!(
        "dev read provisioned keys"
    ))
    .is_none());
    let loaded =
        provisioned_identity_from(config::Channel::Prod, |key| Some(key.to_string())).unwrap();
    assert_eq!(loaded.identity.device_id, "BRIDGE_DEVICE_ID");
    assert_eq!(
        loaded.identity.identity_private_key_b64,
        "BRIDGE_IDENTITY_PRIV"
    );
}
