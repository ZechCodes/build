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
//! Both files hold ids and nothing else: which entity, which agent, which
//! conversation, which harness session. No message, no prompt, no token.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

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
        let path = Self::path(dir);
        let raw = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_file(&path);
        let raw = raw?;
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
        let path = Self::path(dir);
        if self.agents.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        write_atomically(&path, &self.body()?)
    }

    fn body(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| format!("serialize the resume roster: {error}"))
    }

    /// Drop any roster without reading it — the opt-out's shutdown half. The
    /// live roster goes with it: an opted-out roll must not be resumed from
    /// the file a crash would have been.
    pub fn forget(dir: &Path) {
        let _ = std::fs::remove_file(Self::path(dir));
        let _ = std::fs::remove_file(live_path(dir));
    }
}

fn live_path(dir: &Path) -> PathBuf {
    dir.join(LIVE_ROSTER_FILE)
}

/// Replace `path` with `body` so that a reader — the next boot, after any kind
/// of death — finds the old file or the new one and never half of either: a
/// temp file beside it, synced, renamed over it, and the directory synced so
/// the rename itself survives the power going.
fn write_atomically(path: &Path, body: &str) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} names no file", path.display()))?;
    let temp = path.with_file_name(format!(".{}.tmp", name.to_string_lossy()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        if let Some(dir) = path.parent() {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok::<(), std::io::Error>(())
    })();
    written.map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("write {}: {error}", path.display())
    })
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
    let live = live_path(dir);
    if !live.exists() {
        return false;
    }
    if ResumeRoster::path(dir).exists() {
        let _ = std::fs::remove_file(&live);
        return false;
    }
    match std::fs::rename(&live, ResumeRoster::path(dir)) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("resume: could not promote {}: {error}", live.display());
            false
        }
    }
}

/// The live roster: the set of agents a crash would have to bring back, on
/// disk for as long as the daemon runs.
///
/// [`publish`](Self::publish) is called with the app mutex held, so it does no
/// filesystem work at all: it swaps the newest list into a slot and wakes a
/// thread of its own, which writes with every lock released. Lists published
/// while a write is in flight collapse into the newest one — the file only
/// ever has to hold the last.
#[derive(Clone)]
pub struct LiveRoster {
    shared: Arc<LiveShared>,
}

struct LiveShared {
    dir: PathBuf,
    version: String,
    slot: Mutex<LiveSlot>,
    changed: Condvar,
}

#[derive(Default)]
struct LiveSlot {
    /// The newest list published, written or not.
    latest: Vec<ResumingAgent>,
    /// Whether `latest` has not reached the file yet.
    unwritten: bool,
    /// Whether the writer is between taking a list and finishing with it.
    writing: bool,
    /// Set by the clean shutdown; nothing published after it is written.
    finished: bool,
}

impl LiveRoster {
    /// Start the writer for `dir`. Call [`promote_live_roster`] first: the
    /// first list published overwrites whatever the last run left.
    pub fn start(dir: &Path, version: &str) -> LiveRoster {
        let live = LiveRoster {
            shared: Arc::new(LiveShared {
                dir: dir.to_path_buf(),
                version: version.to_string(),
                slot: Mutex::new(LiveSlot::default()),
                changed: Condvar::new(),
            }),
        };
        let writer = live.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("live-roster".to_string())
            .spawn(move || writer.write_until_finished())
        {
            eprintln!("resume: no live roster this run ({error}); only a clean shutdown records");
            live.shared.slot.lock().unwrap().finished = true;
        }
        live
    }

    /// Say who is working now. Cheap and lock-safe: an unchanged list is
    /// dropped here, and a changed one is only handed to the writer.
    pub fn publish(&self, agents: Vec<ResumingAgent>) {
        let mut slot = self.shared.slot.lock().unwrap();
        if slot.finished || slot.latest == agents {
            return;
        }
        slot.latest = agents;
        slot.unwritten = true;
        self.shared.changed.notify_all();
    }

    fn write_until_finished(&self) {
        let mut slot = self.shared.slot.lock().unwrap();
        loop {
            if slot.unwritten {
                let agents = slot.latest.clone();
                slot.unwritten = false;
                slot.writing = true;
                drop(slot);
                if let Err(error) = self.write(agents) {
                    eprintln!("resume: could not write the live roster: {error}");
                }
                slot = self.shared.slot.lock().unwrap();
                slot.writing = false;
                self.shared.changed.notify_all();
                continue;
            }
            if slot.finished {
                return;
            }
            slot = self.shared.changed.wait(slot).unwrap();
        }
    }

    fn write(&self, agents: Vec<ResumingAgent>) -> Result<(), String> {
        let path = live_path(&self.shared.dir);
        if agents.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("remove {}: {error}", path.display()))
                }
                _ => Ok(()),
            };
        }
        let roster = ResumeRoster {
            recorded_at: crate::store::now_rfc3339(),
            version: self.shared.version.clone(),
            agents,
        };
        write_atomically(&path, &roster.body()?)
    }

    /// The clean shutdown: stop the writer, write the roster proper from the
    /// newest list, and only then remove the live file.
    ///
    /// The list is the one the last mutation published, so this never waits
    /// on the app mutex — a handler holding it for a minute cannot hold the
    /// shutdown past systemd's patience. A write already in flight is waited
    /// for, so it cannot land after the removal and leave a live roster for a
    /// shutdown that was clean. `wanted` is the opt-out: `false` records
    /// nobody and leaves neither file behind.
    ///
    /// Returns how many agents were recorded. On an error the live roster is
    /// left where it is, so the next boot still resumes from it.
    pub fn finish(&self, wanted: bool) -> Result<usize, String> {
        let agents = {
            let mut slot = self.shared.slot.lock().unwrap();
            slot.finished = true;
            self.shared.changed.notify_all();
            while slot.writing {
                slot = self.shared.changed.wait(slot).unwrap();
            }
            slot.unwritten = false;
            slot.latest.clone()
        };
        let dir = &self.shared.dir;
        if !wanted {
            ResumeRoster::forget(dir);
            return Ok(0);
        }
        let count = agents.len();
        ResumeRoster {
            recorded_at: crate::store::now_rfc3339(),
            version: self.shared.version.clone(),
            agents,
        }
        .save(dir)?;
        let _ = std::fs::remove_file(live_path(dir));
        Ok(count)
    }

    /// The whole of the shutdown half, as the daemon calls it: honour the
    /// opt-out, [`finish`](Self::finish), and say what happened on stderr.
    ///
    /// Silent about its own failure beyond that line: a roster that could not
    /// be written costs the next boot its resume, and must not cost this
    /// shutdown its exit — the harnesses are already dying and the store is
    /// already durable.
    pub fn record_at_shutdown(&self) {
        let wanted = resume_is_wanted(&self.shared.dir, |key| std::env::var(key).ok());
        match self.finish(wanted) {
            Ok(_) if !wanted => eprintln!("resume: opted out of this roll; recording nobody"),
            Ok(0) => eprintln!("resume: no agent was working; recorded nobody"),
            Ok(count) => eprintln!("resume: recorded {count} agent(s) to bring back"),
            Err(error) => eprintln!(
                "resume: could not record the roster: {error}; the live roster stays for the next boot"
            ),
        }
    }

    /// Block until everything published so far is on disk. For tests, which
    /// read the file the writer thread writes.
    #[cfg(test)]
    pub fn settle(&self) {
        let mut slot = self.shared.slot.lock().unwrap();
        while slot.unwritten || slot.writing {
            slot = self.shared.changed.wait(slot).unwrap();
        }
    }
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

    fn working(agent_id: &str) -> ResumingAgent {
        ResumingAgent {
            entity_id: "run-1".to_string(),
            agent_id: agent_id.to_string(),
            conversation_id: agent_id.to_string(),
            resume_session_id: Some(format!("sess-{agent_id}")),
            was_working: true,
        }
    }

    fn live_file(dir: &Path) -> Option<ResumeRoster> {
        let raw = std::fs::read_to_string(live_path(dir)).ok()?;
        Some(serde_json::from_str(&raw).expect("the live roster parses"))
    }

    /// Every file in the tasks directory, so a test can say no temp file was
    /// left beside the roster.
    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// While the daemon runs, who is working is on disk — not only at the
    /// shutdown that a SIGKILL never runs.
    #[test]
    fn a_published_list_is_on_disk_in_the_live_roster() {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.publish(vec![working("agent-1")]);
        live.settle();

        let written = live_file(dir.path()).expect("the live roster was written");
        assert_eq!(written.agents, vec![working("agent-1")]);
        assert_eq!(written.version, "0.2.2");
        assert!(
            !ResumeRoster::path(dir.path()).exists(),
            "the roster proper is the clean shutdown's alone"
        );
        assert_eq!(files_in(dir.path()), vec![LIVE_ROSTER_FILE.to_string()]);
    }

    /// The newest list replaces the last, and nobody working leaves no file.
    #[test]
    fn the_live_roster_follows_the_newest_list_and_goes_when_it_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.publish(vec![working("agent-1")]);
        live.publish(vec![working("agent-1"), working("agent-2")]);
        live.settle();
        assert_eq!(live_file(dir.path()).unwrap().agents.len(), 2);

        live.publish(Vec::new());
        live.settle();
        assert!(!live_path(dir.path()).exists());
        assert!(
            files_in(dir.path()).is_empty(),
            "{:?}",
            files_in(dir.path())
        );
    }

    /// The hard kill: the writer is simply never told to finish, which is all
    /// a SIGKILL, a panic or the power going looks like from the disk. The
    /// next boot promotes what the live roster held and resumes it.
    #[test]
    fn a_death_that_runs_no_shutdown_leaves_a_roster_for_the_next_boot() {
        let dir = tempfile::tempdir().unwrap();
        {
            let live = LiveRoster::start(dir.path(), "0.2.2");
            live.publish(vec![working("agent-1")]);
            live.settle();
        }

        assert!(
            promote_live_roster(dir.path()),
            "the live roster is promoted"
        );
        assert!(!live_path(dir.path()).exists());
        let roster = ResumeRoster::take(dir.path()).expect("a roster to resume from");
        assert_eq!(roster.agents, vec![working("agent-1")]);
    }

    /// The clean shutdown writes the roster proper BEFORE it returns — the
    /// daemon exits straight after — and clears the live file only after that
    /// write. Nothing published afterwards, by harnesses dying on their way
    /// down, brings the live file back.
    #[test]
    fn the_clean_shutdown_writes_the_roster_before_returning_then_clears_the_live_one() {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.publish(vec![working("agent-1")]);
        live.publish(vec![working("agent-1"), working("agent-2")]);

        assert_eq!(live.finish(true), Ok(2));
        let roster = ResumeRoster::take(dir.path()).expect("the shutdown wrote a roster");
        assert_eq!(
            roster.agents,
            vec![working("agent-1"), working("agent-2")],
            "the newest list, written or not"
        );
        live.publish(Vec::new());
        live.publish(vec![working("agent-3")]);
        live.settle();
        assert!(!live_path(dir.path()).exists());
        assert!(
            files_in(dir.path()).is_empty(),
            "{:?}",
            files_in(dir.path())
        );
        assert!(!promote_live_roster(dir.path()));
    }

    /// Races the writer: a list published a moment before shutdown may be
    /// mid-write when `finish` runs, and that write must not land after the
    /// live file has been removed.
    #[test]
    fn a_write_in_flight_at_shutdown_never_outlives_it() {
        for round in 0..50 {
            let dir = tempfile::tempdir().unwrap();
            let live = LiveRoster::start(dir.path(), "0.2.2");
            live.publish(vec![working(&format!("agent-{round}"))]);
            assert_eq!(live.finish(true), Ok(1));
            std::thread::sleep(std::time::Duration::from_millis(1));
            assert!(!live_path(dir.path()).exists(), "round {round}");
            assert!(ResumeRoster::path(dir.path()).exists(), "round {round}");
        }
    }

    /// Opting out of a roll records nobody at either file.
    #[test]
    fn an_opted_out_shutdown_leaves_neither_roster() {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.publish(vec![working("agent-1")]);
        live.settle();

        assert_eq!(live.finish(false), Ok(0));
        assert!(
            files_in(dir.path()).is_empty(),
            "{:?}",
            files_in(dir.path())
        );
    }

    /// Both on disk means the shutdown wrote its roster and died before it
    /// removed the live one: the roster is the later word.
    #[test]
    fn a_clean_roster_wins_over_a_live_one_at_boot() {
        let dir = tempfile::tempdir().unwrap();
        roster().save(dir.path()).unwrap();
        std::fs::write(live_path(dir.path()), "{ \"stale\": true }").unwrap();

        assert!(!promote_live_roster(dir.path()));
        assert!(!live_path(dir.path()).exists());
        assert_eq!(ResumeRoster::take(dir.path()), Some(roster()));
    }

    /// The files are addresses, not content: ids, a session id, a time and a
    /// version. Nothing an agent or a person said can ride along, because the
    /// record has nowhere to put it.
    #[test]
    fn the_roster_file_holds_ids_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.publish(vec![working("agent-1")]);
        live.settle();

        let raw = std::fs::read_to_string(live_path(dir.path())).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let mut top: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        top.sort_unstable();
        assert_eq!(top, ["agents", "recorded_at", "version"]);
        for agent in value["agents"].as_array().unwrap() {
            let mut keys: Vec<&str> = agent
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "agent_id",
                    "conversation_id",
                    "entity_id",
                    "resume_session_id",
                    "was_working"
                ]
            );
        }
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
