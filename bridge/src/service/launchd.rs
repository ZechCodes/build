//! macOS: a launchd LaunchAgent in the user's gui domain.
//!
//! The agent runs at load and is kept alive across crashes and logins. Its
//! environment is baked into the plist because launchd hands agents a bare
//! PATH, which cannot find a user-installed harness.

use std::path::{Path, PathBuf};

use super::{ServiceConfig, ServiceContext, ServiceManager, ShellCommand, SERVICE_LABEL};

pub struct Launchd;

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl ServiceManager for Launchd {
    fn name(&self) -> &'static str {
        "launchd LaunchAgent"
    }

    fn unit_path(&self, home: &Path) -> PathBuf {
        home.join("Library/LaunchAgents")
            .join(format!("{SERVICE_LABEL}.plist"))
    }

    /// Run at load, keep alive across crashes, log under the bridge's own state
    /// dir.
    fn render_unit(&self, config: &ServiceConfig) -> String {
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

    /// Re-installs: boot the old instance out first, which fails when nothing
    /// was loaded — normal, and tolerated.
    fn activate(&self, ctx: &ServiceContext, unit_path: &Path) -> Vec<ShellCommand> {
        vec![
            ShellCommand::tolerated("launchctl", &["bootout", &gui_service(ctx)]),
            ShellCommand::required(
                "launchctl",
                &["bootstrap", &gui_domain(ctx), &unit_path.to_string_lossy()],
            ),
        ]
    }

    fn deactivate(&self, ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
        vec![ShellCommand::tolerated(
            "launchctl",
            &["bootout", &gui_service(ctx)],
        )]
    }

    fn after_install_hint(&self) -> Option<&'static str> {
        None
    }
}

fn gui_domain(ctx: &ServiceContext) -> String {
    format!("gui/{}", ctx.uid)
}

fn gui_service(ctx: &ServiceContext) -> String {
    format!("{}/{SERVICE_LABEL}", gui_domain(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::with_install_path;

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

    fn context() -> ServiceContext {
        ServiceContext {
            home: PathBuf::from("/Users/dev"),
            uid: "501".to_string(),
        }
    }

    #[test]
    fn plist_runs_serve_keeps_alive_and_carries_env() {
        let plist = Launchd.render_unit(&sample_config());
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
        assert!(Launchd
            .render_unit(&ServiceConfig {
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
        let plist = Launchd.render_unit(&config);
        assert!(plist.contains("Zech's &lt;Mac&gt; &amp; co"));
        assert!(!plist.contains("<Mac>"));
    }

    #[test]
    fn plist_path_is_the_user_launch_agent() {
        assert_eq!(
            Launchd.unit_path(Path::new("/Users/dev")),
            PathBuf::from("/Users/dev/Library/LaunchAgents/ing.getbuild.bridge.plist")
        );
    }

    #[test]
    fn activation_boots_out_then_bootstraps_in_the_users_gui_domain() {
        let ctx = context();
        let unit = Launchd.unit_path(&ctx.home);
        let commands = Launchd.activate(&ctx, &unit);

        assert_eq!(
            commands,
            vec![
                ShellCommand::tolerated("launchctl", &["bootout", "gui/501/ing.getbuild.bridge"]),
                ShellCommand::required(
                    "launchctl",
                    &[
                        "bootstrap",
                        "gui/501",
                        "/Users/dev/Library/LaunchAgents/ing.getbuild.bridge.plist",
                    ]
                ),
            ]
        );
    }

    #[test]
    fn deactivation_only_boots_out() {
        let ctx = context();
        let unit = Launchd.unit_path(&ctx.home);

        assert_eq!(
            Launchd.deactivate(&ctx, &unit),
            vec![ShellCommand::tolerated(
                "launchctl",
                &["bootout", "gui/501/ing.getbuild.bridge"]
            )]
        );
    }

    #[test]
    fn launchd_asks_the_operator_for_nothing_after_install() {
        assert_eq!(Launchd.after_install_hint(), None);
    }
}
