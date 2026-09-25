//! The stop timeout, for a unit installed before the unit stated one.
//!
//! The SIGTERM path writes the resume roster before the daemon exits, and a
//! unit whose `TimeoutStopSec=` is shorter than that path lets systemd's
//! SIGKILL cut it off. `build-bridge install` writes the timeout into the unit
//! itself (`crate::service::systemd`), but the updater never rewrites a unit,
//! so a bridge installed before that keeps whatever the manager's default is
//! — which a distribution may have cut to a few seconds.
//!
//! So on every start the helper makes (the bridge is stopped at that moment),
//! it reads the timeout systemd will actually apply, and only when that is
//! shorter than [`STOP_TIMEOUT_SECS`] does it add a drop-in and reload. Asking
//! for the EFFECTIVE value is what keeps this honest: a unit wherever it was
//! installed, a timeout already stated, or an operator's own override all
//! answer here, and none of them is touched. The drop-in's `10-` name sorts
//! before an operator's `override.conf`, so theirs still wins.
//!
//! Best effort throughout: nothing here can fail an update.

use crate::resume::disk::{self, RealDisk};
use crate::resume::STOP_TIMEOUT_SECS;
use crate::service::systemd::UNIT_NAME;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// The drop-in's file name.
pub(super) const DROP_IN_NAME: &str = "10-build-stop-timeout.conf";

/// A time span as `systemctl show` renders one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Timespan {
    Finite(Duration),
    Infinity,
}

/// The drop-in, in the user unit directory the installer writes the unit to.
pub(super) fn drop_in_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
        .join(format!("{UNIT_NAME}.d"))
        .join(DROP_IN_NAME)
}

fn drop_in_body() -> String {
    format!(
        "# Written by the Build bridge updater: the SIGTERM path records the resume\n\
         # roster before it exits. An override.conf beside this one still wins.\n\
         [Service]\n\
         TimeoutStopSec={STOP_TIMEOUT_SECS}\n"
    )
}

/// Parse systemd's rendering of a span: `infinity`, or space-separated
/// `<number><unit>` terms such as `1min 30s` or `1.500s`. `None` for anything
/// else, which is left alone rather than guessed at.
pub(super) fn parse_timespan(text: &str) -> Option<Timespan> {
    let text = text.trim();
    if text == "infinity" {
        return Some(Timespan::Infinity);
    }
    let mut total = 0f64;
    for term in text.split_whitespace() {
        let split = term
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(term.len());
        let (number, unit) = term.split_at(split);
        total += number.parse::<f64>().ok()? * unit_seconds(unit)?;
    }
    (!text.is_empty()).then(|| Timespan::Finite(Duration::from_secs_f64(total)))
}

fn unit_seconds(unit: &str) -> Option<f64> {
    Some(match unit {
        "us" | "µs" | "usec" => 1e-6,
        "ms" | "msec" => 1e-3,
        "" | "s" | "sec" => 1.0,
        "min" | "m" => 60.0,
        "h" | "hr" => 3600.0,
        "d" => 86_400.0,
        "w" => 604_800.0,
        _ => return None,
    })
}

fn enough(span: Timespan) -> bool {
    span >= Timespan::Finite(Duration::from_secs(STOP_TIMEOUT_SECS))
}

/// What the helper asks systemd, so the tests can answer instead.
pub(super) trait Manager {
    /// The effective `TimeoutStopUSec` of the bridge's unit, as printed.
    fn stop_timeout(&mut self) -> Option<String>;
    fn reload(&mut self) -> Result<(), String>;
}

/// Make sure the next stop leaves room for the roster. Returns a line for the
/// helper's log when it did something, or could not tell.
pub(super) fn ensure(home: &Path, manager: &mut dyn Manager) -> Option<String> {
    let Some(shown) = manager.stop_timeout() else {
        return Some("could not read the bridge's stop timeout; left as it is".to_string());
    };
    let Some(span) = parse_timespan(&shown) else {
        return Some(format!(
            "stop timeout {shown:?} not understood; left as it is"
        ));
    };
    if enough(span) {
        return None;
    }
    if let Err(error) = write_drop_in(home) {
        return Some(format!(
            "stop timeout is {shown}; could not raise it: {error}"
        ));
    }
    if let Err(error) = manager.reload() {
        return Some(format!(
            "stop timeout drop-in written, but systemd did not reload: {error}"
        ));
    }
    let after = manager.stop_timeout().unwrap_or_default();
    Some(match parse_timespan(&after) {
        Some(span) if enough(span) => {
            format!("stop timeout raised from {shown} to {after} for the resume roster")
        }
        _ => format!(
            "stop timeout drop-in written, but another setting keeps it at {after}; left to it"
        ),
    })
}

fn write_drop_in(home: &Path) -> Result<(), String> {
    let path = drop_in_path(home);
    let dir = path.parent().expect("the drop-in has a directory");
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    disk::replace(&RealDisk, &path, &drop_in_body())
}

/// The user manager itself.
pub(super) struct Systemctl;

impl Manager for Systemctl {
    fn stop_timeout(&mut self) -> Option<String> {
        let mut command = Command::new("systemctl");
        command.args([
            "--user",
            "show",
            "-p",
            "TimeoutStopUSec",
            "--value",
            UNIT_NAME,
        ]);
        let output = super::bounded_output(&mut command, Duration::from_secs(10)).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn reload(&mut self) -> Result<(), String> {
        let mut command = Command::new("systemctl");
        command.args(["--user", "daemon-reload"]);
        let status = super::run_command(&mut command, Duration::from_secs(30))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| "systemctl --user daemon-reload failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manager that answers from a script and counts reloads.
    struct Scripted {
        answers: Vec<Option<&'static str>>,
        reloads: usize,
        reload_fails: bool,
    }

    impl Scripted {
        fn answering(answers: &[Option<&'static str>]) -> Scripted {
            Scripted {
                answers: answers.iter().rev().copied().collect(),
                reloads: 0,
                reload_fails: false,
            }
        }
    }

    impl Manager for Scripted {
        fn stop_timeout(&mut self) -> Option<String> {
            self.answers.pop().flatten().map(str::to_string)
        }

        fn reload(&mut self) -> Result<(), String> {
            self.reloads += 1;
            if self.reload_fails {
                Err("no bus".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn spans_parse_as_systemctl_prints_them() {
        let secs = |s| Some(Timespan::Finite(Duration::from_secs(s)));
        assert_eq!(parse_timespan("30s"), secs(30));
        assert_eq!(parse_timespan("1min 30s"), secs(90));
        assert_eq!(parse_timespan("2h"), secs(7200));
        assert_eq!(parse_timespan("0"), secs(0));
        assert_eq!(
            parse_timespan("1min 30.500s"),
            Some(Timespan::Finite(Duration::from_millis(90_500)))
        );
        assert_eq!(
            parse_timespan("500ms"),
            Some(Timespan::Finite(Duration::from_millis(500)))
        );
        assert_eq!(parse_timespan("infinity"), Some(Timespan::Infinity));
        assert_eq!(parse_timespan(""), None);
        assert_eq!(parse_timespan("soon"), None);
        assert_eq!(parse_timespan("3 fortnights"), None);
    }

    /// systemd's own default (90 s), a unit that states 30 s, and no timeout
    /// at all are all left exactly as they are.
    #[test]
    fn a_long_enough_timeout_is_left_alone() {
        for shown in ["1min 30s", "30s", "infinity"] {
            let home = tempfile::tempdir().unwrap();
            let mut manager = Scripted::answering(&[Some(shown)]);
            assert_eq!(ensure(home.path(), &mut manager), None, "{shown}");
            assert_eq!(manager.reloads, 0);
            assert!(!drop_in_path(home.path()).exists());
        }
    }

    #[test]
    fn a_short_timeout_gets_the_drop_in_and_a_reload() {
        let home = tempfile::tempdir().unwrap();
        let mut manager = Scripted::answering(&[Some("10s"), Some("30s")]);
        let said = ensure(home.path(), &mut manager).unwrap();
        assert!(said.contains("raised from 10s to 30s"), "{said}");
        assert_eq!(manager.reloads, 1);
        let written = std::fs::read_to_string(drop_in_path(home.path())).unwrap();
        assert!(
            written.contains("[Service]\nTimeoutStopSec=30\n"),
            "{written}"
        );
        assert!(drop_in_path(home.path())
            .ends_with(".config/systemd/user/build-bridge.service.d/10-build-stop-timeout.conf"));
    }

    /// An operator's override that keeps it short is theirs; the helper says
    /// so and does not fight it.
    #[test]
    fn an_override_that_keeps_it_short_is_reported_not_fought() {
        let home = tempfile::tempdir().unwrap();
        let mut manager = Scripted::answering(&[Some("10s"), Some("10s")]);
        let said = ensure(home.path(), &mut manager).unwrap();
        assert!(said.contains("another setting keeps it at 10s"), "{said}");
        assert_eq!(manager.reloads, 1);
    }

    #[test]
    fn what_cannot_be_read_is_left_alone() {
        for answer in [None, Some("soon")] {
            let home = tempfile::tempdir().unwrap();
            let mut manager = Scripted::answering(&[answer]);
            let said = ensure(home.path(), &mut manager).unwrap();
            assert!(said.contains("left as it is"), "{said}");
            assert_eq!(manager.reloads, 0);
            assert!(!drop_in_path(home.path()).exists());
        }
    }

    #[test]
    fn a_reload_that_fails_is_reported() {
        let home = tempfile::tempdir().unwrap();
        let mut manager = Scripted::answering(&[Some("5s")]);
        manager.reload_fails = true;
        let said = ensure(home.path(), &mut manager).unwrap();
        assert!(said.contains("did not reload: no bus"), "{said}");
    }
}
