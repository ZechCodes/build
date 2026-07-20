//! launchd LaunchAgent install for the bridge daemon (macOS).
//!
//! `build-bridge install-service` is **gated on account pairing**: the device
//! must hold a local identity AND the api must confirm it is approved and owned
//! by a user account before any service file is written. The api is the ground
//! truth — a local `approved` flag can go stale (e.g. the backend was replaced
//! and its device registry reset), so it is never trusted on its own.
//!
//! The launchctl calls themselves live in `main.rs`; everything here is pure so
//! the gate and the generated plist are fully testable.

use std::path::{Path, PathBuf};

use crate::pairing::StatusResponse;

pub const SERVICE_LABEL: &str = "ing.getbuild.bridge";

/// Why the daemon may not be installed yet, with operator instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallGateError {
    /// No identity file — this machine has never started pairing.
    NotPaired,
    /// Registered, but no human has approved the device in the web app yet.
    PendingApproval,
    /// The api does not know this device (stale local identity — e.g. the
    /// backend's device registry was reset since this device last paired).
    UnknownToApi { detail: String },
    /// The api confirmed the device but it is approved without an owner —
    /// never expected from the real pairing flow; refuse rather than guess.
    ApprovedWithoutOwner,
}

impl std::fmt::Display for InstallGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPaired => write!(
                f,
                "this device is not paired to an account — run `build-bridge serve` once, \
                 approve the pairing code in the web app (Settings → Devices), then re-run \
                 `build-bridge install-service`"
            ),
            Self::PendingApproval => write!(
                f,
                "pairing is registered but not approved yet — approve this device in the \
                 web app (Settings → Devices), then re-run `build-bridge install-service`"
            ),
            Self::UnknownToApi { detail } => write!(
                f,
                "the api does not recognize this device ({detail}) — its registry may have \
                 been reset; re-pair by running `build-bridge serve` and approving the new \
                 pairing code in the web app"
            ),
            Self::ApprovedWithoutOwner => write!(
                f,
                "the api reports this device approved but owned by no account — refusing to \
                 install; re-pair with `build-bridge serve`"
            ),
        }
    }
}

/// The install gate. `local_identity_exists` is whether an identity file was
/// loaded; `api_status` is the live answer from `/api/devices/{id}/status`
/// (`Err` carries the api's error, e.g. a 404 for an unknown device).
pub fn check_install_gate(
    local_identity_exists: bool,
    api_status: Result<&StatusResponse, &str>,
) -> Result<String, InstallGateError> {
    if !local_identity_exists {
        return Err(InstallGateError::NotPaired);
    }
    match api_status {
        Err(detail) => Err(InstallGateError::UnknownToApi {
            detail: detail.to_string(),
        }),
        Ok(status) if !status.approved => Err(InstallGateError::PendingApproval),
        Ok(status) => match &status.owner_user_id {
            Some(owner) => Ok(owner.clone()),
            None => Err(InstallGateError::ApprovedWithoutOwner),
        },
    }
}

/// Everything the LaunchAgent needs baked into its plist.
#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub binary_path: PathBuf,
    pub log_dir: PathBuf,
    /// BRIDGE_* environment carried into the daemon, already resolved.
    pub env: Vec<(String, String)>,
}

/// Pin the installing shell's PATH into the daemon environment. launchd starts
/// agents with a bare PATH, and a bare PATH means the `claude` harness — which
/// lives wherever the user's toolchain manager put it — cannot be spawned at
/// all. `path` supplies the value (normally `std::env::var("PATH")`); an
/// explicit PATH already in `env` wins.
pub fn with_install_path(
    mut env: Vec<(String, String)>,
    path: impl FnOnce() -> Option<String>,
) -> Vec<(String, String)> {
    if env.iter().any(|(key, _)| key == "PATH") {
        return env;
    }
    if let Some(path) = path().filter(|path| !path.trim().is_empty()) {
        env.push(("PATH".to_string(), path));
    }
    env
}

pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist"))
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render the LaunchAgent plist: run at load, keep alive across crashes, log
/// under the bridge's own state dir.
pub fn render_launchd_plist(config: &ServiceConfig) -> String {
    let binary = xml_escape(&config.binary_path.to_string_lossy());
    let stdout_log = xml_escape(&config.log_dir.join("bridge.log").to_string_lossy());
    let stderr_log = xml_escape(&config.log_dir.join("bridge.err.log").to_string_lossy());
    let env_entries = config
        .env
        .iter()
        .map(|(key, value)| {
            format!(
                "      <key>{}</key>\n      <string>{}</string>",
                xml_escape(key),
                xml_escape(value)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>{SERVICE_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{binary}</string>
      <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>EnvironmentVariables</key>
    <dict>
{env_entries}
    </dict>
    <key>StandardOutPath</key>
    <string>{stdout_log}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_log}</string>
  </dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(approved: bool, owner: Option<&str>) -> StatusResponse {
        StatusResponse {
            approved,
            owner_user_id: owner.map(str::to_string),
        }
    }

    #[test]
    fn gate_refuses_when_no_local_identity() {
        let api = status(true, Some("user-1"));
        let result = check_install_gate(false, Ok(&api));
        assert_eq!(result, Err(InstallGateError::NotPaired));
    }

    #[test]
    fn gate_refuses_when_api_does_not_know_the_device() {
        let result = check_install_gate(true, Err("status 404"));
        assert_eq!(
            result,
            Err(InstallGateError::UnknownToApi {
                detail: "status 404".into()
            })
        );
    }

    #[test]
    fn gate_refuses_pending_approval() {
        let api = status(false, None);
        let result = check_install_gate(true, Ok(&api));
        assert_eq!(result, Err(InstallGateError::PendingApproval));
    }

    #[test]
    fn gate_refuses_approved_but_ownerless() {
        let api = status(true, None);
        let result = check_install_gate(true, Ok(&api));
        assert_eq!(result, Err(InstallGateError::ApprovedWithoutOwner));
    }

    #[test]
    fn gate_passes_only_for_an_approved_owned_device() {
        let api = status(true, Some("user-42"));
        let result = check_install_gate(true, Ok(&api));
        assert_eq!(result, Ok("user-42".to_string()));
    }

    #[test]
    fn gate_error_messages_tell_the_operator_what_to_do() {
        assert!(InstallGateError::NotPaired.to_string().contains("serve"));
        assert!(InstallGateError::PendingApproval
            .to_string()
            .contains("approve"));
        assert!(InstallGateError::UnknownToApi {
            detail: "404".into()
        }
        .to_string()
        .contains("re-pair"));
    }

    fn sample_config() -> ServiceConfig {
        ServiceConfig {
            binary_path: PathBuf::from("/usr/local/bin/build-bridge"),
            log_dir: PathBuf::from("/Users/dev/.build/log"),
            env: vec![
                ("BRIDGE_RELAY_URL".into(), "wss://relay.getbuild.ing".into()),
                ("BRIDGE_API_URL".into(), "https://getbuild.ing".into()),
            ],
        }
    }

    #[test]
    fn plist_runs_serve_keeps_alive_and_carries_env() {
        let plist = render_launchd_plist(&sample_config());
        assert!(plist.contains("<string>ing.getbuild.bridge</string>"));
        assert!(plist.contains("<string>/usr/local/bin/build-bridge</string>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(plist.contains("<key>KeepAlive</key>\n    <true/>"));
        assert!(plist.contains("<key>BRIDGE_RELAY_URL</key>"));
        assert!(plist.contains("<string>wss://relay.getbuild.ing</string>"));
        assert!(plist.contains("/Users/dev/.build/log/bridge.log"));
        assert!(plist.contains("/Users/dev/.build/log/bridge.err.log"));
    }

    #[test]
    fn daemon_env_pins_the_installing_shells_path() {
        // launchd hands agents a bare PATH ("/usr/bin:/bin:/usr/sbin:/sbin"), so
        // a daemon installed from a normal shell must carry that shell's PATH or
        // it cannot find `claude` (or any other user-installed harness).
        let env = with_install_path(vec![("BRIDGE_API_URL".into(), "https://x".into())], || {
            Some("/opt/homebrew/bin:/usr/bin".to_string())
        });
        assert_eq!(
            env.iter().find(|(key, _)| key == "PATH").map(|(_, v)| v),
            Some(&"/opt/homebrew/bin:/usr/bin".to_string())
        );
        assert!(render_launchd_plist(&ServiceConfig {
            env,
            ..sample_config()
        })
        .contains("<key>PATH</key>"));
    }

    #[test]
    fn an_explicit_path_in_the_env_is_left_alone() {
        let env = with_install_path(vec![("PATH".into(), "/pinned".into())], || {
            Some("/opt/homebrew/bin".to_string())
        });
        assert_eq!(env, vec![("PATH".to_string(), "/pinned".to_string())]);
    }

    #[test]
    fn plist_escapes_xml_significant_characters() {
        let mut config = sample_config();
        config.env = vec![("BRIDGE_DEVICE_NAME".into(), "Zech's <Mac> & co".into())];
        let plist = render_launchd_plist(&config);
        assert!(plist.contains("Zech's &lt;Mac&gt; &amp; co"));
        assert!(!plist.contains("<Mac>"));
    }

    #[test]
    fn plist_path_is_the_user_launch_agent() {
        let path = plist_path(Path::new("/Users/dev"));
        assert_eq!(
            path,
            PathBuf::from("/Users/dev/Library/LaunchAgents/ing.getbuild.bridge.plist")
        );
    }
}
