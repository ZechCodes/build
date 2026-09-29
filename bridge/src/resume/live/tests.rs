use super::super::disk::testing::RecordingDisk;
use super::super::{promote_live_roster_on, LIVE_ROSTER_FILE, ROSTER_FILE};
use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};

fn working(agent_id: &str) -> ResumingAgent {
    working_on("run-1", agent_id)
}

fn working_on(entity_id: &str, agent_id: &str) -> ResumingAgent {
    ResumingAgent {
        entity_id: entity_id.to_string(),
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

fn recorded(dir: &Path) -> (LiveRoster, Arc<RecordingDisk>) {
    let disk = Arc::new(RecordingDisk::default());
    let live = LiveRoster::start_with(dir, "0.2.2", disk.clone(), spawn_thread);
    (live, disk)
}

fn position(log: &[String], entry: &str) -> usize {
    log.iter()
        .position(|line| line == entry)
        .unwrap_or_else(|| panic!("{entry} is not in {log:?}"))
}

/// While the daemon runs, who is working is on disk — not only at the
/// shutdown that a SIGKILL never runs.
#[test]
fn a_published_list_is_on_disk_in_the_live_roster() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start(dir.path(), "0.2.2");
    live.set_entity("run-1", vec![working("agent-1")]);
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

/// The newest list replaces the last, and nobody working leaves no file —
/// removed durably, because the file it removes is a complete roster a power
/// cut would otherwise bring back.
#[test]
fn the_live_roster_follows_the_newest_list_and_is_durably_removed_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    live.set_entity("run-1", vec![working("agent-1")]);
    live.set_entity("run-1", vec![working("agent-1"), working("agent-2")]);
    live.settle();
    assert_eq!(live_file(dir.path()).unwrap().agents.len(), 2);

    disk.clear_log();
    live.forget_entity("run-1");
    live.settle();
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
    assert_eq!(
        disk.log(),
        [format!("unlink {LIVE_ROSTER_FILE}"), "sync-dir".to_string()]
    );
}

/// Each entity keeps the list it last settled with. A transaction that has
/// one entity checked out while another settles must not drop the first
/// entity's agents from the file.
#[test]
fn an_entity_keeps_its_list_while_another_entity_changes() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start(dir.path(), "0.2.2");
    live.set_entity("run-1", vec![working_on("run-1", "agent-1")]);
    live.set_entity("plan-1", vec![working_on("plan-1", "agent-2")]);
    live.set_entity("plan-1", Vec::new());
    live.settle();
    assert_eq!(
        live_file(dir.path()).unwrap().agents,
        vec![working_on("run-1", "agent-1")]
    );
}

/// The hard kill, in process: the writer is simply never told to finish. The
/// next boot promotes what the live roster held and resumes it — durably, so
/// a power cut straight after cannot put the live file back.
#[test]
fn a_death_that_runs_no_shutdown_leaves_a_roster_for_the_next_boot() {
    let dir = tempfile::tempdir().unwrap();
    {
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.set_entity("run-1", vec![working("agent-1")]);
        live.settle();
    }

    let disk = RecordingDisk::default();
    assert_eq!(promote_live_roster_on(&disk, dir.path()), Ok(true));
    assert_eq!(
        disk.log(),
        [
            format!("rename {LIVE_ROSTER_FILE} {ROSTER_FILE}"),
            "sync-dir".to_string()
        ]
    );
    disk.clear_log();
    let roster = ResumeRoster::take_on(&disk, dir.path()).expect("a roster to resume from");
    assert_eq!(roster.agents, vec![working("agent-1")]);
    assert_eq!(
        disk.log(),
        [format!("unlink {ROSTER_FILE}"), "sync-dir".to_string()],
        "consuming it is durable too: a power cut must not resume it twice"
    );
}

/// The clean shutdown writes the roster proper, durably, BEFORE it removes the
/// live file, and both before it returns — the daemon exits straight after.
/// Nothing published afterwards, by harnesses dying on their way down, brings
/// the live file back.
#[test]
fn the_clean_shutdown_writes_the_roster_durably_then_removes_the_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    live.set_entity("run-1", vec![working("agent-1")]);
    live.set_entity("run-2", vec![working_on("run-2", "agent-2")]);

    assert_eq!(live.finish(true), Ok(2));
    let log = disk.log();
    let renamed = position(&log, &format!("rename .{ROSTER_FILE}.tmp {ROSTER_FILE}"));
    let removed = position(&log, &format!("unlink {LIVE_ROSTER_FILE}"));
    assert_eq!(log[renamed + 1], "sync-dir", "{log:?}");
    assert!(renamed < removed, "{log:?}");
    assert_eq!(log.last().unwrap(), "sync-dir", "{log:?}");

    let roster = ResumeRoster::take(dir.path()).expect("the shutdown wrote a roster");
    assert_eq!(
        roster.agents,
        vec![working("agent-1"), working_on("run-2", "agent-2")],
        "the newest lists, written or not"
    );
    live.set_entity("run-1", vec![working("agent-3")]);
    live.settle();
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
}

/// Races the writer: a list published a moment before shutdown may be
/// mid-write when `finish` runs, and that write must not land after the
/// live file has been removed.
#[test]
fn a_write_in_flight_at_shutdown_never_outlives_it() {
    for round in 0..50 {
        let dir = tempfile::tempdir().unwrap();
        let live = LiveRoster::start(dir.path(), "0.2.2");
        live.set_entity("run-1", vec![working(&format!("agent-{round}"))]);
        assert_eq!(live.finish(true), Ok(1));
        std::thread::sleep(Duration::from_millis(1));
        assert!(!live_path(dir.path()).exists(), "round {round}");
        assert!(ResumeRoster::path(dir.path()).exists(), "round {round}");
    }
}

/// Opting out of a roll records nobody at either file, durably.
#[test]
fn an_opted_out_shutdown_durably_leaves_neither_roster() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    live.set_entity("run-1", vec![working("agent-1")]);
    live.settle();
    disk.clear_log();

    assert_eq!(live.finish(false), Ok(0));
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
    assert_eq!(
        disk.log(),
        [
            format!("unlink {ROSTER_FILE}"),
            "sync-dir".to_string(),
            format!("unlink {LIVE_ROSTER_FILE}"),
            "sync-dir".to_string()
        ]
    );
}

/// An empty clean shutdown removes an older roster durably.
#[test]
fn an_empty_clean_shutdown_durably_removes_both_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(ResumeRoster::path(dir.path()), "{}").unwrap();
    let (live, disk) = recorded(dir.path());
    assert_eq!(live.finish(true), Ok(0));
    assert!(
        files_in(dir.path()).is_empty(),
        "{:?}",
        files_in(dir.path())
    );
    assert_eq!(
        disk.log(),
        [
            format!("unlink {ROSTER_FILE}"),
            "sync-dir".to_string(),
            format!("unlink {LIVE_ROSTER_FILE}"),
            "sync-dir".to_string()
        ]
    );
}

/// A shutdown whose final removal cannot be made durable says so, and the
/// caller keeps the live file's word for the next boot.
#[test]
fn a_shutdown_that_cannot_sync_its_removal_reports_it() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    disk.fail_next_dir_syncs(1);
    assert!(live.finish(true).is_err());
}

/// Both on disk means the shutdown wrote its roster and died before it
/// removed the live one: the roster is the later word, and the live file goes
/// durably.
#[test]
fn a_clean_roster_wins_over_a_live_one_at_boot() {
    let dir = tempfile::tempdir().unwrap();
    let roster = ResumeRoster {
        recorded_at: "2026-09-20T03:00:00Z".to_string(),
        version: "0.2.0".to_string(),
        agents: vec![working("agent-1")],
    };
    roster.save(dir.path()).unwrap();
    std::fs::write(live_path(dir.path()), "{ \"stale\": true }").unwrap();

    let disk = RecordingDisk::default();
    assert_eq!(promote_live_roster_on(&disk, dir.path()), Ok(false));
    assert_eq!(
        disk.log(),
        [format!("unlink {LIVE_ROSTER_FILE}"), "sync-dir".to_string()]
    );
    assert_eq!(ResumeRoster::take(dir.path()), Some(roster));
}

/// A write that fails is retried with nothing changing — the list the app
/// published once is the list that reaches the disk once the disk recovers.
#[test]
fn a_failed_write_is_retried_until_it_lands_without_a_new_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    disk.fail_next_creates(3);
    live.set_entity("run-1", vec![working("agent-1")]);
    live.settle();

    assert_eq!(
        live_file(dir.path()).unwrap().agents,
        vec![working("agent-1")]
    );
    let attempts: Vec<String> = disk
        .log()
        .into_iter()
        .filter(|line| line.starts_with("create"))
        .collect();
    let failed = format!("create-failed .{LIVE_ROSTER_FILE}.tmp");
    let landed = format!("create .{LIVE_ROSTER_FILE}.tmp");
    assert_eq!(attempts, [failed.clone(), failed.clone(), failed, landed]);
    assert_eq!(files_in(dir.path()), vec![LIVE_ROSTER_FILE.to_string()]);
}

#[test]
fn the_retry_pause_doubles_up_to_its_ceiling() {
    assert_eq!(retry_pause(1), RETRY_PAUSE_FIRST);
    assert_eq!(retry_pause(2), RETRY_PAUSE_FIRST * 2);
    assert_eq!(retry_pause(3), RETRY_PAUSE_FIRST * 4);
    assert_eq!(retry_pause(40), RETRY_PAUSE_MAX);
    assert_eq!(retry_pause(u32::MAX), RETRY_PAUSE_MAX);
}

/// No writer thread costs the crash half only: the lists are still kept, and
/// the clean shutdown still records every working agent.
#[test]
fn a_writer_that_never_started_still_leaves_the_clean_shutdown_its_lists() {
    fn refuse(_: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
        Err(std::io::Error::other("no threads today"))
    }
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start_with(dir.path(), "0.2.2", Arc::new(RealDisk), refuse);
    live.set_entity("run-1", vec![working("agent-1")]);
    live.settle();
    assert!(!live_path(dir.path()).exists(), "nothing writes it");

    assert_eq!(live.finish(true), Ok(1));
    assert_eq!(
        ResumeRoster::take(dir.path()).unwrap().agents,
        vec![working("agent-1")]
    );
}

/// A writer that dies mid-write cannot leave the shutdown waiting on a write
/// that will never finish.
#[test]
fn a_writer_that_panics_mid_write_does_not_hold_up_the_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (live, disk) = recorded(dir.path());
    disk.panic_on_next_create();
    live.set_entity("run-1", vec![working("agent-1")]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while live.slot().writer_running {
        assert!(Instant::now() < deadline, "the writer never died");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!live.slot().writing);

    assert_eq!(live.finish(true), Ok(1));
    assert!(ResumeRoster::path(dir.path()).exists());
}

/// What the daemon's way down promises: the roster is on disk before the
/// teardown starts, and the teardown still runs.
#[test]
fn the_shutdown_records_the_roster_before_tearing_anything_down() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start(dir.path(), "0.2.2");
    live.set_entity("run-1", vec![working("agent-1")]);
    let mut roster_at_teardown = None;
    shut_down(&live, || {
        roster_at_teardown = Some(ResumeRoster::path(dir.path()).exists());
    });
    assert_eq!(roster_at_teardown, Some(true));
    assert!(!live_path(dir.path()).exists());
}

/// #213: the teardown stops the harnesses, and each death settles its turn as
/// over. Those settles land after the seal, so the roster the next boot reads
/// still names the agent that was working when the shutdown began.
#[test]
fn harnesses_dying_in_the_teardown_do_not_empty_the_recorded_roster() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start(dir.path(), "0.2.2");
    live.set_entity("run-1", vec![working("agent-1")]);
    shut_down(&live, || {
        live.set_entity("run-1", Vec::new());
        live.forget_entity("run-1");
    });

    let roster = ResumeRoster::take(dir.path()).expect("the shutdown wrote a roster");
    assert_eq!(roster.agents, vec![working("agent-1")]);
}

/// The files are addresses, not content: ids, a session id, a time and a
/// version. Nothing an agent or a person said can ride along, because the
/// record has nowhere to put it.
#[test]
fn the_roster_file_holds_ids_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let live = LiveRoster::start(dir.path(), "0.2.2");
    live.set_entity("run-1", vec![working("agent-1")]);
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

// ---- Real process death ---------------------------------------------------
//
// The tests above end the writer by dropping it. These end a whole process:
// this test binary re-run as a child, killed with SIGKILL, and the parent
// reads what the child left in the tasks directory.

const CHILD_SCENARIO: &str = "BUILD_TEST_LIVE_ROSTER_CHILD";
const CHILD_DIR: &str = "BUILD_TEST_LIVE_ROSTER_DIR";

/// The child's half. A no-op in an ordinary run of the suite.
#[test]
fn child_process_entry() {
    let (Ok(scenario), Ok(dir)) = (std::env::var(CHILD_SCENARIO), std::env::var(CHILD_DIR)) else {
        return;
    };
    let dir = Path::new(&dir);
    let live = LiveRoster::start(dir, "0.2.2");
    match scenario.as_str() {
        "working" => {
            live.set_entity("run-1", vec![working("agent-1")]);
            live.settle();
            println!("READY");
            loop {
                std::thread::park();
            }
        }
        "churn" => {
            let mut round = 0u64;
            loop {
                let agents = (0..=(round % 4))
                    .map(|n| working(&format!("agent-{round}-{n}")))
                    .collect();
                live.set_entity("run-1", agents);
                if round == 0 {
                    live.settle();
                    println!("READY");
                }
                round += 1;
            }
        }
        "clean" => {
            live.set_entity("run-1", vec![working("agent-1")]);
            shut_down(&live, || {
                println!("TEARDOWN roster={}", ResumeRoster::path(dir).exists());
            });
        }
        other => panic!("no scenario {other}"),
    }
}

/// A child that is killed however the test ends, so a failing assertion
/// cannot leave one parked forever with the suite waiting on it.
struct ChildProcess(Child);

impl Drop for ChildProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_child(scenario: &str, dir: &Path) -> (ChildProcess, BufReader<ChildStdout>) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "resume::live::tests::child_process_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_SCENARIO, scenario)
        .env(CHILD_DIR, dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    (ChildProcess(child), stdout)
}

/// What the child said from `word` on. The harness prints `test … ... ` with
/// no newline before the test's own output, so the word is found anywhere in
/// the line.
fn said(stdout: &mut BufReader<ChildStdout>, word: &str) -> String {
    for line in stdout.lines() {
        let line = line.unwrap();
        if let Some(at) = line.find(word) {
            return line[at..].to_string();
        }
    }
    panic!("the child ended without saying {word}");
}

/// SIGKILL with an agent working: the next boot finds it.
#[test]
fn a_process_killed_mid_turn_leaves_its_agents_for_the_next_boot() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, mut stdout) = spawn_child("working", dir.path());
    said(&mut stdout, "READY");
    child.0.kill().unwrap();
    child.0.wait().unwrap();

    assert!(super::super::promote_live_roster(dir.path()));
    let roster = ResumeRoster::take(dir.path()).expect("a roster to resume from");
    assert_eq!(roster.agents, vec![working("agent-1")]);
}

/// SIGKILL while the writer is rewriting the file as fast as it can: whatever
/// moment the kill lands, the live roster is a whole roster or absent — a
/// torn one would cost the boot its resume.
#[test]
fn a_process_killed_mid_write_never_leaves_a_torn_roster() {
    for round in 0..8u64 {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, mut stdout) = spawn_child("churn", dir.path());
        said(&mut stdout, "READY");
        std::thread::sleep(Duration::from_millis(3 * round + 1));
        child.0.kill().unwrap();
        child.0.wait().unwrap();

        let raw = std::fs::read_to_string(live_path(dir.path()))
            .unwrap_or_else(|error| panic!("round {round}: no live roster: {error}"));
        let roster: ResumeRoster = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("round {round}: torn ({error}): {raw}"));
        assert!(!roster.agents.is_empty(), "round {round}");
        assert!(super::super::promote_live_roster(dir.path()));
        assert_eq!(
            ResumeRoster::take(dir.path()),
            Some(roster),
            "round {round}"
        );
    }
}

/// A process that goes down through `shut_down` has its roster on disk before
/// its teardown runs, and leaves no live roster behind.
#[test]
fn a_process_that_shuts_down_cleanly_records_before_teardown() {
    let dir = tempfile::tempdir().unwrap();
    let (mut child, mut stdout) = spawn_child("clean", dir.path());
    assert_eq!(said(&mut stdout, "TEARDOWN"), "TEARDOWN roster=true");
    assert!(child.0.wait().unwrap().success());

    assert!(!live_path(dir.path()).exists());
    assert_eq!(
        ResumeRoster::take(dir.path()).unwrap().agents,
        vec![working("agent-1")]
    );
}
