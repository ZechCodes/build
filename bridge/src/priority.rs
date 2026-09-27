//! Where the daemon's children run, and behind whom.
//!
//! Every agent the bridge starts, headless or in a terminal, and every
//! terminal the user opens, is a child of this process, and everything they
//! run (cargo, vitest, a headless Chromium) is a grandchild. On Linux that
//! puts all of it inside the daemon's own cgroup, and a `CPUWeight` on the
//! service unit, which only arbitrates between sibling cgroups, can never
//! keep the daemon ahead of its own children. On 2026-09-24 the user's phone
//! lost its session every few seconds while ten vitest runs and a headless
//! Chromium, all agents' work, ran at the bridge's own priority inside the
//! bridge's own cgroup (task #128).
//!
//! So the children are moved out, and every child goes the same way whatever
//! spawns it: a PTY, a pipe, an agent's harness or the user's shell. The child
//! is started **once**, at a gate: `/bin/sh` waiting on a FIFO for one line,
//! after which it becomes the command it was given. While it waits, forked and
//! not yet anything, the daemon decides where it runs: where the user's
//! systemd is reachable, the child's pid is put into a transient scope of its
//! own under [`AGENTS_SLICE`] (or beside the user's apps, for a terminal) by
//! one D-Bus call, the move is verified in `/proc`, and the scope is bound to
//! the daemon's own unit so it stops when the daemon does; where it is not
//! reachable, on macOS, in a container, over a broken bus, the child is niced
//! instead, which is the one lever that works inside a cgroup. Then the line
//! is written and the child runs, exactly once, in exactly one place. A scope
//! that could not be had costs the child nothing but the scope: the fallback
//! is taken for that spawn, said once in the log, and kept from then on.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// How far below the daemon the user's own terminal runs where there is no
/// scope to rank it in: half an agent's step, so the order bridge, then the
/// user's shell, then the agents holds by nice alone. Under a scope the
/// terminal is not niced at all; its scope beside the user's apps is its rank.
pub const TERMINAL_NICE_WITHOUT_SCOPE: i32 = 5;

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

/// How long the startup probe and the slice's `set-property` wait for the
/// manager before deciding it is not answering. A working manager answers in
/// milliseconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The shell every child waits at, by its POSIX path: what it runs is one
/// builtin and one `exec`, so nothing it does depends on the child's `PATH`.
const GATE_SHELL: &str = "/bin/sh";

/// What the shell at the gate runs. `$0` is the FIFO, the rest is the command:
/// one line read off the FIFO, then the shell becomes the command. `read` is a
/// builtin and `exec` replaces the shell, so between the fork and the daemon's
/// line nothing runs and nothing is forked — the pid the daemon places is the
/// only process there is, and every process the command ever makes descends
/// from it, in its scope and at its nice.
const GATE_SCRIPT: &str = "read -r _ <\"$0\" && exec \"$@\"";

/// How long a child has to reach its gate. A shell reaches it in a
/// millisecond; this outlasts a host so loaded that forking takes seconds,
/// and a child that is still not there is killed rather than left waiting for
/// a line that would never be read.
const GATE_ARRIVAL: Duration = Duration::from_secs(60);

/// The two waits a scope involves, and their defaults: the manager answering
/// the call that starts the scope, and the pid then showing up inside it (the
/// call returns when the job is queued, not when it has run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeDeadlines {
    pub call: Duration,
    pub arrival: Duration,
}

impl Default for ScopeDeadlines {
    fn default() -> Self {
        ScopeDeadlines {
            call: Duration::from_secs(5),
            arrival: Duration::from_secs(3),
        }
    }
}

/// How long stopping every scope at shutdown may take, all of them in one
/// `systemctl` call. Best effort behind the `BindsTo=` that stops them anyway.
const SCOPE_STOP: Duration = Duration::from_secs(15);

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

    fn word(self) -> &'static str {
        match self {
            ChildKind::Agent => "agent",
            ChildKind::Terminal => "terminal",
        }
    }

    /// How far below the daemon a child of this kind runs, given whether a
    /// scope already ranks it. `None` is not lowered.
    fn nice_step(self, scoped: bool) -> Option<i32> {
        match (self, scoped) {
            (ChildKind::Agent, _) => Some(CHILD_NICE),
            (ChildKind::Terminal, true) => None,
            (ChildKind::Terminal, false) => Some(TERMINAL_NICE_WITHOUT_SCOPE),
        }
    }
}

/// How this process places the children it spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildPlacement {
    /// A transient scope per child, asked of the user's systemd over the bus
    /// by `busctl`, in the kind's slice; agents niced as well.
    TransientScope {
        busctl: PathBuf,
        /// `systemctl`, for stopping every scope at shutdown. `None` leaves
        /// that to the `BindsTo=` each scope carries.
        systemctl: Option<PathBuf>,
        /// The daemon's own unit, which every scope is bound to so it ends
        /// when the daemon does. `None` when the daemon runs outside one.
        bound_to: Option<String>,
        deadlines: ScopeDeadlines,
    },
    /// The nice alone: there is no user systemd to ask, or it was turned off,
    /// or it stopped answering.
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
        let path = lookup("PATH");
        let Some(busctl) = find_on_path("busctl", path.as_deref()) else {
            return (ChildPlacement::NiceOnly, "no busctl on PATH".into());
        };
        let placement = ChildPlacement::TransientScope {
            busctl,
            systemctl: find_on_path("systemctl", path.as_deref()),
            bound_to: own_service_unit(),
            deadlines: ScopeDeadlines::default(),
        };
        match placement.probe() {
            Ok(()) => {
                let reason = placement.describe();
                (placement, reason)
            }
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

    /// The scope stopped working after the probe said it would. Every later
    /// spawn is niced only, and the fact is said once, because a child that
    /// silently lands in the daemon's own cgroup is the bug this module
    /// exists to end and nobody should learn about it from `top`.
    pub fn demote(reason: &str) {
        *CURRENT.lock().unwrap() = Some(ChildPlacement::NiceOnly);
        DEMOTED.call_once(|| {
            crate::logline::say(format!(
                "children: {reason}; from now on nice {CHILD_NICE} only, no scope"
            ));
        });
    }

    /// Whether children leave the daemon's cgroup.
    pub fn is_scoped(&self) -> bool {
        matches!(self, ChildPlacement::TransientScope { .. })
    }

    /// One line on what this placement does, for the startup log.
    pub fn describe(&self) -> String {
        match self {
            ChildPlacement::NiceOnly => "no scope".to_string(),
            ChildPlacement::TransientScope { bound_to, .. } => match bound_to {
                Some(unit) => format!("transient scopes under {AGENTS_SLICE}, bound to {unit}"),
                None => format!("transient scopes under {AGENTS_SLICE}, bound to no unit"),
            },
        }
    }

    /// Start `binary args` as a child of this kind, spawned by `std::process`
    /// and placed here: what the headless harnesses do. `configure` sets what
    /// the caller needs on the command (environment, directory, pipes); the
    /// program and its first arguments are the gate's.
    pub fn spawn_command(
        &self,
        kind: ChildKind,
        binary: &Path,
        args: &[String],
        configure: impl FnOnce(&mut Command),
    ) -> io::Result<std::process::Child> {
        let launch = Launch::gated(binary, args)?;
        let mut command = Command::new(&launch.program);
        command.args(&launch.args);
        configure(&mut command);
        let mut child = command.spawn()?;
        let pid = child.id();
        let placed = self.place(launch.gate, kind, pid, &mut || {
            matches!(child.try_wait(), Ok(None))
        });
        match placed.released {
            Ok(()) => Ok(child),
            Err(reason) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(io::Error::other(reason))
            }
        }
    }

    /// Place the child waiting at `gate`, then let it run. Never fails the
    /// spawn on the scope's account: a scope that could not be had is the
    /// nice alone for this child, and [`demote`](Self::demote) for the next.
    /// `alive` says whether the child is still there, so a child that died
    /// at the gate is not waited for.
    pub fn place(
        &self,
        gate: Gate,
        kind: ChildKind,
        pid: u32,
        alive: &mut dyn FnMut() -> bool,
    ) -> Placed {
        let scope = match self {
            ChildPlacement::NiceOnly => None,
            ChildPlacement::TransientScope { .. } => match self.start_scope(kind, pid) {
                Ok(name) => Some(name),
                Err(reason) => {
                    ChildPlacement::demote(&reason);
                    None
                }
            },
        };
        if let Some(step) = kind.nice_step(scope.is_some()) {
            lower_process(pid, child_nice_for(own_nice(), step));
        }
        let released = gate.release(pid, alive);
        if let Some(name) = &scope {
            SCOPES.lock().unwrap().push((name.clone(), pid));
        }
        Placed { scope, released }
    }

    /// Ask the manager for a scope holding `pid`, and wait until the pid is
    /// in it.
    fn start_scope(&self, kind: ChildKind, pid: u32) -> Result<String, String> {
        let ChildPlacement::TransientScope {
            busctl,
            bound_to,
            deadlines,
            ..
        } = self
        else {
            return Err("no scope placement".into());
        };
        let name = next_scope_name(kind);
        let argv = start_transient_unit_arguments(&name, kind, pid, bound_to.as_deref());
        let mut command = Command::new(busctl);
        command
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        match run_within(command, deadlines.call)? {
            Ok(()) => {}
            Err(stderr) => {
                return Err(format!(
                    "the manager refused a scope for pid {pid}: {stderr}"
                ))
            }
        }
        wait_for_scope(pid, &name, deadlines.arrival)?;
        Ok(name)
    }

    /// Ask the manager for one throwaway scope around a child that does
    /// nothing, exactly the way every real spawn will, so a manager that
    /// cannot answer is found out here, at startup, rather than by the first
    /// agent.
    fn probe(&self) -> Result<(), String> {
        let mut child = Command::new(GATE_SHELL)
            .args(["-c", "sleep 5"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot start the probe child: {error}"))?;
        let probed = self.start_scope(ChildKind::Agent, child.id());
        let _ = child.kill();
        let _ = child.wait();
        let name = probed?;
        SCOPES.lock().unwrap().retain(|(scope, _)| scope != &name);
        Ok(())
    }
}

/// What placing a child came to.
#[derive(Debug)]
pub struct Placed {
    /// The scope the child runs in, when it got one.
    pub scope: Option<String>,
    /// Whether the child was let through its gate. `Err` is a child that never
    /// reached it, or died at it: the spawn did not happen.
    pub released: Result<(), String>,
}

/// What to spawn so that the child waits at a gate: the shell, its
/// arguments (the FIFO, then the real command), and the gate to release it
/// through once it is placed.
#[derive(Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub gate: Gate,
}

impl Launch {
    /// The gated form of `binary args`. `binary` is already resolved to a
    /// path: the shell at the gate searches nothing. A binary that is not
    /// there, or not executable, is refused here, at once, the way a direct
    /// spawn would have refused it — not by a shell exiting 127 at the gate
    /// after the spawn was reported a success.
    pub fn gated(binary: &Path, args: &[String]) -> io::Result<Launch> {
        executable(binary)?;
        let gate = Gate::open()?;
        let mut argv = vec![
            "-c".to_string(),
            GATE_SCRIPT.to_string(),
            gate.fifo.to_string_lossy().into_owned(),
            binary.to_string_lossy().into_owned(),
        ];
        argv.extend(args.iter().cloned());
        Ok(Launch {
            program: PathBuf::from(GATE_SHELL),
            args: argv,
            gate,
        })
    }
}

/// `Ok` if `binary` is a file this process may execute; the error a direct
/// spawn of it would have reported otherwise.
fn executable(binary: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(binary)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", binary.display())))?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: not a regular file", binary.display()),
        ));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{}: not executable", binary.display()),
        ));
    }
    Ok(())
}

/// One child's gate: the FIFO its shell waits on. Removed when the child is
/// released, and when the gate is dropped unused.
#[derive(Debug)]
pub struct Gate {
    fifo: PathBuf,
}

static GATES: AtomicU64 = AtomicU64::new(0);

impl Gate {
    fn open() -> io::Result<Gate> {
        let dir = gates_dir()?;
        let fifo = dir.join(format!(
            "gate-{}-{}",
            std::process::id(),
            GATES.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&fifo);
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::other("the gate's path holds a NUL byte"))?;
        // SAFETY: mkfifo reads the path and writes nothing.
        if unsafe { libc::mkfifo(path.as_ptr(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Gate { fifo })
    }

    /// Where the shell is waiting.
    pub fn path(&self) -> &Path {
        &self.fifo
    }

    /// Write the line the child is waiting for. Opening the FIFO for writing
    /// succeeds only once the child holds it open for reading, so the open
    /// itself is the wait for the child to reach the gate: until then it is
    /// retried while the child lives, and a child that is still not there
    /// after [`GATE_ARRIVAL`] is killed rather than left waiting forever.
    fn release(self, pid: u32, alive: &mut dyn FnMut() -> bool) -> Result<(), String> {
        use std::io::Write;
        let started = Instant::now();
        let mut writer = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.fifo)
            {
                Ok(writer) => break writer,
                Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                    if !alive() {
                        return Err("the child died before it reached its gate".into());
                    }
                    if started.elapsed() > GATE_ARRIVAL {
                        // SAFETY: kill sends a signal and touches no memory.
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGKILL);
                        }
                        return Err(format!(
                            "the child did not reach its gate within {}s",
                            GATE_ARRIVAL.as_secs()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(format!("cannot open the gate: {error}")),
            }
        };
        writer
            .write_all(b"go\n")
            .map_err(|error| format!("cannot write the gate's line: {error}"))?;
        Ok(())
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.fifo);
    }
}

/// The directory the gates' FIFOs live in: the user's runtime directory when
/// there is one, else a directory of this user's own under the temporary
/// directory. Private to the user either way.
fn gates_dir() -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(runtime) if !runtime.is_empty() => PathBuf::from(runtime).join("build-bridge"),
        _ => {
            // SAFETY: getuid reads nothing and cannot fail.
            let uid = unsafe { libc::getuid() };
            std::env::temp_dir().join(format!("build-bridge-{uid}"))
        }
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(&dir)?;
    Ok(dir)
}

use std::os::unix::fs::OpenOptionsExt;

static SCOPE_NAMES: AtomicU64 = AtomicU64::new(0);

/// A name for the next scope: this daemon's pid and a count, so two daemons on
/// one machine (the user's and a test's) never ask for the same unit.
fn next_scope_name(kind: ChildKind) -> String {
    format!(
        "build-{}-p{}-{}.scope",
        kind.word(),
        std::process::id(),
        SCOPE_NAMES.fetch_add(1, Ordering::Relaxed)
    )
}

/// The `busctl` call that starts a scope named `name` holding `pid` in the
/// kind's slice: the manager's `StartTransientUnit`, with the properties
/// `systemd-run --scope --collect --slice=…` would set, and, when the daemon
/// has a unit, `BindsTo=`/`After=` on it so the scope is stopped when the
/// daemon's unit is — the `KillMode=control-group` the children had while
/// they lived inside it, kept now that they do not.
pub fn start_transient_unit_arguments(
    name: &str,
    kind: ChildKind,
    pid: u32,
    bound_to: Option<&str>,
) -> Vec<String> {
    let mut properties: Vec<Vec<String>> = vec![
        vec!["PIDs".into(), "au".into(), "1".into(), pid.to_string()],
        vec!["Slice".into(), "s".into(), kind.slice().into()],
        vec![
            "CollectMode".into(),
            "s".into(),
            "inactive-or-failed".into(),
        ],
        vec![
            "Description".into(),
            "s".into(),
            format!("Build {} (bridge pid {})", kind.word(), std::process::id()),
        ],
    ];
    if let Some(unit) = bound_to {
        properties.push(vec!["BindsTo".into(), "as".into(), "1".into(), unit.into()]);
        properties.push(vec!["After".into(), "as".into(), "1".into(), unit.into()]);
    }
    let mut argv: Vec<String> = vec![
        "--user".into(),
        "call".into(),
        "org.freedesktop.systemd1".into(),
        "/org/freedesktop/systemd1".into(),
        "org.freedesktop.systemd1.Manager".into(),
        "StartTransientUnit".into(),
        "ssa(sv)a(sa(sv))".into(),
        name.into(),
        "fail".into(),
        properties.len().to_string(),
    ];
    for property in properties {
        argv.extend(property);
    }
    argv.push("0".into());
    argv
}

/// Wait until `pid` is inside the scope `name`, as `/proc` tells it.
fn wait_for_scope(pid: u32, name: &str, within: Duration) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match unit_of_pid(pid) {
            Some(unit) if unit == name => return Ok(()),
            _ if started.elapsed() > within => {
                return Err(format!(
                    "pid {pid} did not arrive in {name} within {} ms",
                    within.as_millis()
                ))
            }
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// The unit `pid` runs in, off its cgroup path. `None` off Linux, or when the
/// process is gone.
fn unit_of_pid(pid: u32) -> Option<String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    unit_of_cgroup(&cgroup)
}

/// The last component of the unified cgroup path in `/proc/<pid>/cgroup`.
fn unit_of_cgroup(cgroup: &str) -> Option<String> {
    let path = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .trim();
    let unit = path.rsplit('/').next()?;
    if unit.is_empty() {
        None
    } else {
        Some(unit.to_string())
    }
}

/// The service unit this daemon runs as, if it runs as one: what every scope
/// is bound to.
fn own_service_unit() -> Option<String> {
    service_unit_of_cgroup(&std::fs::read_to_string("/proc/self/cgroup").ok()?)
}

fn service_unit_of_cgroup(cgroup: &str) -> Option<String> {
    unit_of_cgroup(cgroup).filter(|unit| unit.ends_with(".service"))
}

/// Every scope this daemon started, with the pid it holds: what the shutdown
/// stops. Pruned of the dead each time one is added.
static SCOPES: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());

/// Stop every scope this daemon started and that still holds a live child:
/// the daemon is going down, and its children go with it as they did when
/// they shared its cgroup. Belt to the `BindsTo=` braces. `None` when there
/// is nothing to stop; otherwise a line for the log.
pub fn stop_children() -> Option<String> {
    let scopes: Vec<String> = {
        let mut scopes = SCOPES.lock().unwrap();
        scopes.retain(|(_, pid)| process_exists(*pid));
        scopes.iter().map(|(name, _)| name.clone()).collect()
    };
    if scopes.is_empty() {
        return None;
    }
    let ChildPlacement::TransientScope {
        systemctl: Some(systemctl),
        ..
    } = ChildPlacement::current()
    else {
        return Some(format!(
            "children: {} scopes left to the manager's BindsTo=",
            scopes.len()
        ));
    };
    let mut command = Command::new(systemctl);
    command
        .args(["--user", "stop", "--no-block"])
        .args(&scopes)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Some(match run_within(command, SCOPE_STOP) {
        Ok(Ok(())) => format!("children: stopped {} scopes", scopes.len()),
        Ok(Err(stderr)) => format!("children: stopping {} scopes: {stderr}", scopes.len()),
        Err(error) => format!("children: stopping {} scopes: {error}", scopes.len()),
    })
}

fn process_exists(pid: u32) -> bool {
    // SAFETY: a signal of 0 is a probe: nothing is sent, and nothing written.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || errno() != libc::ESRCH }
}

/// This process's own nice.
pub fn own_nice() -> i32 {
    // SAFETY: getpriority reads no memory and writes none. Its -1 is both an
    // answer and an error, told apart by errno, which is cleared first.
    unsafe {
        clear_errno();
        let nice = libc::getpriority(libc::PRIO_PROCESS, 0);
        if nice == -1 && errno() != 0 {
            0
        } else {
            nice
        }
    }
}

/// The calling thread's errno.
fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Where the calling thread's errno lives, by the name each libc gives it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: the pointer is thread-local and lives as long as the thread.
    unsafe { libc::__errno_location() }
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: as above.
    unsafe { libc::__error() }
}

#[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
fn errno_location() -> *mut libc::c_int {
    // SAFETY: as above.
    unsafe { libc::__errno() }
}

fn clear_errno() {
    // SAFETY: the location is this thread's own.
    unsafe {
        *errno_location() = 0;
    }
}

/// The nice a child `step` below a daemon at `daemon_nice` gets.
pub fn child_nice_for(daemon_nice: i32, step: i32) -> i32 {
    (daemon_nice + step).min(MAX_NICE)
}

/// Lower one process. Its descendants inherit the nice at every fork, and a
/// child lowered at its gate has forked nothing yet, so this covers
/// everything it will ever run. A pid that no longer exists is answered
/// with ESRCH, which is ignored.
fn lower_process(pid: u32, nice: i32) {
    // SAFETY: setpriority reads no memory and writes none.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice);
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
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{:?} did not finish within {} ms",
                    command.get_program(),
                    timeout.as_millis()
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
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_scope_is_asked_of_the_manager_with_the_child_s_pid_in_the_kind_s_slice() {
        let argv = start_transient_unit_arguments(
            "build-agent-p1-0.scope",
            ChildKind::Agent,
            4242,
            Some("build-bridge.service"),
        );
        let joined = argv.join(" ");
        assert!(
            joined.starts_with(
                "--user call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
             org.freedesktop.systemd1.Manager StartTransientUnit ssa(sv)a(sa(sv)) \
             build-agent-p1-0.scope fail 6 PIDs au 1 4242 Slice s app-build_agents.slice \
             CollectMode s inactive-or-failed Description s Build agent"
            ),
            "{joined}"
        );
        assert!(
            joined.ends_with("BindsTo as 1 build-bridge.service After as 1 build-bridge.service 0"),
            "{joined}"
        );
    }

    #[test]
    fn a_terminal_s_scope_is_beside_the_user_s_apps_and_bound_to_nothing_outside_a_unit() {
        let argv = start_transient_unit_arguments(
            "build-terminal-p1-1.scope",
            ChildKind::Terminal,
            7,
            None,
        );
        assert!(argv.contains(&"app.slice".to_string()), "{argv:?}");
        assert!(!argv.iter().any(|arg| arg.contains("agents")), "{argv:?}");
        assert!(!argv.contains(&"BindsTo".to_string()), "{argv:?}");
        assert_eq!(
            argv[9], "4",
            "four properties without the binding: {argv:?}"
        );
        assert_eq!(argv.last().map(String::as_str), Some("0"));
    }

    #[test]
    fn the_unit_is_the_last_component_of_the_unified_cgroup_path() {
        let cgroup =
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/build-bridge.service\n";
        assert_eq!(
            unit_of_cgroup(cgroup).as_deref(),
            Some("build-bridge.service")
        );
        assert_eq!(
            service_unit_of_cgroup(cgroup).as_deref(),
            Some("build-bridge.service")
        );
        let scoped = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-build_agents.slice/build-agent-p9-3.scope\n";
        assert_eq!(
            unit_of_cgroup(scoped).as_deref(),
            Some("build-agent-p9-3.scope")
        );
        assert_eq!(
            service_unit_of_cgroup(scoped),
            None,
            "a scope is not a service"
        );
        assert_eq!(unit_of_cgroup("0::/\n"), None, "the root is no unit");
        assert_eq!(
            unit_of_cgroup("1:name=systemd:/foo.service\n"),
            None,
            "v1 lines are not read"
        );
    }

    #[test]
    fn the_gated_launch_is_the_shell_the_fifo_and_the_command() {
        let launch = Launch::gated(
            Path::new("/bin/sh"),
            &["--dangerously-skip-permissions".to_string()],
        )
        .unwrap();
        assert_eq!(launch.program, PathBuf::from("/bin/sh"));
        assert_eq!(launch.args[0], "-c");
        assert_eq!(launch.args[1], "read -r _ <\"$0\" && exec \"$@\"");
        assert_eq!(launch.args[2], launch.gate.path().to_string_lossy());
        assert_eq!(
            &launch.args[3..],
            ["/bin/sh", "--dangerously-skip-permissions"]
        );
        assert!(
            launch.gate.path().exists(),
            "the FIFO is there before the spawn"
        );
        let fifo = launch.gate.path().to_path_buf();
        drop(launch);
        assert!(!fifo.exists(), "an unused gate is removed with the launch");
    }

    /// A binary that is not there fails the launch before any gate exists,
    /// as a direct spawn of it would have — the app learns at once, not from
    /// a shell dying at the gate.
    #[test]
    fn a_missing_binary_is_refused_before_the_gate() {
        let refused = Launch::gated(Path::new("/definitely/missing/agent"), &[]).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::NotFound, "{refused}");
        assert!(
            refused.to_string().contains("/definitely/missing/agent"),
            "{refused}"
        );
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("not-executable");
        std::fs::write(&plain, "text").unwrap();
        let refused = Launch::gated(&plain, &[]).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
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
    fn no_busctl_on_path_means_the_nice_alone() {
        let (placement, reason) = ChildPlacement::resolve(|key| match key {
            "PATH" => Some("/nonexistent".into()),
            _ => None,
        });
        assert_eq!(placement, ChildPlacement::NiceOnly);
        assert_eq!(reason, "no busctl on PATH");
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

    #[test]
    fn the_child_nice_is_a_step_below_the_daemon_and_never_past_nineteen() {
        assert_eq!(child_nice_for(0, CHILD_NICE), 10);
        assert_eq!(child_nice_for(10, CHILD_NICE), 19);
        assert_eq!(child_nice_for(-5, CHILD_NICE), 5);
        assert_eq!(child_nice_for(0, TERMINAL_NICE_WITHOUT_SCOPE), 5);
    }

    /// Without a scope the order is kept by nice alone: bridge, then the
    /// user's shell, then the agents.
    #[test]
    fn without_a_scope_a_terminal_is_half_a_step_down_and_an_agent_a_whole_one() {
        assert_eq!(
            ChildKind::Terminal.nice_step(false),
            Some(TERMINAL_NICE_WITHOUT_SCOPE)
        );
        assert_eq!(ChildKind::Agent.nice_step(false), Some(CHILD_NICE));
        assert_eq!(
            ChildKind::Terminal.nice_step(true),
            None,
            "its scope is its rank"
        );
        assert_eq!(ChildKind::Agent.nice_step(true), Some(CHILD_NICE));
    }

    /// A child spawned through the gate that appends one line to `record`:
    /// how often it ran (one line per run) and at what nice.
    fn run_recording(placement: &ChildPlacement, kind: ChildKind, record: &Path) -> io::Result<()> {
        let script = format!("echo NICE=$(nice) >> '{}'", record.display());
        let mut child = placement.spawn_command(
            kind,
            Path::new("/bin/sh"),
            &["-c".to_string(), script],
            |command| {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
            },
        )?;
        child.wait()?;
        Ok(())
    }

    fn lines_of(record: &Path) -> Vec<String> {
        std::fs::read_to_string(record)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// What an agent child reports: ten below this process, which the test
    /// runner may itself run niced (the gates run under `nice -n 10`).
    fn agent_line() -> String {
        format!("NICE={}", child_nice_for(own_nice(), CHILD_NICE))
    }

    #[test]
    fn a_niced_only_agent_runs_once_ten_below_the_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        run_recording(&ChildPlacement::NiceOnly, ChildKind::Agent, &record).unwrap();
        assert_eq!(lines_of(&record), vec![agent_line()]);
    }

    #[test]
    fn a_niced_only_terminal_runs_once_half_a_step_down() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("record");
        run_recording(&ChildPlacement::NiceOnly, ChildKind::Terminal, &record).unwrap();
        assert_eq!(
            lines_of(&record),
            vec![format!(
                "NICE={}",
                child_nice_for(own_nice(), TERMINAL_NICE_WITHOUT_SCOPE)
            )]
        );
    }

    /// A stand-in for `busctl` with the given body, and a placement that
    /// uses it with short deadlines.
    fn fake_busctl(dir: &Path, body: &str, deadlines: ScopeDeadlines) -> ChildPlacement {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("busctl");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        ChildPlacement::TransientScope {
            busctl: path,
            systemctl: None,
            bound_to: None,
            deadlines,
        }
    }

    fn quick() -> ScopeDeadlines {
        ScopeDeadlines {
            call: Duration::from_millis(500),
            arrival: Duration::from_millis(200),
        }
    }

    /// The fallback, every way a scope can fail to come: the child runs
    /// exactly once, niced, and the spawn succeeds.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_scope_that_cannot_be_had_costs_the_child_nothing_but_the_scope() {
        let dir = tempfile::tempdir().unwrap();
        let cases: Vec<(&str, ChildPlacement)> = vec![
            (
                "the manager refuses",
                fake_busctl(
                    &dir.path().join("refuses"),
                    "echo 'Failed to connect to bus' >&2; exit 1",
                    quick(),
                ),
            ),
            (
                "the manager says yes and moves nothing",
                fake_busctl(&dir.path().join("lies"), "exit 0", quick()),
            ),
            (
                "the manager never answers",
                fake_busctl(&dir.path().join("hangs"), "sleep 30", quick()),
            ),
            (
                "there is no busctl at all",
                ChildPlacement::TransientScope {
                    busctl: dir.path().join("missing/busctl"),
                    systemctl: None,
                    bound_to: None,
                    deadlines: quick(),
                },
            ),
        ];
        for (case, placement) in cases {
            let record = dir
                .path()
                .join(format!("record-{}", case.replace(' ', "-")));
            let started = Instant::now();
            run_recording(&placement, ChildKind::Agent, &record)
                .unwrap_or_else(|error| panic!("{case}: the spawn failed: {error}"));
            assert_eq!(lines_of(&record), vec![agent_line()], "{case}");
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{case}: the fallback took {:?}",
                started.elapsed()
            );
        }
        assert_eq!(
            ChildPlacement::current(),
            ChildPlacement::NiceOnly,
            "a scope that failed is not asked for again"
        );
    }

    /// A child that dies before it reaches the gate is not waited for, and
    /// the spawn says so.
    #[test]
    fn a_child_that_dies_at_the_gate_is_a_failed_spawn_not_a_wait() {
        let gate = Gate::open().unwrap();
        let fifo = gate.path().to_path_buf();
        // Not the gate script: this child never opens the FIFO.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let placed = ChildPlacement::NiceOnly.place(gate, ChildKind::Agent, pid, &mut || {
            matches!(child.try_wait(), Ok(None))
        });
        assert_eq!(
            placed.released.unwrap_err(),
            "the child died before it reached its gate"
        );
        assert!(!fifo.exists(), "the gate is removed either way");
        let _ = child.wait();
    }
}
