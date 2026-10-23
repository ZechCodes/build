//! Bridge runtime configuration: production-by-default so the end-user story is
//! "install, pair, go" with zero environment setup. Every value can still be
//! overridden through `BRIDGE_*` variables (dev stacks and compose set them all
//! explicitly).

use std::io;
use std::path::{Component, Path, PathBuf};

pub const DEFAULT_RELAY_URL: &str = "wss://relay.getbuild.ing";
pub const DEFAULT_API_URL: &str = "https://getbuild.ing";
pub const DEFAULT_BASE_BRANCH: &str = "main";

/// The selected stack. A channel is explicit authority for state and updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Prod,
    Dev,
}

impl Channel {
    pub fn resolve(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("prod") => Ok(Self::Prod),
            Some("dev") => Ok(Self::Dev),
            Some(other) => Err(format!(
                "unknown BRIDGE_CHANNEL: {other:?}; expected prod or dev"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Dev => "dev",
        }
    }
}

/// The resolved serve/install configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub channel: Channel,
    pub state_root: PathBuf,
    pub tasks_dir: PathBuf,
    pub config_path: PathBuf,
    pub projects_dir: PathBuf,
    pub log_dir: PathBuf,
    pub relay_url: String,
    pub api_url: String,
    /// The web app base URL shown in the pairing approval link.
    pub web_url: String,
    /// Explicitly configured default repo; `None` means projects come from the
    /// UI (clone/add) and persisted config only — never a phantom default path.
    pub repo: Option<String>,
    /// Extra environment projects, with the exact paths startup will register.
    pub environment_projects: Vec<(PathBuf, String)>,
    pub worktrees: PathBuf,
    pub base_branch: String,
    pub identity_file: PathBuf,
    pub mcp_socket: PathBuf,
    /// The home directory the paths above were resolved against; what the
    /// bridge prints names paths under it with `~`.
    pub home: PathBuf,
}

impl BridgeConfig {
    /// The task store and harness share the task directory's parent as state.
    pub fn task_state_root(&self) -> &Path {
        self.tasks_dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }
}

/// Resolve config from an environment lookup and the user's home directory.
/// Dev validates filesystem aliases without creating state. Prod keeps its
/// original environment-only resolution.
pub fn resolve(
    lookup: impl Fn(&str) -> Option<String>,
    home: &Path,
) -> Result<BridgeConfig, String> {
    let channel = Channel::resolve(lookup("BRIDGE_CHANNEL").as_deref())?;
    let (root, api, relay) = match channel {
        Channel::Prod => (".build", DEFAULT_API_URL, DEFAULT_RELAY_URL),
        Channel::Dev => (
            ".local/state/build-dev",
            "https://dev.getbuild.ing",
            "wss://relay-dev.getbuild.ing",
        ),
    };
    let state_root = home.join(root);
    let expand = |raw: String| expand_home(&raw, home);
    let api_url = lookup("BRIDGE_API_URL").unwrap_or_else(|| api.to_string());
    let base_branch =
        lookup("BRIDGE_BASE_BRANCH").unwrap_or_else(|| DEFAULT_BASE_BRANCH.to_string());
    let worktrees = lookup("BRIDGE_WORKTREES")
        .map(expand)
        .unwrap_or_else(|| state_root.join("worktrees"));
    let mcp_socket = lookup("BRIDGE_MCP_SOCKET")
        .map(expand)
        .unwrap_or_else(|| match channel {
            Channel::Prod => worktrees.join("build-bridge-mcp.sock"),
            Channel::Dev => state_root.join("mcp.sock"),
        });
    let state_path = |key: &str, leaf: &str| {
        lookup(key)
            .map(expand)
            .unwrap_or_else(|| state_root.join(leaf))
    };
    let config = BridgeConfig {
        channel,
        tasks_dir: state_path("BRIDGE_TASKS_DIR", "tasks"),
        config_path: state_path("BRIDGE_CONFIG", "config.json"),
        projects_dir: state_path("BRIDGE_PROJECTS_DIR", "projects"),
        log_dir: state_root.join("log"),
        relay_url: lookup("BRIDGE_RELAY_URL").unwrap_or_else(|| relay.to_string()),
        web_url: lookup("BRIDGE_WEB_URL").unwrap_or_else(|| api_url.clone()),
        api_url,
        repo: lookup("BRIDGE_REPO").map(|raw| {
            project_path(&raw, channel, home)
                .to_string_lossy()
                .into_owned()
        }),
        environment_projects: resolve_environment_projects(
            lookup("BRIDGE_PROJECTS").as_deref(),
            channel,
            home,
            &base_branch,
        ),
        worktrees,
        base_branch,
        identity_file: lookup("BRIDGE_IDENTITY_FILE")
            .map(expand)
            .unwrap_or_else(|| state_root.join("identity.json")),
        mcp_socket,
        home: home.to_path_buf(),
        state_root,
    };
    if channel == Channel::Dev {
        validate_dev_paths(&config)?;
    }
    Ok(config)
}

fn expand_home(raw: &str, home: &Path) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if raw == "~" => home.to_path_buf(),
        None => PathBuf::from(raw),
    }
}

fn project_path(raw: &str, channel: Channel, home: &Path) -> PathBuf {
    match channel {
        Channel::Prod => PathBuf::from(raw),
        Channel::Dev => expand_home(raw, home),
    }
}

fn resolve_environment_projects(
    value: Option<&str>,
    channel: Channel,
    home: &Path,
    base_branch: &str,
) -> Vec<(PathBuf, String)> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (path, branch) = entry.split_once('=').unwrap_or((entry, base_branch));
            (project_path(path, channel, home), branch.to_string())
        })
        .collect()
}

fn validate_dev_paths(config: &BridgeConfig) -> Result<(), String> {
    let prod = canonicalize_state_path(&config.home.join(".build"))
        .map_err(|error| format!("cannot resolve production state root: {error}"))?;
    let paths: [(&str, &Path); 9] = [
        ("state_root", &config.state_root),
        ("BRIDGE_IDENTITY_FILE", &config.identity_file),
        ("BRIDGE_CONFIG", &config.config_path),
        ("BRIDGE_TASKS_DIR", &config.tasks_dir),
        ("task_state_root", config.task_state_root()),
        ("BRIDGE_PROJECTS_DIR", &config.projects_dir),
        ("BRIDGE_WORKTREES", &config.worktrees),
        ("BRIDGE_MCP_SOCKET", &config.mcp_socket),
        ("log_dir", &config.log_dir),
    ];
    for (name, path) in paths {
        validate_dev_path(name, path, &prod)?;
    }
    if let Some(repo) = &config.repo {
        validate_dev_path("BRIDGE_REPO", Path::new(repo), &prod)?;
    }
    for (path, _) in &config.environment_projects {
        validate_dev_path("BRIDGE_PROJECTS", path, &prod)?;
    }
    Ok(())
}

fn validate_dev_path(name: &str, path: &Path, prod: &Path) -> Result<(), String> {
    let resolved = canonicalize_state_path(path)
        .map_err(|error| format!("cannot resolve {name} ({}): {error}", path.display()))?;
    if resolved.starts_with(prod) {
        return Err(format!(
            "BRIDGE_CHANNEL=dev refuses {name} ({}): {} is within the production state root {}",
            path.display(),
            resolved.display(),
            prod.display()
        ));
    }
    Ok(())
}

/// Resolve existing symlinks in order, retaining missing components for a fresh
/// installation. Resolving `..` only after symlinks prevents alias/../ escapes.
fn canonicalize_state_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    resolve_state_components(&absolute, 0)
}

fn resolve_state_components(path: &Path, symlinks: u32) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::RootDir | Component::Prefix(_) => resolved.push(component),
            Component::Normal(name) => {
                resolved.push(name);
                if let Some(target) = symlink_target(&resolved)? {
                    if symlinks >= 40 {
                        return Err(io::Error::other("too many state path symlinks"));
                    }
                    resolved.pop();
                    return resolve_state_components(
                        &resolved.join(target).join(components.as_path()),
                        symlinks + 1,
                    );
                }
                resolved = canonicalize_existing(&resolved)?;
            }
        }
    }
    Ok(resolved)
}

fn canonicalize_existing(path: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error),
    }
}

fn symlink_target(path: &Path) -> io::Result<Option<PathBuf>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => std::fs::read_link(path).map(Some),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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
        let config = resolve(no_env, Path::new("/Users/dev")).unwrap();
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
        let config = resolve(env, Path::new("/Users/dev")).unwrap();
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
        let config = resolve(env, Path::new("/Users/dev")).unwrap();
        assert_eq!(config.web_url, "http://app:8080");
    }

    #[test]
    fn tilde_paths_expand_against_home() {
        let env = |key: &str| match key {
            "BRIDGE_WORKTREES" => Some("~/work/trees".to_string()),
            "BRIDGE_IDENTITY_FILE" => Some("~/ids/bridge.json".to_string()),
            _ => None,
        };
        let config = resolve(env, Path::new("/Users/dev")).unwrap();
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
