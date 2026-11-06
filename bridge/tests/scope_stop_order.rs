//! A roll stops the bridge's unit while its agents are mid-turn, and the
//! shutdown has to see them working to bring them back (#213). The agents run
//! in transient scopes of their own, bound to the bridge's unit, so which of
//! the two the manager stops first is up to the ordering each scope carries:
//! a scope stopped first kills its harness while the bridge still runs, the
//! bridge settles that turn as over, and the roster it records says nobody.
//!
//! This asks the user's real systemd, with the exact properties the bridge
//! asks for: a stand-in "bridge" service, a scope bound to it holding a
//! stand-in "harness", and a stop of the service. The service's SIGTERM
//! handler writes down whether the harness was still alive. Skipped, with a
//! line, where there is no user manager to ask (CI, containers).

use build_bridge::priority::{start_transient_unit_arguments, ChildKind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn user_manager_answers() -> bool {
    let answers = |program: &str, args: &[&str]| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    answers("systemctl", &["--user", "show-environment"])
        && answers("busctl", &["--user", "status"])
        && answers("systemd-run", &["--version"])
}

fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Stops both units however the test ends, so a failure leaves nothing
/// running under the user's manager.
struct Units {
    service: String,
    scope: String,
    harness: Option<Child>,
    // Fields are dropped after Units::drop, so the traps can still write.
    dir: tempfile::TempDir,
}

impl Units {
    fn new(test: &str) -> Option<Self> {
        if !user_manager_answers() {
            eprintln!("skipped: no user systemd manager to ask");
            return None;
        }
        let tag = format!("{test}-{}", std::process::id());
        Some(Self {
            service: format!("build-test-213-bridge-{tag}.service"),
            scope: format!("build-test-213-agent-{tag}.scope"),
            harness: None,
            dir: tempfile::tempdir().unwrap(),
        })
    }

    fn start_harness(&mut self) {
        // Own the child before scope admission, which can fail or panic.
        self.harness = Some(
            Command::new("bash")
                .args(["-c", &noting_term(self.dir.path(), "harness")])
                .stdin(Stdio::null())
                .spawn()
                .expect("the stand-in harness starts"),
        );
    }
}

impl Drop for Units {
    fn drop(&mut self) {
        // A restart collects the old scope. Combining it with the service
        // makes systemctl reject the whole stop request when that scope is gone.
        for unit in [&self.service, &self.scope] {
            systemctl(&["stop", unit]);
            systemctl(&["reset-failed", unit]);
        }
        if let Some(mut harness) = self.harness.take() {
            let _ = harness.kill();
            let _ = harness.wait();
        }
    }
}

/// A process that announces its installed trap in `dir/<name>-ready`, then
/// notes its SIGTERM in `dir/<name>-term` and exits.
fn noting_term(dir: &Path, name: &str) -> String {
    let note = dir.join(format!("{name}-term"));
    let ready = dir.join(format!("{name}-ready"));
    format!(
        "trap 'touch {note}; exit 0' TERM; touch {ready}; while :; do sleep 0.05; done",
        note = note.display(),
        ready = ready.display()
    )
}

/// Like the bridge on SIGTERM: note whether its agent is still alive (the
/// roster), then stop the agent's scope itself without waiting on the
/// manager (`priority::stop_children`).
fn stand_in_bridge(dir: &Path, service: &str, scope: &str) {
    let saw = dir.join("bridge-saw");
    let harness_term = dir.join("harness-term");
    let ready = dir.join("bridge-ready");
    let script = format!(
        "trap 'if [ -e {harness_term} ]; then echo gone; else echo alive; fi > {saw}; \
         systemctl --user stop --no-block {scope}; exit 0' TERM; \
         touch {ready}; while :; do sleep 0.05; done",
        harness_term = harness_term.display(),
        saw = saw.display(),
        ready = ready.display()
    );
    let started = Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", service])
        // Also bound a stand-in left behind by an aborted test runner.
        .args([
            "--property=RuntimeMaxSec=60s",
            "--property=TimeoutStopSec=5s",
        ])
        .args(["bash", "-c", &script])
        .stdin(Stdio::null())
        .status()
        .expect("systemd-run runs");
    assert!(started.success(), "the stand-in bridge started");
    wait_until("the stand-in bridge to be active", || {
        systemctl(&["is-active", "--quiet", service])
    });
    // Type=simple becomes active before bash has installed its SIGTERM trap.
    wait_until("the stand-in bridge's SIGTERM trap to be ready", || {
        ready.exists()
    });
}

fn stand_in_harness_in_scope(units: &mut Units) {
    units.start_harness();
    let argv = start_transient_unit_arguments(
        &units.scope,
        ChildKind::Agent,
        units.harness.as_ref().unwrap().id(),
        Some(&units.service),
    );
    let placed = Command::new("busctl")
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .expect("busctl runs");
    assert!(placed.success(), "the manager started the scope");
    wait_until("the scope to be active", || {
        systemctl(&["is-active", "--quiet", &units.scope])
    });
    // Scope admission says nothing about how far the harness has executed.
    wait_until("the harness's SIGTERM trap to be ready", || {
        units.dir.path().join("harness-ready").exists()
    });
}

/// A stand-in bridge running as a service, with a stand-in harness in a
/// scope bound to it the way the bridge binds its agents' scopes. `None`
/// where there is no user manager to ask.
fn bridge_with_an_agent(test: &str) -> Option<Units> {
    let mut units = Units::new(test)?;
    stand_in_bridge(units.dir.path(), &units.service, &units.scope);
    stand_in_harness_in_scope(&mut units);
    Some(units)
}

fn assert_unit_stopped(unit: &str) {
    let state = Command::new("systemctl")
        .args(["--user", "show", "--property=ActiveState", "--value", unit])
        .stdin(Stdio::null())
        .output()
        .expect("systemctl shows the unit state");
    assert!(state.status.success(), "could not inspect {unit}");
    // Collected units also report inactive; deactivating is not enough.
    assert_eq!(
        String::from_utf8(state.stdout).unwrap().trim(),
        "inactive",
        "fixture cleanup left {unit} running"
    );
}

fn assert_fixture_cleanup(units: Units) {
    let service = units.service.clone();
    let scope = units.scope.clone();
    drop(units);
    assert_unit_stopped(&service);
    assert_unit_stopped(&scope);
}

fn assert_the_bridge_saw_its_agent_alive(dir: &Path) {
    let saw: PathBuf = dir.join("bridge-saw");
    wait_until("the stand-in bridge's SIGTERM note", || saw.exists());
    assert_eq!(
        std::fs::read_to_string(&saw).unwrap().trim(),
        "alive",
        "the bridge's SIGTERM came after its agents were already stopped"
    );
}

fn assert_the_agent_was_stopped(dir: &Path) {
    let harness_term = dir.join("harness-term");
    wait_until("the harness to be stopped", || harness_term.exists());
}

/// The #213 roll: `systemctl stop` on the bridge's unit. The bridge must get
/// its SIGTERM while its agents are still alive, so the roster it records
/// names who was working; the scope is stopped after it, by `BindsTo=`.
#[test]
fn stopping_the_bridge_unit_signals_the_bridge_before_its_agents_scopes() {
    let Some(units) = bridge_with_an_agent("stop") else {
        return;
    };
    assert!(systemctl(&["stop", &units.service]));
    assert_the_bridge_saw_its_agent_alive(units.dir.path());
    assert_the_agent_was_stopped(units.dir.path());
    assert_fixture_cleanup(units);
}

/// A plain restart is a stop and a start: the same order. The manager does
/// not carry a restart across `BindsTo=` (with `After=` or `Before=`), so
/// it is the bridge's own way down that stops the scope: no orphan.
#[test]
fn restarting_the_bridge_unit_signals_the_bridge_first_and_leaves_no_orphan() {
    let Some(units) = bridge_with_an_agent("restart") else {
        return;
    };
    assert!(systemctl(&["restart", &units.service]));
    assert_the_bridge_saw_its_agent_alive(units.dir.path());
    assert_the_agent_was_stopped(units.dir.path());
    assert_fixture_cleanup(units);
}

/// A bridge that dies without a shutdown (SIGKILL, a panic that aborts)
/// still takes its agents' scopes with it: `Before=` orders stops, it does
/// not weaken `BindsTo=`.
#[test]
fn a_bridge_that_dies_takes_its_agents_scopes_with_it() {
    let Some(units) = bridge_with_an_agent("crash") else {
        return;
    };
    assert!(systemctl(&["kill", "--signal=KILL", &units.service]));
    assert_the_agent_was_stopped(units.dir.path());
    assert_fixture_cleanup(units);
}

#[test]
fn a_panic_after_restart_stops_the_restarted_service() {
    let Some(units) = bridge_with_an_agent("restart-panic") else {
        return;
    };
    assert!(systemctl(&["restart", &units.service]));
    let service = units.service.clone();
    let scope = units.scope.clone();
    let result = std::panic::catch_unwind(move || {
        let _units = units;
        panic!("simulate an assertion failure after restart");
    });
    assert!(result.is_err());
    assert_unit_stopped(&service);
    assert_unit_stopped(&scope);
}

#[test]
fn a_panic_before_scope_admission_stops_the_service_and_reaps_the_harness() {
    let Some(mut units) = Units::new("setup-panic") else {
        return;
    };
    stand_in_bridge(units.dir.path(), &units.service, &units.scope);
    units.start_harness();
    let harness_pid = units.harness.as_ref().unwrap().id();
    let service = units.service.clone();
    let scope = units.scope.clone();
    let result = std::panic::catch_unwind(move || {
        let _units = units;
        panic!("simulate an assertion failure before scope admission");
    });
    assert!(result.is_err());
    assert_unit_stopped(&service);
    assert_unit_stopped(&scope);
    assert!(
        !Path::new(&format!("/proc/{harness_pid}")).exists(),
        "fixture cleanup did not reap its unscoped harness"
    );
}
