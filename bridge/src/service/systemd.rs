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

/// The slice the daemon spawns every agent into. Its weight and memory ceiling
/// put the agents behind the bridge and the user's apps (deploy/OPS.md,
/// "Priority").
pub const AGENTS_SLICE: &str = "app-build-agents.slice";

/// Where systemd puts [`AGENTS_SLICE`]: a slice's name is its path, so
/// `app-build-agents.slice` lives in `app-build.slice`, which lives in
/// `app.slice` beside this unit and the user's apps. Weights only arbitrate
/// between siblings, so this is the slice that competes with them.
const AGENTS_PARENT_SLICE: &str = "app-build.slice";

/// The agents' CPU share, against the bridge's 500 and a user app's 100.
const AGENTS_CPU_WEIGHT: &str = "CPUWeight=20";

/// Said on stderr when a slice property does not take.
const AGENTS_SLICE_NOT_SET: &str =
    "the bridge is installed, but its agents run at systemd's default priority.";

/// One MiB in bytes.
const MIB: u64 = 1024 * 1024;

pub struct Systemd;

/// systemd's own quoting for a unit value: a double-quoted string in which `\`
/// and `"` are backslash-escaped and `%` is doubled. Quoting unconditionally
/// keeps a value with spaces (a PATH entry, a device name, an install directory)
/// one word; doubling `%` stops systemd expanding a specifier (`%h`, `%i`, …) in
/// an `Environment=` value or an `ExecStart=` word, which would otherwise make
/// the unit carry something other than what was written.
fn systemd_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
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
    ///
    /// The `ExecStart=` program is quoted because it is wherever the installer
    /// put the binary — a directory with a space or a `%` in it must still run
    /// the command that was written, exactly as the launchd renderer escapes an
    /// arbitrary path into its plist. The `append:` log paths are left bare:
    /// systemd.exec(5) does not document a quoted form for them.
    ///
    /// `CPUWeight=500` puts the bridge ahead of the user's apps (systemd's
    /// default, 100) and far ahead of its agents' slice (20): it relays the
    /// user's live phone session. `ExecStart=` must not change shape —
    /// `update::provenance` matches that line byte for byte.
    fn render_unit(&self, config: &ServiceConfig) -> String {
        let binary = systemd_quote(&config.binary_path.to_string_lossy());
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
             CPUWeight=500\n\
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
    /// read last time. The agents' slices are weighted before the bridge
    /// starts, so the first agent it spawns already runs behind the user's
    /// apps; the ceiling is sized from this machine's RAM.
    fn activate(&self, _ctx: &ServiceContext, _unit_path: &Path) -> Vec<ShellCommand> {
        activation_commands(read_mem_total_kib())
    }

    /// Both tolerated: uninstalling a unit that was never enabled still has to
    /// remove the file. The agents' slices keep their settings: a reinstalled
    /// bridge's agents run under them again.
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

/// `activate`, given the machine's `MemTotal` in KiB (`None` when unknown).
/// The slice properties are best effort: an older systemd, or a machine
/// without user slices, still gets a working bridge, told why its agents are
/// not deprioritised.
fn activation_commands(mem_total_kib: Option<u64>) -> Vec<ShellCommand> {
    vec![
        ShellCommand::required("systemctl", &["--user", "daemon-reload"]),
        agents_parent_priority(),
        agents_slice_priority(mem_total_kib),
        ShellCommand::required("systemctl", &["--user", "enable", "--now", UNIT_NAME]),
    ]
}

/// The weight that ranks the agents against the bridge and the user's apps,
/// set where it counts: on their sibling in `app.slice`.
fn agents_parent_priority() -> ShellCommand {
    ShellCommand::best_effort(
        "systemctl",
        &[
            "--user",
            "set-property",
            AGENTS_PARENT_SLICE,
            AGENTS_CPU_WEIGHT,
        ],
        AGENTS_SLICE_NOT_SET,
    )
}

/// The agents' own slice: the same weight, and a `MemoryHigh` of 75 % of RAM.
/// No `MemoryMax`: nothing is killed; above the ceiling the kernel reclaims
/// the agents' memory, page cache first. A ceiling that rounds to 0 MiB is
/// left off rather than throttle every agent to a halt.
fn agents_slice_priority(mem_total_kib: Option<u64>) -> ShellCommand {
    let memory_high = mem_total_kib
        .map(agents_memory_high)
        .filter(|bytes| *bytes > 0)
        .map(|bytes| format!("MemoryHigh={}M", bytes / MIB));
    let mut args = vec!["--user", "set-property", AGENTS_SLICE, AGENTS_CPU_WEIGHT];
    args.extend(memory_high.as_deref());
    ShellCommand::best_effort("systemctl", &args, AGENTS_SLICE_NOT_SET)
}

/// The agents' `MemoryHigh` in bytes: 75 % of `MemTotal` (KiB), rounded down
/// to a whole MiB.
fn agents_memory_high(mem_total_kib: u64) -> u64 {
    (mem_total_kib.saturating_mul(3) / 4 / 1024).saturating_mul(MIB)
}

/// This machine's physical RAM, as `/proc/meminfo` reports it.
fn read_mem_total_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .as_deref()
        .and_then(parse_mem_total_kib)
}

/// `MemTotal:  131072000 kB` → `131072000`.
fn parse_mem_total_kib(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::fixtures::{context, sample_config};

    /// The Linux home every fixture in this module is built around.
    const HOME: &str = "/home/dev";

    #[test]
    fn unit_path_is_the_users_systemd_dir() {
        assert_eq!(
            Systemd.unit_path(Path::new("/home/dev")),
            PathBuf::from("/home/dev/.config/systemd/user/build-bridge.service")
        );
    }

    #[test]
    fn unit_runs_serve_restarts_on_failure_and_logs_under_the_state_dir() {
        let unit = Systemd.render_unit(&sample_config(HOME));
        assert!(unit.contains("ExecStart=\"/home/dev/.local/bin/build-bridge\" serve\n"));
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("RestartSec=5\nCPUWeight=500\n"));
        assert!(unit.contains("StandardOutput=append:/home/dev/.build/log/bridge.log\n"));
        assert!(unit.contains("StandardError=append:/home/dev/.build/log/bridge.err.log\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
    }

    /// The whole unit, byte for byte: `ExecStart=` is what `update::provenance`
    /// matches to recognise an installed bridge, and `CPUWeight=500` is the
    /// bridge's share against the user's apps (100) and the agents' slice (20)
    /// — see deploy/OPS.md, "Priority".
    #[test]
    fn unit_text_is_pinned() {
        let unit = Systemd.render_unit(&ServiceConfig {
            env: vec![],
            ..sample_config(HOME)
        });
        assert_eq!(
            unit,
            "[Unit]\n\
             Description=Build bridge (device daemon)\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart=\"/home/dev/.local/bin/build-bridge\" serve\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             CPUWeight=500\n\
             StandardOutput=append:/home/dev/.build/log/bridge.log\n\
             StandardError=append:/home/dev/.build/log/bridge.err.log\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        );
    }

    #[test]
    fn unit_carries_each_env_pair_as_its_own_environment_line() {
        let unit = Systemd.render_unit(&sample_config(HOME));
        assert!(unit.contains("Environment=\"BRIDGE_RELAY_URL=wss://relay.getbuild.ing\"\n"));
        assert!(unit.contains("Environment=\"BRIDGE_API_URL=https://getbuild.ing\"\n"));
    }

    /// systemd starts user services with a bare PATH too, so the pinned PATH
    /// has to arrive as an `Environment=` line like every other pair — the
    /// reason `with_install_path` is shared rather than a launchd detail.
    #[test]
    fn the_unit_carries_the_pinned_path() {
        let env = crate::service::with_install_path(vec![], || Some("/home/dev/.local/bin".into()));

        let unit = Systemd.render_unit(&ServiceConfig {
            env,
            ..sample_config(HOME)
        });

        assert!(
            unit.contains("Environment=\"PATH=/home/dev/.local/bin\"\n"),
            "the daemon inherits the installing shell's PATH: {unit}"
        );
    }

    #[test]
    fn unit_quotes_values_with_spaces_quotes_and_backslashes() {
        let unit = Systemd.render_unit(&ServiceConfig {
            env: vec![("BRIDGE_DEVICE_NAME".into(), r#"Zech's "Mac" \ desk"#.into())],
            ..sample_config(HOME)
        });
        assert!(
            unit.contains(r#"Environment="BRIDGE_DEVICE_NAME=Zech's \"Mac\" \\ desk""#),
            "the value stays one assignment: {unit}"
        );
    }

    #[test]
    fn exec_start_quotes_the_binary_path() {
        let unit = Systemd.render_unit(&ServiceConfig {
            binary_path: PathBuf::from("/home/dev/my tools/100% build/build-bridge"),
            ..sample_config(HOME)
        });
        assert!(
            unit.contains("ExecStart=\"/home/dev/my tools/100%% build/build-bridge\" serve\n"),
            "the installed path stays one argument, specifiers unexpanded: {unit}"
        );
    }

    #[test]
    fn unit_escapes_percent_so_systemd_expands_no_specifier() {
        let unit = Systemd.render_unit(&ServiceConfig {
            env: vec![("BRIDGE_DEVICE_NAME".into(), "100% mine %H".into())],
            ..sample_config(HOME)
        });
        assert!(
            unit.contains(r#"Environment="BRIDGE_DEVICE_NAME=100%% mine %%H""#),
            "the value reaches the daemon verbatim: {unit}"
        );
    }

    /// A 128 GiB machine as `/proc/meminfo` reports it: 134217728 kB.
    const MEM_128_GIB_KIB: u64 = 128 * 1024 * 1024;

    #[test]
    fn activation_reloads_weights_the_agents_slice_then_enables_now() {
        assert_eq!(
            activation_commands(Some(MEM_128_GIB_KIB)),
            vec![
                ShellCommand::required("systemctl", &["--user", "daemon-reload"]),
                ShellCommand::best_effort(
                    "systemctl",
                    &["--user", "set-property", "app-build.slice", "CPUWeight=20"],
                    AGENTS_SLICE_NOT_SET,
                ),
                ShellCommand::best_effort(
                    "systemctl",
                    &[
                        "--user",
                        "set-property",
                        "app-build-agents.slice",
                        "CPUWeight=20",
                        "MemoryHigh=98304M",
                    ],
                    AGENTS_SLICE_NOT_SET,
                ),
                ShellCommand::required(
                    "systemctl",
                    &["--user", "enable", "--now", "build-bridge.service"]
                ),
            ]
        );
    }

    /// The installer sizes the slice from the machine it runs on.
    #[test]
    fn activate_sizes_the_agents_slice_from_this_machines_memory() {
        let ctx = context(HOME, "1000");
        let unit = Systemd.unit_path(&ctx.home);

        assert_eq!(
            Systemd.activate(&ctx, &unit),
            activation_commands(read_mem_total_kib())
        );
    }

    /// A failed set-property (an older systemd, a machine without the slice) is
    /// reported and survived: the bridge itself is still a working install.
    #[test]
    fn the_agents_slice_is_best_effort_and_says_so_when_it_fails() {
        for command in [
            agents_parent_priority(),
            agents_slice_priority(Some(MEM_128_GIB_KIB)),
        ] {
            assert!(command.tolerate_failure);
            assert!(command.failure_note.is_some());
        }
    }

    /// A slice's name is its path: `a-b-c.slice` lives in `a-b.slice`. The
    /// weight that ranks the agents against the bridge and the user's apps
    /// has to be on the slice that is their sibling in `app.slice`.
    #[test]
    fn the_parent_slice_is_the_agents_slice_one_level_up() {
        let (parent, _) = AGENTS_SLICE
            .strip_suffix(".slice")
            .and_then(|name| name.rsplit_once('-'))
            .expect("the agents' slice is nested");
        assert_eq!(format!("{parent}.slice"), AGENTS_PARENT_SLICE);
        assert_eq!(AGENTS_PARENT_SLICE, "app-build.slice");
    }

    /// Without a readable MemTotal the slice still gets its CPU weight; no
    /// memory ceiling is guessed.
    #[test]
    fn an_unknown_memory_size_weights_the_slice_without_a_memory_ceiling() {
        assert_eq!(
            agents_slice_priority(None).args,
            vec![
                "--user",
                "set-property",
                "app-build-agents.slice",
                "CPUWeight=20"
            ]
        );
    }

    /// 75 % of physical RAM, in bytes, rounded down to a whole MiB.
    #[test]
    fn agents_memory_high_is_three_quarters_of_ram_in_whole_mib() {
        assert_eq!(agents_memory_high(MEM_128_GIB_KIB), 98_304 * MIB);
        // 32594520 kB (a "32 GB" box) × 3/4 = 24445890 KiB = 23872.9 MiB.
        assert_eq!(agents_memory_high(32_594_520), 23_872 * MIB);
        // 4097 KiB × 3/4 = 3072.75 KiB: 3 MiB, the fraction dropped.
        assert_eq!(agents_memory_high(4_097), 3 * MIB);
        assert_eq!(agents_memory_high(0), 0);
    }

    /// A MemoryHigh that rounds to 0 would throttle every agent to a halt, so a
    /// machine too small to have a whole MiB of headroom gets no ceiling at all.
    #[test]
    fn a_memory_ceiling_below_one_mib_is_not_set() {
        assert!(!agents_slice_priority(Some(1_000))
            .args
            .iter()
            .any(|arg| arg.starts_with("MemoryHigh=")));
    }

    #[test]
    fn mem_total_is_read_from_proc_meminfo() {
        let meminfo = "MemTotal:       131072000 kB\n\
                       MemFree:         1024000 kB\n\
                       MemAvailable:   65536000 kB\n";
        assert_eq!(parse_mem_total_kib(meminfo), Some(131_072_000));
        assert_eq!(parse_mem_total_kib("MemFree: 12 kB\n"), None);
        assert_eq!(parse_mem_total_kib("MemTotal: lots kB\n"), None);
    }

    /// Uninstall leaves `app-build-agents.slice` weighted: nothing here resets
    /// it, and a reinstalled bridge's agents run under it again.
    #[test]
    fn deactivation_disables_now_then_reloads_and_tolerates_failure() {
        let ctx = context(HOME, "1000");
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
