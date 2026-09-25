//! What a bridge restart has to put back.
//!
//! Rolling the binary kills every harness on the device at once — the project
//! agent orchestrating the work included — and nothing brought them back. An
//! overnight roll left the work stalled until a human noticed and said
//! something. This is the record that closes that gap: who was mid-turn when
//! the daemon went down, written at shutdown and read once at boot.
//!
//! It is a sidecar JSON file beside the store rather than a table in it, and
//! deliberately so. The store refuses to open a database written by a newer
//! bridge, so a new table means the roll cannot be rolled back — and the roster
//! is worth nothing the moment it has been read. A file the next boot consumes
//! and deletes is the right durability for it.
//!
//! A shutdown that never runs — SIGKILL, a panic, the power going — writes no
//! roster, so there is a second file: the [`LiveRoster`], rewritten whenever
//! the set of working agents changes and removed only once a clean shutdown
//! has written the roster proper. A boot that finds the live file and no
//! roster promotes the one to the other ([`promote_live_roster`]), and resumes
//! from it exactly as it would from a roll.
//!
//! Every change to either file — written, removed, renamed — is durable before
//! the call that made it returns ([`disk`]): a power cut must never bring back
//! a roster that had already been cleared or consumed.
//!
//! Both files hold ids and nothing else: which entity, which agent, which
//! conversation, which harness session. No message, no prompt, no token.

pub mod disk;
mod live;

pub use live::{shut_down, LiveRoster};

use disk::{Disk, RealDisk};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The roster, beside the store's database in the tasks directory.
pub const ROSTER_FILE: &str = "resume-roster.json";

/// The live roster: who is working right now, kept on disk for the shutdown
/// that never happens. Beside the roster, and only ever renamed onto it.
pub const LIVE_ROSTER_FILE: &str = "resume-roster.live.json";

/// How long systemd waits after SIGTERM before it sends SIGKILL. The final
/// write is one small file and an fsync, and nothing on the way to it waits
/// for the app mutex; this is the headroom for a disk that is slow to sync
/// and the scope stops after it, not an estimate of either. A SIGKILL past it
/// still loses nothing the live roster did not already have on disk.
pub const STOP_TIMEOUT_SECS: u64 = 30;

/// The per-roll opt-out a script can drop next to it. Consumed at boot, so it
/// silences exactly one restart and never the one after.
pub const OPT_OUT_FILE: &str = "no-resume";

/// The opt-out for a shell that would rather say it than write a file.
/// Anything but `0`, `false` or `no` leaves resuming on, which is the default.
pub const OPT_OUT_ENV: &str = "BRIDGE_RESUME_AGENTS";

/// One agent to bring back, and what bringing it back needs.
///
/// Everything here is read off the agent record at shutdown. `resume_session_id`
/// is the one field that is not merely an address: it is what lets the harness
/// resume the conversation BY NAME instead of guessing the newest transcript in
/// the checkout, and an agent that never announced one still resumes through
/// that older probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumingAgent {
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
    /// Whether the agent was mid-turn rather than merely live. Both are
    /// resumed; this is what the notice means by "may have been cut short".
    #[serde(default)]
    pub was_working: bool,
}

/// Every agent one shutdown found working, and when.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRoster {
    /// When the bridge went down, RFC3339. The notice says it back to the
    /// agent, because "your turn was cut short" is only actionable with a time
    /// against the conversation it is reading.
    pub recorded_at: String,
    /// The binary that went down. The one at boot may be a different one —
    /// that is the whole point of a roll — and the notice names the new one.
    #[serde(default)]
    pub version: String,
    pub agents: Vec<ResumingAgent>,
}

impl ResumeRoster {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(ROSTER_FILE)
    }

    /// Read and consume the roster: the file goes whether or not what it held
    /// could be acted on. A roster left behind would resurrect the same agents
    /// on the boot after this one, which is not a restart notice — it is a
    /// haunting.
    pub fn take(dir: &Path) -> Option<ResumeRoster> {
        Self::take_on(&RealDisk, dir)
    }

    pub(crate) fn take_on(disk: &dyn Disk, dir: &Path) -> Option<ResumeRoster> {
        let path = Self::path(dir);
        let raw = disk.read(&path).ok()?;
        if let Err(error) = disk::remove(disk, &path) {
            eprintln!("resume: could not consume the roster ({error}); it may be read again");
        }
        match serde_json::from_str(&raw) {
            Ok(roster) => Some(roster),
            Err(error) => {
                eprintln!(
                    "resume: {} is unreadable ({error}); ignoring it",
                    path.display()
                );
                None
            }
        }
    }

    /// Write the roster, or nothing at all when there is nobody to bring back.
    ///
    /// An empty roster is not written and any older one is removed: shutting
    /// down with every agent idle must not leave last week's roster to be read
    /// as this boot's.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        self.save_on(&RealDisk, dir)
    }

    pub(crate) fn save_on(&self, disk: &dyn Disk, dir: &Path) -> Result<(), String> {
        let path = Self::path(dir);
        if self.agents.is_empty() {
            return disk::remove(disk, &path);
        }
        disk::replace(disk, &path, &self.body()?)
    }

    fn body(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| format!("serialize the resume roster: {error}"))
    }

    /// Drop any roster without reading it — the opt-out's shutdown half. The
    /// live roster goes with it: an opted-out roll must not be resumed from
    /// the file a crash would have been.
    pub(crate) fn forget_on(disk: &dyn Disk, dir: &Path) -> Result<(), String> {
        disk::remove(disk, &Self::path(dir))?;
        disk::remove(disk, &live_path(dir))
    }
}

pub(crate) fn live_path(dir: &Path) -> PathBuf {
    dir.join(LIVE_ROSTER_FILE)
}

/// At boot, before anything can write the live roster again: a live roster
/// with no roster beside it is what a death that ran no shutdown left, and it
/// becomes the roster this boot resumes from. Renamed rather than read, so a
/// boot that dies before it resumes anybody leaves the same roster for the
/// boot after it.
///
/// A roster beside it wins: a clean shutdown wrote that one last, and the live
/// file is what it had not yet removed. Returns whether a live roster was
/// promoted, which is what the boot log says.
pub fn promote_live_roster(dir: &Path) -> bool {
    promote_live_roster_on(&RealDisk, dir).unwrap_or_else(|error| {
        eprintln!("resume: could not promote the live roster: {error}");
        false
    })
}

pub(crate) fn promote_live_roster_on(disk: &dyn Disk, dir: &Path) -> Result<bool, String> {
    let live = live_path(dir);
    if !live.exists() {
        return Ok(false);
    }
    if ResumeRoster::path(dir).exists() {
        disk::remove(disk, &live)?;
        return Ok(false);
    }
    disk::rename(disk, &live, &ResumeRoster::path(dir))?;
    Ok(true)
}

/// Whether this roll resumes its agents. The default is yes; a deliberate
/// shutdown says otherwise.
///
/// Two ways to say it because the two callers are different: a person stopping
/// the service by hand has an environment, and `roll-bridge.sh` — which stops
/// the daemon from a unit of its own, with an environment the daemon never
/// sees — has a directory it can write a file into. Either one is enough.
///
/// `read_env` is passed in rather than read here so the decision is testable
/// without setting a process-wide variable under a parallel test runner.
pub fn resume_is_wanted(dir: &Path, read_env: impl Fn(&str) -> Option<String>) -> bool {
    if opt_out_file_present(dir) {
        return false;
    }
    match read_env(OPT_OUT_ENV) {
        Some(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        None => true,
    }
}

/// Whether the per-roll marker is there. Split out because boot has to REMOVE
/// it after asking — one marker silences one restart.
pub fn opt_out_file_present(dir: &Path) -> bool {
    dir.join(OPT_OUT_FILE).exists()
}

/// Take the marker away, so the roll after this one resumes again.
pub fn clear_opt_out_file(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(OPT_OUT_FILE));
}

/// What the agent is told when it comes back.
///
/// Written to be actionable by a model that has just been handed a transcript
/// it does not remember writing: what happened, when, that its own last turn is
/// suspect, and what to do about it. It says Build restarted rather than "you
/// were restarted" because the agent did not do this and must not read it as an
/// instruction it failed to follow.
pub fn restart_notice(at: &str, version: &str, was_working: bool) -> String {
    let turn = if was_working {
        "You were part-way through a turn when it went down, so your last action may have been \
         cut short: assume nothing you were doing finished, and check rather than trust it."
    } else {
        "Your session was live when it went down, so anything you had not finished is unfinished."
    };
    format!(
        "Build restarted at {at} (bridge {version}) and brought your session back. This message \
         is from Build, not from the user — nobody is waiting on an answer to it.\n\n{turn}\n\n\
         Read your conversation from where you left off, work out what you had reached, and \
         carry on with it. If the work was already finished, say so and stop; if you cannot tell \
         what you were doing, ask."
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> ResumeRoster {
        ResumeRoster {
            recorded_at: "2026-09-20T03:00:00Z".to_string(),
            version: "0.2.0".to_string(),
            agents: vec![ResumingAgent {
                entity_id: "run-1".to_string(),
                agent_id: "agent-1".to_string(),
                conversation_id: "agent-1".to_string(),
                resume_session_id: Some("sess-1".to_string()),
                was_working: true,
            }],
        }
    }

    #[test]
    fn a_roster_round_trips_through_the_file_it_is_written_to() {
        let dir = tempfile::tempdir().unwrap();
        roster().save(dir.path()).unwrap();
        assert_eq!(ResumeRoster::take(dir.path()), Some(roster()));
    }

    /// Reading consumes it: the boot after a resume must not resume again.
    #[test]
    fn taking_a_roster_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        roster().save(dir.path()).unwrap();
        assert!(ResumeRoster::take(dir.path()).is_some());
        assert!(ResumeRoster::take(dir.path()).is_none());
        assert!(!ResumeRoster::path(dir.path()).exists());
    }

    /// An empty roster is not a roster, and it clears the one before it — a
    /// shutdown with nobody working must not leave an older list to be read.
    #[test]
    fn an_empty_roster_removes_whatever_was_there() {
        let dir = tempfile::tempdir().unwrap();
        roster().save(dir.path()).unwrap();
        ResumeRoster::default().save(dir.path()).unwrap();
        assert!(!ResumeRoster::path(dir.path()).exists());
        assert!(ResumeRoster::take(dir.path()).is_none());
    }

    /// Unreadable is not fatal: a truncated file loses the resume, never the
    /// boot.
    #[test]
    fn an_unreadable_roster_is_ignored_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(ResumeRoster::path(dir.path()), "{ not json").unwrap();
        assert_eq!(ResumeRoster::take(dir.path()), None);
        assert!(!ResumeRoster::path(dir.path()).exists());
    }

    #[test]
    fn resuming_is_the_default_and_either_opt_out_turns_it_off() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resume_is_wanted(dir.path(), |_| None));
        assert!(resume_is_wanted(dir.path(), |_| Some("1".to_string())));
        assert!(resume_is_wanted(dir.path(), |_| Some("yes".to_string())));

        for said in ["0", "false", "no", "off", " FALSE "] {
            assert!(
                !resume_is_wanted(dir.path(), |_| Some(said.to_string())),
                "{said} should turn resuming off"
            );
        }

        std::fs::write(dir.path().join(OPT_OUT_FILE), "").unwrap();
        assert!(
            !resume_is_wanted(dir.path(), |_| None),
            "the marker file alone is enough"
        );
        clear_opt_out_file(dir.path());
        assert!(resume_is_wanted(dir.path(), |_| None), "and one roll only");
    }

    /// The notice has to carry the three things a cold model cannot work out:
    /// that Build did this, when, and that its own last turn is suspect.
    #[test]
    fn the_notice_says_who_restarted_when_and_what_to_distrust() {
        let notice = restart_notice("2026-09-20T03:00:00Z", "0.2.0", true);
        assert!(notice.contains("2026-09-20T03:00:00Z"), "{notice}");
        assert!(notice.contains("bridge 0.2.0"), "{notice}");
        assert!(notice.contains("from Build, not from the user"), "{notice}");
        assert!(notice.contains("cut short"), "{notice}");
        assert!(notice.contains("Read your conversation"), "{notice}");

        let live = restart_notice("2026-09-20T03:00:00Z", "0.2.0", false);
        assert!(
            !live.contains("cut short"),
            "an idle-at-prompt session was not mid-turn: {live}"
        );
        assert!(live.contains("Read your conversation"), "{live}");
    }
}
