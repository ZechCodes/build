//! Bridge runtime configuration: production-by-default so the end-user story is
//! "install, pair, go" with zero environment setup. Every value can still be
//! overridden through `BRIDGE_*` variables (dev stacks and compose set them all
//! explicitly).

use std::path::{Path, PathBuf};

pub const DEFAULT_RELAY_URL: &str = "wss://relay.getbuild.ing";
pub const DEFAULT_API_URL: &str = "https://getbuild.ing";
pub const DEFAULT_BASE_BRANCH: &str = "main";

/// The resolved serve/install configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub relay_url: String,
    pub api_url: String,
    /// The web app base URL shown in the pairing approval link.
    pub web_url: String,
    /// Explicitly configured default repo; `None` means projects come from the
    /// UI (clone/add) and persisted config only — never a phantom default path.
    pub repo: Option<String>,
    pub worktrees: PathBuf,
    pub base_branch: String,
    pub identity_file: PathBuf,
    pub mcp_socket: PathBuf,
}

/// Resolve config from an environment lookup and the user's home directory.
/// Pure so the default/override matrix is unit-testable.
pub fn resolve(lookup: impl Fn(&str) -> Option<String>, home: &Path) -> BridgeConfig {
    let expand = |raw: String| -> PathBuf {
        match raw.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None if raw == "~" => home.to_path_buf(),
            None => PathBuf::from(raw),
        }
    };
    let api_url = lookup("BRIDGE_API_URL").unwrap_or_else(|| DEFAULT_API_URL.to_string());
    let worktrees = lookup("BRIDGE_WORKTREES")
        .map(&expand)
        .unwrap_or_else(|| home.join(".build/worktrees"));
    let mcp_socket = lookup("BRIDGE_MCP_SOCKET")
        .map(&expand)
        .unwrap_or_else(|| worktrees.join("build-bridge-mcp.sock"));
    BridgeConfig {
        relay_url: lookup("BRIDGE_RELAY_URL").unwrap_or_else(|| DEFAULT_RELAY_URL.to_string()),
        web_url: lookup("BRIDGE_WEB_URL").unwrap_or_else(|| api_url.clone()),
        api_url,
        repo: lookup("BRIDGE_REPO"),
        worktrees,
        base_branch: lookup("BRIDGE_BASE_BRANCH")
            .unwrap_or_else(|| DEFAULT_BASE_BRANCH.to_string()),
        identity_file: lookup("BRIDGE_IDENTITY_FILE")
            .map(&expand)
            .unwrap_or_else(|| home.join(".build/identity.json")),
        mcp_socket,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_are_production_and_need_no_environment() {
        let config = resolve(no_env, Path::new("/Users/dev"));
        assert_eq!(config.relay_url, "wss://relay.getbuild.ing");
        assert_eq!(config.api_url, "https://getbuild.ing");
        assert_eq!(config.web_url, "https://getbuild.ing");
        assert_eq!(config.repo, None, "no phantom default project");
        assert_eq!(
            config.worktrees,
            PathBuf::from("/Users/dev/.build/worktrees")
        );
        assert_eq!(config.base_branch, "main");
        assert_eq!(
            config.identity_file,
            PathBuf::from("/Users/dev/.build/identity.json")
        );
        assert_eq!(
            config.mcp_socket,
            PathBuf::from("/Users/dev/.build/worktrees/build-bridge-mcp.sock")
        );
    }

    #[test]
    fn every_value_is_overridable_for_dev_stacks() {
        let env = |key: &str| -> Option<String> {
            Some(
                match key {
                    // Plain-ws is deliberate here: a test fixture for the local
                    // compose dev stack, never a production connection default
                    // (which is wss — see defaults_are_production_...).
                    "BRIDGE_RELAY_URL" => "ws://relay:8081", // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
                    "BRIDGE_API_URL" => "http://app:8080",
                    "BRIDGE_WEB_URL" => "http://localhost:8090",
                    "BRIDGE_REPO" => "/repo",
                    "BRIDGE_WORKTREES" => "/worktrees",
                    "BRIDGE_BASE_BRANCH" => "trunk",
                    "BRIDGE_IDENTITY_FILE" => "/state/identity.json",
                    "BRIDGE_MCP_SOCKET" => "/state/mcp.sock",
                    _ => return None,
                }
                .to_string(),
            )
        };
        let config = resolve(env, Path::new("/Users/dev"));
        assert_eq!(config.relay_url, "ws://relay:8081"); // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
        assert_eq!(config.api_url, "http://app:8080");
        assert_eq!(config.web_url, "http://localhost:8090");
        assert_eq!(config.repo.as_deref(), Some("/repo"));
        assert_eq!(config.worktrees, PathBuf::from("/worktrees"));
        assert_eq!(config.base_branch, "trunk");
        assert_eq!(config.identity_file, PathBuf::from("/state/identity.json"));
        assert_eq!(config.mcp_socket, PathBuf::from("/state/mcp.sock"));
    }

    #[test]
    fn web_url_follows_an_overridden_api_url() {
        let env = |key: &str| (key == "BRIDGE_API_URL").then(|| "http://app:8080".to_string());
        let config = resolve(env, Path::new("/Users/dev"));
        assert_eq!(config.web_url, "http://app:8080");
    }

    #[test]
    fn tilde_paths_expand_against_home() {
        let env = |key: &str| match key {
            "BRIDGE_WORKTREES" => Some("~/work/trees".to_string()),
            "BRIDGE_IDENTITY_FILE" => Some("~/ids/bridge.json".to_string()),
            _ => None,
        };
        let config = resolve(env, Path::new("/Users/dev"));
        assert_eq!(config.worktrees, PathBuf::from("/Users/dev/work/trees"));
        assert_eq!(
            config.identity_file,
            PathBuf::from("/Users/dev/ids/bridge.json")
        );
        // the socket default tracks the overridden worktrees dir
        assert_eq!(
            config.mcp_socket,
            PathBuf::from("/Users/dev/work/trees/build-bridge-mcp.sock")
        );
    }
}
