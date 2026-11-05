//! The only secret authority transport into an agent is its launch environment.
use crate::pty::HarnessSpec;

pub const SECRET_ENVIRONMENT: [&str; 5] = [
    "BRIDGE_SECRET_SOCKET",
    "BRIDGE_SECRET_CHANNEL",
    "BRIDGE_SECRET_AGENT",
    "BRIDGE_SECRET_OWNER",
    "BRIDGE_SECRET_TOKEN",
];

// Deliberately no Debug or Serialize: this owns an unlogged capability.
#[derive(Clone)]
pub struct SecretLaunchEnvironment {
    socket: String,
    channel: String,
    agent: String,
    owner: String,
    token: String,
}

impl SecretLaunchEnvironment {
    pub(crate) fn new(
        socket: String,
        channel: String,
        agent: String,
        owner: String,
        token: String,
    ) -> Self {
        Self {
            socket,
            channel,
            agent,
            owner,
            token,
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
        ]) {
            spec = spec.env(key, value);
        }
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_injects_dedicated_environment_and_never_argv() {
        let launch = SecretLaunchEnvironment::new(
            "/synthetic.sock".into(),
            "dev".into(),
            "agent-a".into(),
            "owner-a".into(),
            "synthetic-token".into(),
        );
        let spec = apply(
            HarnessSpec::new("synthetic-harness")
                .arg("--model")
                .arg("fixture"),
            Some(launch),
        );
        assert_eq!(spec.env.len(), 5);
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
            );
            let spec = apply(spec, Some(launch));
            assert_eq!(
                spec.env
                    .iter()
                    .filter(|(key, _)| key.starts_with("BRIDGE_SECRET_"))
                    .count(),
                5,
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
