//! Where the daemon's children run, and behind whom.
//!
//! Every agent the bridge starts, and every terminal, is a child of this
//! process, and everything they run (cargo, vitest, a headless Chromium) is a
//! grandchild. On Linux that puts all of it inside the daemon's own cgroup, and
//! a `CPUWeight` on the service unit, which only arbitrates between sibling
//! cgroups, can never keep the daemon ahead of its own children. On 2026-09-24
//! the user's phone lost its session every few seconds while ten vitest runs
//! and a headless Chromium, all agents' work, ran at the bridge's own priority
//! inside the bridge's own cgroup (issue #128).
//!
//! So the children are moved out. Where the user's systemd is reachable, each
//! child starts in its own transient scope under [`AGENTS_SLICE`], a sibling of
//! the daemon's unit and of the user's own apps, and the slice carries the
//! weight (a fifth of one app) and the memory ceiling (three quarters of RAM,
//! reclaimed before anything is killed). Where it is not reachable, on macOS,
//! in a container, or over a broken bus, the child is niced instead, which is
//! the one lever that works inside a cgroup. The nice is applied in both cases:
//! it costs nothing, and a scope that lands somewhere unexpected still leaves
//! the child behind the daemon.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

/// The slice every agent's scope is started in. A slice's name is its path,
/// dash by dash: `app-build_agents.slice` is a child of `app.slice` and of
/// nothing in between, so it sits beside the daemon's unit and the user's own
/// apps, which is the level the weight is meant to rank it at. (A slice named
/// with a second dash would have lived one level down, and its weight would
/// have ranked it against nothing.)
pub const AGENTS_SLICE: &str = "app-build_agents.slice";

/// Where the user's own terminals go: straight into `app.slice`, a scope of
/// their own at the default weight, so a shell the user is typing in ranks
/// with Chrome and not behind it. What the user runs there by hand is theirs;
/// only the agents' work is behind everything else.
pub const TERMINALS_SLICE: &str = "app.slice";

/// The slice's CPU weight: a fifth of one of the user's apps (100), and a
/// twenty-fifth of the daemon's unit (500). Under contention the order is
/// bridge, the user's apps, then every agent together.
pub const AGENTS_SLICE_CPU_WEIGHT: u32 = 20;

/// How far below the daemon's own nice its agents run: ten levels, which
/// makes a runnable agent thread weigh a tenth of a runnable daemon thread,
/// and which a default user may set without `RLIMIT_NICE`. Relative to the
/// daemon's own nice rather than an absolute ten, so a daemon somebody runs
/// niced already still keeps its agents behind it (as far as 19 allows).
pub const CHILD_NICE: i32 = 10;

/// The highest nice Linux and macOS have.
const MAX_NICE: i32 = 19;

/// The share of physical memory the agents' slice may hold before the kernel
/// reclaims from it first: three quarters. What remains is the desktop's and
/// the daemon's, which stay out of swap while the agents build.
const AGENTS_MEMORY_HIGH_NUMERATOR: u64 = 3;
const AGENTS_MEMORY_HIGH_DENOMINATOR: u64 = 4;

/// `0` turns the scope off (the child is niced only); anything else, or unset,
/// probes for the user's systemd and uses it when it answers.
pub const CHILD_SCOPE_ENV: &str = "BRIDGE_CHILD_SCOPE";

/// How long the startup probe waits for `systemd-run` before deciding the bus
/// is not answering. A working manager answers in milliseconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Which of the daemon's two kinds of child a spawn is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChildKind {
    /// An agent's harness: behind the daemon and the user's apps, niced.
    #[default]
    Agent,
    /// The user's own shell: beside the user's apps, never niced.
    Terminal,
}

impl ChildKind {
    fn slice(self) -> &'static str {
        match self {
            ChildKind::Agent => AGENTS_SLICE,
            ChildKind::Terminal => TERMINALS_SLICE,
        }
    }

    fn niced(self) -> bool {
        matches!(self, ChildKind::Agent)
    }
}

/// How this process places the children it spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildPlacement {
    /// `systemd-run --user --scope` into the kind's slice, and the nice.
    TransientScope { systemd_run: PathBuf },
    /// The nice alone: there is no user systemd to ask, or it was turned off.
    NiceOnly,
}

static CURRENT: Mutex<Option<ChildPlacement>> = Mutex::new(None);
static DEMOTED: Once = Once::new();

impl ChildPlacement {
    /// Decide once, at daemon start: `BRIDGE_CHILD_SCOPE=0` is the nice alone;
    /// otherwise the user's systemd is asked for a throwaway scope and used
    /// when it answers. `reason` is what to say in the log about the choice.
    pub fn resolve(lookup: impl Fn(&str) -> Option<String>) -> (ChildPlacement, String) {
        if lookup(CHILD_SCOPE_ENV).as_deref().map(str::trim) == Some("0") {
            return (ChildPlacement::NiceOnly, format!("{CHILD_SCOPE_ENV}=0"));
        }
        let Some(systemd_run) = find_on_path("systemd-run", lookup("PATH").as_deref()) else {
            return (ChildPlacement::NiceOnly, "no systemd-run on PATH".into());
        };
        match probe_scope(&systemd_run) {
            Ok(()) => (
                ChildPlacement::TransientScope { systemd_run },
                format!("transient scopes under {AGENTS_SLICE}"),
            ),
            Err(reason) => (ChildPlacement::NiceOnly, reason),
        }
    }

    /// Make this the placement every spawn in the process uses. The daemon
    /// installs once at start; a test that never installs one gets
    /// [`ChildPlacement::NiceOnly`], so no test ever asks the machine's
    /// systemd for a scope.
    pub fn install(self) {
        *CURRENT.lock().unwrap() = Some(self);
    }

    /// The placement in force.
    pub fn current() -> ChildPlacement {
        CURRENT
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(ChildPlacement::NiceOnly)
    }

    /// The scope stopped working after the probe said it would: `systemd-run`
    /// itself died at a spawn. Every later spawn is niced only, and the fact
    /// is said once, because a spawn that had to be redone is not a spawn that
    /// failed and nobody should learn about it from a silent change in `top`.
    pub fn demote(reason: &str) {
        *CURRENT.lock().unwrap() = Some(ChildPlacement::NiceOnly);
        DEMOTED.call_once(|| {
            crate::logline::say(format!(
                "children: {reason}; from now on nice {CHILD_NICE} only, no scope"
            ));
        });
    }

    /// The program and arguments that start `binary args` under this
    /// placement, for a child of this kind. `binary` is already resolved to
    /// a path: `systemd-run` searches nothing for us.
    pub fn command(
        &self,
        kind: ChildKind,
        binary: &Path,
        args: &[String],
    ) -> (PathBuf, Vec<String>) {
        match self {
            ChildPlacement::NiceOnly => (binary.to_path_buf(), args.to_vec()),
            ChildPlacement::TransientScope { systemd_run } => {
                let mut argv = scope_arguments(kind.slice());
                argv.push("--".into());
                argv.push(binary.to_string_lossy().into_owned());
                argv.extend(args.iter().cloned());
                (systemd_run.clone(), argv)
            }
        }
    }

    /// Lower the child that was just spawned, if its kind is lowered at all.
    /// Its own process first, which exists the moment `spawn` returns, then
    /// its process group, which covers anything it forked before this ran. A
    /// child already gone is not an error worth a word.
    pub fn lower(&self, kind: ChildKind, pid: u32) {
        if !kind.niced() {
            return;
        }
        lower_process(pid);
        lower_process_group(pid);
    }

    /// Whether children leave the daemon's cgroup.
    pub fn is_scoped(&self) -> bool {
        matches!(self, ChildPlacement::TransientScope { .. })
    }
}

/// `systemd-run`'s arguments up to, not including, the `--` and the command.
fn scope_arguments(slice: &str) -> Vec<String> {
    vec![
        "--user".into(),
        "--scope".into(),
        "--quiet".into(),
        "--collect".into(),
        format!("--slice={slice}"),
    ]
}

/// This process's own nice.
pub fn own_nice() -> i32 {
    // SAFETY: getpriority reads no memory and writes none. Its -1 is both an
    // answer and an error, told apart by errno, which is cleared first.
    unsafe {
        *libc::__errno_location() = 0;
        let nice = libc::getpriority(libc::PRIO_PROCESS, 0);
        if nice == -1 && *libc::__errno_location() != 0 {
            0
        } else {
            nice
        }
    }
}

/// The nice an agent child gets under a daemon at `daemon_nice`.
pub fn child_nice_for(daemon_nice: i32) -> i32 {
    (daemon_nice + CHILD_NICE).min(MAX_NICE)
}

fn lower_process(pid: u32) {
    // SAFETY: setpriority reads no memory and writes none; a pid that no
    // longer exists is answered with ESRCH, which is ignored.
    unsafe {
        libc::setpriority(
            libc::PRIO_PROCESS,
            pid as libc::id_t,
            child_nice_for(own_nice()),
        );
    }
}

fn lower_process_group(pid: u32) {
    // SAFETY: as above, for the group the PTY child leads once it has called
    // setsid; before that the group does not exist and the call is a no-op.
    unsafe {
        libc::setpriority(
            libc::PRIO_PGRP,
            pid as libc::id_t,
            child_nice_for(own_nice()),
        );
    }
}

/// The `MemoryHigh` for the agents' slice on a machine with this much RAM:
/// three quarters of it, rounded down to a whole mebibyte, which is the unit
/// the property is written in.
pub fn agents_memory_high(mem_total_bytes: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    (mem_total_bytes / AGENTS_MEMORY_HIGH_DENOMINATOR * AGENTS_MEMORY_HIGH_NUMERATOR) / MIB * MIB
}

/// `MemTotal` as `/proc/meminfo` reports it, in bytes. `None` off Linux or
/// when the file cannot be read.
pub fn physical_memory_bytes() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    mem_total_from_meminfo(&meminfo)
}

fn mem_total_from_meminfo(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// The `systemctl set-property` arguments that shape the agents' slice, the
/// same ones `install-service` writes persistently. `runtime` makes them last
/// until the next boot only, which is what the daemon itself applies at every
/// start so a rolled binary on an old install is covered before anyone re-runs
/// the installer.
pub fn slice_property_arguments(mem_total_bytes: Option<u64>, runtime: bool) -> Vec<String> {
    let mut argv = vec!["--user".to_string(), "set-property".to_string()];
    if runtime {
        argv.push("--runtime".into());
    }
    argv.push(AGENTS_SLICE.into());
    argv.push(format!("CPUWeight={AGENTS_SLICE_CPU_WEIGHT}"));
    if let Some(total) = mem_total_bytes {
        argv.push(format!(
            "MemoryHigh={}M",
            agents_memory_high(total) / (1024 * 1024)
        ));
    }
    argv
}

/// Apply the slice's properties for this boot. Best effort: a refusal is
/// reported, not fatal, because the children are still scoped and niced.
pub fn apply_slice_properties_for_this_boot(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let systemctl = find_on_path("systemctl", lookup("PATH").as_deref())
        .ok_or_else(|| "no systemctl on PATH".to_string())?;
    let argv = slice_property_arguments(physical_memory_bytes(), true);
    let mut command = Command::new(&systemctl);
    command
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match run_within(command, PROBE_TIMEOUT)? {
        Ok(()) => Ok(argv[3..].join(" ")),
        Err(stderr) => Err(format!("systemctl {}: {stderr}", argv.join(" "))),
    }
}

/// Ask the user's systemd for one throwaway scope. What every real spawn will
/// do, done once with `sh -c :` so a manager that cannot answer is found out
/// here, at startup, rather than by the first agent's PTY dying at birth.
fn probe_scope(systemd_run: &Path) -> Result<(), String> {
    let mut command = Command::new(systemd_run);
    command
        .args(scope_arguments(AGENTS_SLICE))
        .args(["--", "sh", "-c", ":"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    match run_within(command, PROBE_TIMEOUT)? {
        Ok(()) => Ok(()),
        Err(stderr) => Err(format!("systemd-run refused a scope: {stderr}")),
    }
}

/// Run `command` to completion within `timeout`. `Ok(Ok(()))` on a zero exit,
/// `Ok(Err(stderr))` on a non-zero one, `Err` when it could not be run or did
/// not finish in time (it is killed).
fn run_within(mut command: Command, timeout: Duration) -> Result<Result<(), String>, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run {:?}: {error}", command.get_program()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    let _ = pipe.read_to_string(&mut stderr);
                }
                return Ok(if status.success() {
                    Ok(())
                } else {
                    Err(stderr.trim().to_string())
                });
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{:?} did not finish within {}s",
                    command.get_program(),
                    timeout.as_secs()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "cannot wait for {:?}: {error}",
                    command.get_program()
                ))
            }
        }
    }
}

fn find_on_path(program: &str, path: Option<&str>) -> Option<PathBuf> {
    let path = path
        .map(str::to_string)
        .or_else(|| std::env::var("PATH").ok())?;
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(candidate)
                .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scoped_child_is_started_by_systemd_run_in_the_agents_slice() {
        let placement = ChildPlacement::TransientScope {
            systemd_run: PathBuf::from("/usr/bin/systemd-run"),
        };
        let (program, argv) = placement.command(
            ChildKind::Agent,
            Path::new("/usr/bin/claude"),
            &["--dangerously-skip-permissions".to_string()],
        );
        assert_eq!(program, PathBuf::from("/usr/bin/systemd-run"));
        assert_eq!(
            argv,
            vec![
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=app-build_agents.slice",
                "--",
                "/usr/bin/claude",
                "--dangerously-skip-permissions",
            ]
        );
    }

    #[test]
    fn the_user_s_terminal_is_scoped_beside_the_user_s_apps_not_behind_them() {
        let placement = ChildPlacement::TransientScope {
            systemd_run: PathBuf::from("/usr/bin/systemd-run"),
        };
        let (_, argv) = placement.command(ChildKind::Terminal, Path::new("/bin/zsh"), &[]);
        assert!(argv.contains(&"--slice=app.slice".to_string()), "{argv:?}");
        assert!(!argv.iter().any(|arg| arg.contains("agents")), "{argv:?}");
    }

    #[test]
    fn a_niced_only_child_is_started_as_itself() {
        let (program, argv) = ChildPlacement::NiceOnly.command(
            ChildKind::Agent,
            Path::new("/usr/bin/codex"),
            &["exec".to_string()],
        );
        assert_eq!(program, PathBuf::from("/usr/bin/codex"));
        assert_eq!(argv, vec!["exec"]);
    }

    #[test]
    fn the_scope_is_off_when_the_environment_says_so() {
        let (placement, reason) = ChildPlacement::resolve(|key| match key {
            CHILD_SCOPE_ENV => Some("0".into()),
            _ => None,
        });
        assert_eq!(placement, ChildPlacement::NiceOnly);
        assert_eq!(reason, "BRIDGE_CHILD_SCOPE=0");
    }

    #[test]
    fn no_systemd_run_on_path_means_the_nice_alone() {
        let (placement, reason) = ChildPlacement::resolve(|key| match key {
            "PATH" => Some("/nonexistent".into()),
            _ => None,
        });
        assert_eq!(placement, ChildPlacement::NiceOnly);
        assert_eq!(reason, "no systemd-run on PATH");
    }

    #[test]
    fn the_agents_memory_ceiling_is_three_quarters_of_ram_in_whole_mebibytes() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(agents_memory_high(128 * GIB), 96 * GIB);
        assert_eq!(agents_memory_high(16 * GIB), 12 * GIB);
        // 1 GiB + 3 bytes: the odd bytes round away, never up.
        assert_eq!(agents_memory_high(GIB + 3), 768 * 1024 * 1024);
    }

    #[test]
    fn mem_total_is_read_off_meminfo_in_bytes() {
        let meminfo = "MemFree:        9160800 kB\nMemTotal:       131781396 kB\n";
        assert_eq!(mem_total_from_meminfo(meminfo), Some(131_781_396 * 1024));
        assert_eq!(mem_total_from_meminfo("nothing here"), None);
    }

    #[test]
    fn the_slice_properties_name_the_weight_and_the_ceiling() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(
            slice_property_arguments(Some(128 * GIB), true),
            vec![
                "--user",
                "set-property",
                "--runtime",
                "app-build_agents.slice",
                "CPUWeight=20",
                "MemoryHigh=98304M",
            ]
        );
        assert_eq!(
            slice_property_arguments(None, false),
            vec![
                "--user",
                "set-property",
                "app-build_agents.slice",
                "CPUWeight=20"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lowering_a_child_puts_its_nice_ten_below_the_daemon() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("sh spawns");
        ChildPlacement::NiceOnly.lower(ChildKind::Agent, child.id());
        let nice = nice_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        // Relative to this very process, which the test runner may itself run
        // niced (the gates run under `nice -n 10`).
        assert_eq!(nice, child_nice_for(own_nice()));
    }

    #[test]
    fn the_child_nice_is_ten_below_the_daemon_and_never_past_nineteen() {
        assert_eq!(child_nice_for(0), 10);
        assert_eq!(child_nice_for(10), 19);
        assert_eq!(child_nice_for(-5), 5);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_terminal_is_never_lowered() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("sh spawns");
        ChildPlacement::NiceOnly.lower(ChildKind::Terminal, child.id());
        let nice = nice_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            nice,
            own_nice(),
            "a terminal inherits the daemon's own nice"
        );
    }

    /// Field 19 of `/proc/<pid>/stat` is the nice value; the command name in
    /// field 2 may hold spaces, so count from after its closing paren.
    #[cfg(target_os = "linux")]
    fn nice_of(pid: u32) -> i32 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("stat");
        let after_comm = stat.rsplit_once(')').expect("comm").1;
        after_comm
            .split_whitespace()
            .nth(16)
            .expect("nice")
            .parse()
            .unwrap()
    }
}
