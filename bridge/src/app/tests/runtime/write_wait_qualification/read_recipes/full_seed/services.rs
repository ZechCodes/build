//! Inject before AppState initialization/listener capture.
//! No real CLI/provider/network/updater backend.
use crate::harness::installed::{CliProbe, CliReading, ListedModel, Readings};
use crate::harness::inventory::{DeviceEnvironment, Inventory, RunningSessions};
use crate::update::{Release, UpdateBackend, UpdateConfig, UpdateService};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct FixedCli;
impl CliProbe for FixedCli {
    fn read(&self, binary: &str) -> CliReading {
        if binary == "claude" || binary == "codex" {
            CliReading {
                version: Some(semver::Version::new(2, 1, 999)),
                listed: Some(vec![ListedModel {
                    id: "qualification-model".into(),
                    label: "Qualification fixed model".into(),
                    hidden: false,
                    efforts: vec!["low".into(), "high".into()],
                }]),
            }
        } else {
            CliReading::default()
        }
    }
    fn needs_retry(&self, _: &CliReading) -> bool {
        false
    }
}
static FIXED_CLI: FixedCli = FixedCli;

pub(super) struct Services {
    pub readings: Arc<Readings>,
    pub inventory: Arc<Inventory>,
    pub github: crate::github::GithubCli,
    pub updates: Arc<UpdateService>,
    pub update_backend: Arc<FixedUpdate>,
    pub github_argv: PathBuf,
    bin: PathBuf,
}

pub(super) struct FixedUpdate {
    pub calls: Mutex<Vec<&'static str>>,
}
#[async_trait::async_trait]
impl UpdateBackend for FixedUpdate {
    async fn latest(&self) -> Result<Release, String> {
        self.calls.lock().unwrap().push("latest");
        Ok(Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: None,
        })
    }
    async fn install(&self, _: &Release, _: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push("FORBIDDEN install");
        Err("qualification read backend cannot install updates".into())
    }
}

pub(super) fn seed_services(root: &Path) -> Services {
    let bin = root.join("read-service-bin");
    let home = root.join("read-service-home");
    let managed = root.join("read-service-managed-policy");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&managed).unwrap();
    let cli = bin.join("claude");
    // Executable identity is present but model/version comes from FixedCli.
    // If unexpected code actually tries to run it, fail and record the attempt.
    std::fs::write(
        &cli,
        b"#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$0.unexpected-argv\"\nexit 91\n",
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
    let codex = bin.join("codex");
    std::fs::copy(&cli, &codex).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let readings = Readings::answering_inline(&FIXED_CLI);
    let running = RunningSessions::new();
    let environment =
        DeviceEnvironment::new(home, [("PATH", bin.as_os_str())]).with_claude_managed_root(managed);
    let inventory = Inventory::builder()
        .environment(move || environment.clone())
        .readings(readings.clone())
        .running(running.clone())
        .build();
    inventory.sweep();
    let gh = bin.join("gh");
    std::fs::write(&gh,br##"#!/bin/sh
printf '%s\n' "$*" >> "$0.argv"
case "$*" in
"repo list --limit 1000 --json nameWithOwner,description,sshUrl,url,isPrivate,pushedAt") printf '%s\n' '[{"nameWithOwner":"qualification/read-witness","description":"isolated fixture","sshUrl":"git@github.com:qualification/read-witness.git","url":"https://github.com/qualification/read-witness","isPrivate":true,"pushedAt":"2026-10-21T00:00:00Z"}]' ;;
"org list --limit 1000") exit 0 ;;
*) exit 92 ;;
esac
"##).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let github_argv = gh.with_extension("argv");
    let github = crate::github::GithubCli::at(gh, "qualification-fixture");
    let update_backend = Arc::new(FixedUpdate {
        calls: Mutex::new(Vec::new()),
    });
    let updates = Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("read-update/status.json"),
                result_path: root.join("read-update/result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(86400),
            },
            update_backend.clone(),
        )
        .unwrap(),
    );
    Services {
        readings,
        inventory,
        github,
        updates,
        update_backend,
        github_argv,
        bin,
    }
}
// Caller awaits updates.check() before timing; asserts Available + fixed release.
// Attach via AppState with_cli_readings/with_harness_inventory/with_github_cli/
// with_update_service BEFORE .shared()/ControlPlane/service capture.
// Retain Services for complete lifetime; verify no claude.unexpected-argv exists,
// no FORBIDDEN install calls, gh log contains only expected repo/org reads.
// RunningSessions must be wired to actual retained fake AgentSession version
// reports if the ready oracle claims running-version evidence.

impl Services {
    pub(super) fn install_into(&self, app: &mut crate::app::AppState) {
        app.cli_readings = self.readings.clone();
        app.harness_inventory = self.inventory.clone();
        app.github = self.github.clone();
        app.updates = Some(self.updates.clone());
        self.readings.warm();
    }
}

impl Services {
    pub(super) fn assert_isolated(&self, expect_github_reads: bool) {
        for executable in ["claude", "codex"] {
            assert!(
                !self
                    .bin
                    .join(format!("{executable}.unexpected-argv"))
                    .exists(),
                "unexpected direct provider executable probe"
            );
        }
        let calls = self.update_backend.calls.lock().unwrap();
        assert!(
            calls.contains(&"latest"),
            "fake update metadata check never ran"
        );
        assert!(
            !calls.contains(&"FORBIDDEN install"),
            "read fixture attempted update install"
        );
        let logged = std::fs::read_to_string(&self.github_argv).unwrap_or_default();
        if expect_github_reads {
            assert!(!logged.is_empty(), "native gh stand-in never ran");
        }
        for invocation in logged.lines() {
            assert!(matches!(invocation,
                "repo list --limit 1000 --json nameWithOwner,description,sshUrl,url,isPrivate,pushedAt"
                |"org list --limit 1000"),"unexpected gh stand-in call: {invocation}");
        }
    }
}
