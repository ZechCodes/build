//! The only secret authority transport into an agent is its launch environment.
use crate::pty::HarnessSpec;

pub const SECRET_ENVIRONMENT: [&str; 6] = [
    "BRIDGE_SECRET_SOCKET",
    "BRIDGE_SECRET_CHANNEL",
    "BRIDGE_SECRET_AGENT",
    "BRIDGE_SECRET_OWNER",
    "BRIDGE_SECRET_TOKEN",
    "BRIDGE_SECRET_BRIDGE",
];

// Deliberately no Debug or Serialize: this owns an unlogged capability.
#[derive(Clone)]
pub struct SecretLaunchEnvironment {
    socket: String,
    channel: String,
    agent: String,
    owner: String,
    token: String,
    bridge: String,
}

impl SecretLaunchEnvironment {
    pub(crate) fn new(
        socket: String,
        channel: String,
        agent: String,
        owner: String,
        token: String,
        bridge: String,
    ) -> Self {
        Self {
            socket,
            channel,
            agent,
            owner,
            token,
            bridge,
        }
    }
}

pub(crate) fn inherited_keys() -> Vec<std::ffi::OsString> {
    let mut keys: Vec<_> = SECRET_ENVIRONMENT
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect();
    keys.extend(std::env::vars_os().filter_map(|(name, _)| {
        name.as_encoded_bytes()
            .starts_with(b"BRIDGE_SECRET_")
            .then_some(name)
    }));
    keys
}

/// Invoked after every provider builder, including the scripted QA harness.
/// A missing launch context grants no capability, including capture routers.
pub(crate) fn apply(mut spec: HarnessSpec, launch: Option<SecretLaunchEnvironment>) -> HarnessSpec {
    spec.env
        .retain(|(key, _)| !key.starts_with("BRIDGE_SECRET_"));
    if let Some(launch) = launch {
        for (key, value) in SECRET_ENVIRONMENT.into_iter().zip([
            launch.socket,
            launch.channel,
            launch.agent,
            launch.owner,
            launch.token,
            launch.bridge,
        ]) {
            spec = spec.env(key, value);
        }
    }
    spec
}

/// Every agent can find the installed runtime facade, independently of its checkout.
pub(crate) fn runtime_path(
    mut spec: HarnessSpec,
    bridge: &std::path::Path,
) -> Result<HarnessSpec, crate::harness::HarnessError> {
    if !bridge.is_absolute() {
        return Err(crate::harness::HarnessError::Setup(
            "runtime bridge path is not absolute".into(),
        ));
    }
    let Some(bin) = crate::runtime_launcher::bin_directory(bridge) else {
        return Ok(spec);
    };
    let inherited = spec
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| std::ffi::OsString::from(value))
        .or_else(|| std::env::var_os("PATH"));
    let mut paths = vec![bin];
    if let Some(inherited) = inherited {
        paths.extend(std::env::split_paths(&inherited));
    }
    let path = std::env::join_paths(paths)
        .ok()
        .and_then(|path| path.into_string().ok())
        .ok_or_else(|| crate::harness::HarnessError::Setup("runtime PATH is invalid".into()))?;
    spec.env.retain(|(key, _)| key != "PATH");
    Ok(spec.env("PATH", path))
}

/// Preserve Codex's effective user policy, adding only this launch's six names.
pub(crate) fn codex_thread_config(
    spec: &HarnessSpec,
    config: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let launch = secret_values(spec)?;
    let mut config = config.unwrap_or_else(|| serde_json::json!({}));
    let policy = config
        .get("shell_environment_policy")
        .filter(|value| !value.is_null())
        .cloned()
        .or_else(|| spec.codex_shell_policy.as_deref().cloned())
        .unwrap_or_else(|| serde_json::json!({}));
    let policy = extend_codex_policy(policy, launch)?;
    config
        .as_object_mut()?
        .insert("shell_environment_policy".into(), policy);
    Some(config)
}

pub(crate) fn secret_values(
    spec: &HarnessSpec,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    SECRET_ENVIRONMENT
        .into_iter()
        .map(|name| {
            let value = spec
                .env
                .iter()
                .rev()
                .find(|(key, _)| key == name)?
                .1
                .clone();
            Some((name.into(), value.into()))
        })
        .collect()
}

fn extend_codex_policy(
    policy: serde_json::Value,
    launch: serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut policy = policy.as_object()?.clone();
    policy.retain(|_, value| !value.is_null());
    policy
        .entry("ignore_default_excludes")
        .or_insert(false.into());
    if policy
        .get("filters")
        .is_some_and(serde_json::Value::is_object)
    {
        extend_modern_filters(&mut policy)?;
    } else {
        extend_legacy_filters(&mut policy)?;
    }
    let set = policy
        .entry("set")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()?;
    // Explicit user settings remain theirs; replace only this launch's names.
    set.extend(launch);
    Some(policy.into())
}

fn extend_legacy_filters(policy: &mut serde_json::Map<String, serde_json::Value>) -> Option<()> {
    let excludes = policy
        .entry("exclude")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()?;
    append_pattern(excludes, "BRIDGE_*");
    if let Some(includes) = policy.get_mut("include_only") {
        let includes = includes.as_array_mut()?;
        if !includes.is_empty() {
            for name in SECRET_ENVIRONMENT {
                append_pattern(includes, name);
            }
        }
    }
    Some(())
}

fn append_pattern(patterns: &mut Vec<serde_json::Value>, name: &str) {
    if !patterns.iter().any(|value| {
        value
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(name))
    }) {
        patterns.push(name.into());
    }
}

fn extend_modern_filters(policy: &mut serde_json::Map<String, serde_json::Value>) -> Option<()> {
    let mut filters = policy.remove("filters")?.as_object()?.clone();
    merge_legacy_patterns(&mut filters, policy.remove("exclude"), "exclude")?;
    merge_legacy_patterns(&mut filters, policy.remove("include_only"), "include")?;
    let allowlist = filters
        .values()
        .any(|value| value.as_str() == Some("include"));
    insert_filter(&mut filters, "BRIDGE_*", "exclude");
    if allowlist {
        for name in SECRET_ENVIRONMENT {
            insert_filter(&mut filters, name, "include");
        }
    }
    policy.insert("filters".into(), filters.into());
    Some(())
}

fn merge_legacy_patterns(
    filters: &mut serde_json::Map<String, serde_json::Value>,
    patterns: Option<serde_json::Value>,
    action: &str,
) -> Option<()> {
    if let Some(patterns) = patterns {
        for pattern in patterns.as_array()? {
            let pattern = pattern.as_str()?;
            if !filters
                .keys()
                .any(|name| name.eq_ignore_ascii_case(pattern))
            {
                filters.insert(pattern.into(), action.into());
            }
        }
    }
    Some(())
}

fn insert_filter(
    filters: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    action: &str,
) {
    filters.retain(|key, _| !key.eq_ignore_ascii_case(name));
    filters.insert(name.into(), action.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_secret_overlay_preserves_legacy_user_shell_policy() {
        let spec = runtime_path(
            apply(
                HarnessSpec::new("codex"),
                Some(SecretLaunchEnvironment::new(
                    "/fixture.sock".into(),
                    "dev".into(),
                    "agent".into(),
                    "owner".into(),
                    "own-capability".into(),
                    "/installed/bin/build-bridge".into(),
                )),
            ),
            std::path::Path::new("/installed/bin/build-bridge"),
        )
        .unwrap();
        let config = codex_thread_config(&spec, Some(serde_json::json!({"shell_environment_policy": {
            "inherit": "core", "ignore_default_excludes": false, "exclude": ["CORP_*"],
                "include_only": ["PATH", "HOME", "LANG"], "set": {"CUSTOM_POLICY": "kept", "BRIDGE_CUSTOM": "user-choice", "PATH": "/corporate/bin"}
        }}))).unwrap();
        let policy = &config["shell_environment_policy"];
        assert_eq!(policy["inherit"], "core");
        assert_eq!(policy["exclude"], serde_json::json!(["CORP_*", "BRIDGE_*"]));
        assert_eq!(policy["set"]["CUSTOM_POLICY"], "kept");
        assert_eq!(policy["set"]["BRIDGE_CUSTOM"], "user-choice");
        assert_eq!(policy["set"]["PATH"], "/corporate/bin");
        let allowed = policy["include_only"].as_array().unwrap();
        for name in SECRET_ENVIRONMENT
            .into_iter()
            .chain(["PATH", "HOME", "LANG"])
        {
            assert!(allowed.contains(&serde_json::json!(name)), "{name}");
        }
    }

    #[test]
    fn codex_secret_overlay_preserves_modern_case_insensitive_filters() {
        let spec = runtime_path(
            apply(
                HarnessSpec::new("codex"),
                Some(SecretLaunchEnvironment::new(
                    "/fixture.sock".into(),
                    "dev".into(),
                    "agent".into(),
                    "owner".into(),
                    "own-capability".into(),
                    "/installed/bin/build-bridge".into(),
                )),
            ),
            std::path::Path::new("/installed/bin/build-bridge"),
        )
        .unwrap();
        let config = codex_thread_config(&spec, Some(serde_json::json!({"shell_environment_policy": {
            "inherit": "core", "ignore_default_excludes": false,
            "filters": {"PATH": "include", "HOME": "include", "corp_*": "exclude", "bridge_secret_token": "exclude"},
            "set": {"CUSTOM_POLICY": "kept"}
        }}))).unwrap();
        let policy = &config["shell_environment_policy"];
        assert_eq!(policy["inherit"], "core");
        assert!(policy.get("exclude").is_none());
        assert!(policy.get("include_only").is_none());
        assert_eq!(policy["filters"]["corp_*"], "exclude");
        assert_eq!(policy["filters"]["HOME"], "include");
        assert_eq!(policy["filters"]["BRIDGE_SECRET_TOKEN"], "include");
        assert!(policy["filters"].get("bridge_secret_token").is_none());
        assert_eq!(policy["set"]["CUSTOM_POLICY"], "kept");
    }

    #[test]
    fn codex_shell_policy_restores_only_secret_capability_and_path_after_default_filtering() {
        let spec = runtime_path(
            apply(
                HarnessSpec::new("codex"),
                Some(SecretLaunchEnvironment::new(
                    "/fixture.sock".into(),
                    "dev".into(),
                    "agent".into(),
                    "owner".into(),
                    "secret-capability".into(),
                    "/installed/bin/build-bridge".into(),
                )),
            ),
            std::path::Path::new("/installed/bin/build-bridge"),
        )
        .unwrap()
        .env("BRIDGE_MCP_TOKEN", "mcp-capability")
        .env("BRIDGE_OTHER", "unrelated-context")
        .env("AWS_SECRET_ACCESS_KEY", "unrelated-provider-key");
        let config = codex_thread_config(
            &spec,
            Some(serde_json::json!({"model_auto_compact_token_limit": 123})),
        )
        .unwrap();
        assert_eq!(config["model_auto_compact_token_limit"], 123);
        let policy = &config["shell_environment_policy"];
        assert_eq!(policy["ignore_default_excludes"], false);
        assert!(policy.get("inherit").is_none());
        assert_eq!(policy["exclude"], serde_json::json!(["BRIDGE_*"]));
        let restored = policy["set"].as_object().unwrap();
        assert_eq!(restored.len(), SECRET_ENVIRONMENT.len());
        assert!(!restored.contains_key("PATH"));
        assert_eq!(restored["BRIDGE_SECRET_TOKEN"], "secret-capability");
        for withheld in ["BRIDGE_MCP_TOKEN", "BRIDGE_OTHER", "AWS_SECRET_ACCESS_KEY"] {
            assert!(!restored.contains_key(withheld));
        }
    }

    #[test]
    fn launch_injects_dedicated_environment_and_never_argv() {
        let launch = SecretLaunchEnvironment::new(
            "/synthetic.sock".into(),
            "dev".into(),
            "agent-a".into(),
            "owner-a".into(),
            "synthetic-token".into(),
            "/installed/bin/build-bridge".into(),
        );
        let spec = apply(
            HarnessSpec::new("synthetic-harness")
                .arg("--model")
                .arg("fixture"),
            Some(launch),
        );
        assert_eq!(spec.env.len(), 6);
        assert!(!format!("{spec:?}").contains("synthetic-token"));
        assert!(spec
            .env
            .iter()
            .any(|(key, value)| key == "BRIDGE_SECRET_TOKEN" && value == "synthetic-token"));
        assert!(!spec.args.iter().any(|arg| arg.contains("synthetic-token")));
        assert!(SECRET_ENVIRONMENT
            .iter()
            .all(|key| inherited_keys().contains(&std::ffi::OsString::from(key))));
    }

    #[test]
    fn missing_context_removes_explicit_inherited_capabilities() {
        let spec = apply(
            HarnessSpec::new("synthetic-harness")
                .env("BRIDGE_SECRET_TOKEN", "parent-token")
                .env("BRIDGE_SECRET_FUTURE", "future-token")
                .env("SAFE_FIXTURE", "kept"),
            None,
        );
        assert_eq!(spec.env, vec![("SAFE_FIXTURE".into(), "kept".into())]);
    }

    #[test]
    fn runtime_bin_precedes_the_provider_path_and_never_changes_global_path() {
        let original = std::env::var_os("PATH");
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("installed bridge/bin");
        std::fs::create_dir_all(&directory).unwrap();
        let bridge = directory.join("build-bridge");
        crate::runtime_launcher::install_sibling(&bridge).unwrap();
        let spec = runtime_path(
            HarnessSpec::new("fixture").env("PATH", "/fixture/bin:/usr/bin"),
            &bridge,
        )
        .unwrap();
        assert_eq!(
            spec.env.iter().find(|(key, _)| key == "PATH").unwrap().1,
            format!(
                "{}:/fixture/bin:/usr/bin",
                directory.join(".build-runtime").display()
            )
        );
        assert_eq!(std::env::var_os("PATH"), original);
        assert!(runtime_path(
            HarnessSpec::new("fixture"),
            std::path::Path::new("relative/bridge")
        )
        .is_err());
    }

    #[test]
    fn runtime_path_never_injects_a_foreign_or_missing_launcher_directory() {
        let root = tempfile::tempdir().unwrap();
        let bridge = root.path().join("build-bridge");
        let original = HarnessSpec::new("fixture").env("PATH", "/fixture/bin");
        assert_eq!(
            runtime_path(original.clone(), &bridge).unwrap().env,
            original.env
        );
        std::fs::create_dir(root.path().join(".build-runtime")).unwrap();
        std::fs::write(
            root.path().join(".build-runtime/buildctl"),
            "foreign command",
        )
        .unwrap();
        assert_eq!(
            runtime_path(original.clone(), &bridge).unwrap().env,
            original.env
        );
    }
    #[test]
    fn every_provider_receives_only_its_dedicated_secret_launch_environment() {
        use crate::harness::{harness_for, HarnessContext};
        use crate::models::{AgentProvider, ModelChoice};
        use crate::orchestrator::SpawnOptions;
        let root = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let context =
            HarnessContext::resolved(root.path().join("mcp.sock"), root.path().to_path_buf())
                .unwrap();
        for provider in AgentProvider::ALL {
            let options = SpawnOptions {
                owner_id: "agent-fixture".into(),
                mcp_session_token: "separate-mcp-token".into(),
                cwd: repo.path().to_path_buf(),
                ..SpawnOptions::default()
            };
            let spec = harness_for(provider)
                .spec(
                    &ModelChoice {
                        provider,
                        ..ModelChoice::default()
                    },
                    &options,
                    &context,
                )
                .unwrap();
            let launch = SecretLaunchEnvironment::new(
                "/synthetic.sock".into(),
                "dev".into(),
                "agent-fixture".into(),
                "owner-fixture".into(),
                "dedicated-secret-token".into(),
                "/installed/bin/build-bridge".into(),
            );
            let spec = apply(spec, Some(launch));
            assert_eq!(
                spec.env
                    .iter()
                    .filter(|(key, _)| key.starts_with("BRIDGE_SECRET_"))
                    .count(),
                6,
                "{provider:?}"
            );
            assert!(
                spec.env
                    .iter()
                    .any(|(key, value)| key == "BRIDGE_SECRET_TOKEN"
                        && value == "dedicated-secret-token"),
                "{provider:?}"
            );
            assert!(
                !spec
                    .args
                    .iter()
                    .any(|arg| arg.contains("dedicated-secret-token")),
                "{provider:?}"
            );
        }
    }
}
