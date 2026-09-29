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
}

impl Drop for Units {
    fn drop(&mut self) {
        systemctl(&["stop", &self.scope, &self.service]);
        systemctl(&["reset-failed", &self.scope, &self.service]);
        if let Some(mut harness) = self.harness.take() {
            let _ = harness.kill();
            let _ = harness.wait();
        }
    }
}

/// A process that notes its SIGTERM in `dir/<name>-term`, then exits.
fn noting_term(dir: &Path, name: &str) -> String {
    let note = dir.join(format!("{name}-term"));
    format!(
        "trap 'touch {note}; exit 0' TERM; while :; do sleep 0.05; done",
        note = note.display()
    )
}

/// Like the bridge on SIGTERM: note whether its agent is still alive (the
/// roster), then stop the agent's scope itself without waiting on the
/// manager (`priority::stop_children`).
fn stand_in_bridge(dir: &Path, service: &str, scope: &str) {
    let saw = dir.join("bridge-saw");
    let harness_term = dir.join("harness-term");
    let script = format!(
        "trap 'if [ -e {harness_term} ]; then echo gone; else echo alive; fi > {saw}; \
         systemctl --user stop --no-block {scope}; exit 0' TERM; \
         while :; do sleep 0.05; done",
        harness_term = harness_term.display(),
        saw = saw.display()
    );
    let started = Command::new("systemd-run")
        .args(["--user", "--quiet", "--collect", "--unit", service])
        .args(["bash", "-c", &script])
        .stdin(Stdio::null())
        .status()
        .expect("systemd-run runs");
    assert!(started.success(), "the stand-in bridge started");
    wait_until("the stand-in bridge to be active", || {
        systemctl(&["is-active", "--quiet", service])
    });
}

fn stand_in_harness_in_scope(dir: &Path, scope: &str, service: &str) -> Child {
    let harness = Command::new("bash")
        .args(["-c", &noting_term(dir, "harness")])
        .stdin(Stdio::null())
        .spawn()
        .expect("the stand-in harness starts");
    let argv = start_transient_unit_arguments(scope, ChildKind::Agent, harness.id(), Some(service));
    let placed = Command::new("busctl")
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .expect("busctl runs");
    assert!(placed.success(), "the manager started the scope");
    wait_until("the scope to be active", || {
        systemctl(&["is-active", "--quiet", scope])
    });
    harness
}

/// A stand-in bridge running as a service, with a stand-in harness in a
/// scope bound to it the way the bridge binds its agents' scopes. `None`
/// where there is no user manager to ask.
fn bridge_with_an_agent(test: &str) -> Option<(tempfile::TempDir, Units)> {
    if !user_manager_answers() {
        eprintln!("skipped: no user systemd manager to ask");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let tag = format!("{test}-{}", std::process::id());
    let mut units = Units {
        service: format!("build-test-213-bridge-{tag}.service"),
        scope: format!("build-test-213-agent-{tag}.scope"),
        harness: None,
    };
    stand_in_bridge(dir.path(), &units.service, &units.scope);
    units.harness = Some(stand_in_harness_in_scope(
        dir.path(),
        &units.scope,
        &units.service,
    ));
    Some((dir, units))
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
    let Some((dir, units)) = bridge_with_an_agent("stop") else {
        return;
    };
    assert!(systemctl(&["stop", &units.service]));
    assert_the_bridge_saw_its_agent_alive(dir.path());
    assert_the_agent_was_stopped(dir.path());
}

/// A plain restart is a stop and a start: the same order. The manager does
/// not carry a restart across `BindsTo=` (with `After=` or `Before=`), so
/// it is the bridge's own way down that stops the scope: no orphan.
#[test]
fn restarting_the_bridge_unit_signals_the_bridge_first_and_leaves_no_orphan() {
    let Some((dir, units)) = bridge_with_an_agent("restart") else {
        return;
    };
    assert!(systemctl(&["restart", &units.service]));
    assert_the_bridge_saw_its_agent_alive(dir.path());
    assert_the_agent_was_stopped(dir.path());
}

/// A bridge that dies without a shutdown (SIGKILL, a panic that aborts)
/// still takes its agents' scopes with it: `Before=` orders stops, it does
/// not weaken `BindsTo=`.
#[test]
fn a_bridge_that_dies_takes_its_agents_scopes_with_it() {
    let Some((dir, units)) = bridge_with_an_agent("crash") else {
        return;
    };
    assert!(systemctl(&["kill", "--signal=KILL", &units.service]));
    assert_the_agent_was_stopped(dir.path());
}
