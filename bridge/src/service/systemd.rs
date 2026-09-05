//! Linux: a systemd `--user` unit under `~/.config/systemd/user`.
//!
//! `enable --now` starts it and wires it to `default.target` so it comes back on
//! the next login. A user unit stops when the last session ends unless lingering
//! is on, which is a per-user decision the daemon cannot make for the operator —
//! hence [`ServiceManager::after_install_hint`].

use std::path::{Path, PathBuf};

use super::{ServiceConfig, ServiceContext, ServiceManager, ShellCommand};

/// The unit's name — how the operator addresses it with `systemctl --user`.
pub const UNIT_NAME: &str = "build-bridge.service";

pub struct Systemd;

/// systemd's own quoting for an `Environment=` assignment: a double-quoted
/// string in which `\` and `"` are backslash-escaped. Quoting unconditionally
/// keeps a value with spaces (a PATH entry, a device name) one assignment.
fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

impl ServiceManager for Systemd {
    fn name(&self) -> &'static str {
        "systemd user unit"
    }

    fn unit_path(&self, home: &Path) -> PathBuf {
        home.join(".config/systemd/user").join(UNIT_NAME)
    }

    /// Restart on failure rather than `always`: a bridge that exits 0 was told
    /// to stop. Logs append to the same two files the LaunchAgent writes, so
    /// "where are the logs" has one answer on both platforms.
    fn render_unit(&self, config: &ServiceConfig) -> String {
        let binary = config.binary_path.display();
        let stdout_log = config.log_dir.join("bridge.log");
        let stderr_log = config.log_dir.join("bridge.err.log");
        let environment = config
            .env
            .iter()
            .map(|(key, value)| {
                format!("Environment={}\n", systemd_quote(&format!("{key}={value}")))
            })
            .collect::<String>();
        format!(
            "[Unit]\n\
             Description=Build bridge (device daemon)\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart={binary} serve\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             {environment}\
             StandardOutput=append:{stdout}\n\
             StandardError=append:{stderr}\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            stdout = stdout_log.display(),
            stderr = stderr_log.display(),
        )
    }

    /// The reload has to land before the enable, or systemd enables the unit it
    /// read last time.
    fn activate(&self, _ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
        vec![
            ShellCommand::required("systemctl", &["--user", "daemon-reload"]),
            ShellCommand::required("systemctl", &["--user", "enable", "--now", UNIT_NAME]),
        ]
    }

    /// Both tolerated: uninstalling a unit that was never enabled still has to
    /// remove the file.
    fn deactivate(&self, _ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
        vec![
            ShellCommand::tolerated("systemctl", &["--user", "disable", "--now", UNIT_NAME]),
            ShellCommand::tolerated("systemctl", &["--user", "daemon-reload"]),
        ]
    }

    fn after_install_hint(&self) -> Option<&'static str> {
        Some("run `loginctl enable-linger $USER` to keep the bridge running while logged out")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> ServiceConfig {
        ServiceConfig {
            binary_path: PathBuf::from("/home/dev/.local/bin/build-bridge"),
            log_dir: PathBuf::from("/home/dev/.build/log"),
            env: vec![
                ("BRIDGE_RELAY_URL".into(), "wss://relay.getbuild.ing".into()),
                ("BRIDGE_API_URL".into(), "https://getbuild.ing".into()),
            ],
        }
    }

    fn context() -> ServiceContext {
        ServiceContext {
            home: PathBuf::from("/home/dev"),
            uid: "1000".to_string(),
        }
    }

    #[test]
    fn unit_path_is_the_users_systemd_dir() {
        assert_eq!(
            Systemd.unit_path(Path::new("/home/dev")),
            PathBuf::from("/home/dev/.config/systemd/user/build-bridge.service")
        );
    }

    #[test]
    fn unit_runs_serve_restarts_on_failure_and_logs_under_the_state_dir() {
        let unit = Systemd.render_unit(&sample_config());
        assert!(unit.contains("ExecStart=/home/dev/.local/bin/build-bridge serve\n"));
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("RestartSec=5\n"));
        assert!(unit.contains("StandardOutput=append:/home/dev/.build/log/bridge.log\n"));
        assert!(unit.contains("StandardError=append:/home/dev/.build/log/bridge.err.log\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
    }

    #[test]
    fn unit_carries_each_env_pair_as_its_own_environment_line() {
        let unit = Systemd.render_unit(&sample_config());
        assert!(unit.contains("Environment=\"BRIDGE_RELAY_URL=wss://relay.getbuild.ing\"\n"));
        assert!(unit.contains("Environment=\"BRIDGE_API_URL=https://getbuild.ing\"\n"));
    }

    #[test]
    fn unit_quotes_values_with_spaces_quotes_and_backslashes() {
        let unit = Systemd.render_unit(&ServiceConfig {
            env: vec![("BRIDGE_DEVICE_NAME".into(), r#"Zech's "Mac" \ desk"#.into())],
            ..sample_config()
        });
        assert!(
            unit.contains(r#"Environment="BRIDGE_DEVICE_NAME=Zech's \"Mac\" \\ desk""#),
            "the value stays one assignment: {unit}"
        );
    }

    #[test]
    fn activation_reloads_then_enables_now() {
        let ctx = context();
        let unit = Systemd.unit_path(&ctx.home);

        assert_eq!(
            Systemd.activate(&ctx, &unit),
            vec![
                ShellCommand::required("systemctl", &["--user", "daemon-reload"]),
                ShellCommand::required(
                    "systemctl",
                    &["--user", "enable", "--now", "build-bridge.service"]
                ),
            ]
        );
    }

    #[test]
    fn deactivation_disables_now_then_reloads_and_tolerates_failure() {
        let ctx = context();
        let unit = Systemd.unit_path(&ctx.home);

        assert_eq!(
            Systemd.deactivate(&ctx, &unit),
            vec![
                ShellCommand::tolerated(
                    "systemctl",
                    &["--user", "disable", "--now", "build-bridge.service"]
                ),
                ShellCommand::tolerated("systemctl", &["--user", "daemon-reload"]),
            ]
        );
    }

    #[test]
    fn after_install_hint_names_enable_linger() {
        let hint = Systemd.after_install_hint().expect("systemd has a hint");
        assert!(hint.contains("enable-linger"));
    }
}
