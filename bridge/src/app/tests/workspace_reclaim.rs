//! The workspace reclaim service and `workspace.reclaim` (#135), end to end
//! over a real project, workspace and tracker.

use super::project_agent::{rooted, workspace};
use super::tracker::{filed, tracked_with_origin};
use super::*;
use crate::reclaim::{DigestSchedule, DigestTime, ReclaimPolicy};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// The daily digest at `hour`:00 UTC, whatever the machine's zone (#958).
fn digest_at(hour: i8) -> DigestSchedule {
    DigestSchedule {
        at: DigestTime::new(hour, 0).unwrap(),
        zone: Some(jiff::tz::TimeZone::UTC),
    }
}

/// The default policy with its digest due all day, at midnight UTC: for
/// tests about something other than when the digest goes out.
pub(super) fn daily() -> ReclaimPolicy {
    ReclaimPolicy {
        digest: digest_at(0),
        ..ReclaimPolicy::default()
    }
}

/// Everything is idle at once: the threshold is zero. Pruning stays off, as
/// it is unless `BRIDGE_WORKSPACE_PRUNE` is set. The digest is due all day.
pub(super) fn impatient() -> ReclaimPolicy {
    ReclaimPolicy {
        idle_after: std::time::Duration::ZERO,
        ..daily()
    }
}

/// Midnight UTC two days from now: far enough ahead that a workspace made
/// now is past the default 24-hour threshold all that day.
fn quiet_day() -> i64 {
    (now_ms() / DAY_MS + 2) * DAY_MS
}

/// [`impatient`], with pruning switched on.
pub(super) fn pruning() -> ReclaimPolicy {
    ReclaimPolicy {
        prune: true,
        ..impatient()
    }
}

pub(super) fn now_ms() -> i64 {
    i64::try_from(crate::agent::now_ms()).unwrap()
}

fn task_id(task: &Value) -> String {
    task["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn shutdown_can_cancel_reclaim_while_the_app_mutex_is_held() {
    let (_tmp, state, _project, _ws, _task) = linked_workspace();
    let stop = AppState::spawn_workspace_reclaim(
        state.clone(),
        ReclaimPolicy {
            first_sweep_after: std::time::Duration::from_secs(24 * 60 * 60),
            ..ReclaimPolicy::default()
        },
    )
    .await;
    let app = state.lock().unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(app.reclaim_stop.load(std::sync::atomic::Ordering::Relaxed));
}

/// A project over a repository with an origin, one workspace in it, and one
/// task linked to that workspace: `(tempdir, state, project, workspace, task)`.
pub(super) fn linked_workspace() -> (
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    String,
    String,
    String,
) {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let (mut state, project_id) = tracked_with_origin(&state_root);
    let ws = workspace(&mut state, &project_id, "quiet");
    let task = task_id(&filed(&mut state, &project_id, "Quiet work"));
    let linked = link_task(&mut state, json!({ "task_id": task, "workspace_id": ws }));
    assert_eq!(linked["ok"], true, "{linked:?}");
    // Deterministic terminals: plain bash, whatever the machine's login shell.
    state.term_shell = "/bin/bash".into();
    (tmp, state.shared(), project_id, ws, task)
}

pub(super) fn call(state: &Arc<Mutex<AppState>>, method: &str, params: Value) -> Value {
    state.lock().unwrap().handle(req(method, params))
}

pub(super) fn lifecycle(state: &Arc<Mutex<AppState>>, ws: &str) -> Value {
    let listed = call(state, "workspace.list", json!({}));
    listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["workspace_id"] == ws)
        .map(|row| row["lifecycle"].clone())
        .unwrap_or_else(|| panic!("no row for {ws}: {listed:?}"))
}

pub(super) fn timeline_kinds(state: &Arc<Mutex<AppState>>, task: &str) -> Vec<Value> {
    call(state, "tasks.get", json!({ "task_id": task }))["result"]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("workspace_"))
        })
        .cloned()
        .collect()
}

/// What Build said on the project agent's conversation.
fn project_agent_notices(state: &Arc<Mutex<AppState>>, project_id: &str) -> Vec<String> {
    let owner = call(
        state,
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    )["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let agents = agent_roster(&mut state.lock().unwrap(), json!({ "entity_id": owner }));
    let Some(agent_id) = agents["result"]["agents"][0]["id"]
        .as_str()
        .map(str::to_string)
    else {
        return Vec::new();
    };
    let page = call(
        state,
        "thread.page",
        json!({ "entity_id": owner, "agent_id": agent_id, "limit": 50 }),
    );
    page["result"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item["type"] == "message" && item["data"]["from_build"] == true)
        .map(|item| {
            item["data"]["body"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Move the linked task to Done.
pub(super) fn finish(state: &Arc<Mutex<AppState>>, task: &str) {
    let moved = call(
        state,
        "tasks.update",
        json!({ "task_id": task, "status": "done" }),
    );
    assert_eq!(moved["ok"], true, "{moved:?}");
}

/// The workspace's root, and its one checkout.
pub(super) fn root_and_checkout(state: &Arc<Mutex<AppState>>, ws: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(
        call(state, "workspace.get", json!({ "workspace_id": ws }))["result"]["root"]
            .as_str()
            .unwrap(),
    );
    let checkout = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.join(".git").exists())
        .expect("the workspace's checkout");
    (root, checkout)
}

/// Give the checkout build output its repository ignores, without committing
/// anything the remote does not have: `checkout/node_modules`.
pub(super) fn build_output_in(checkout: &Path) -> PathBuf {
    let dot_git = checkout.join(".git");
    let exclude = if dot_git.is_dir() {
        dot_git.join("info/exclude")
    } else {
        let gitdir = std::fs::read_to_string(&dot_git).unwrap();
        let common = PathBuf::from(gitdir.trim().trim_start_matches("gitdir: "));
        common.join("../../info/exclude")
    };
    std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::fs::write(&exclude, "node_modules/\n").unwrap();
    let output = checkout.join("node_modules/pkg");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(output.join("index.js"), vec![1u8; 32 * 1024]).unwrap();
    checkout.join("node_modules")
}

fn assert_says_everything(notice: &str, phrases: &[&str]) {
    for phrase in phrases {
        assert!(notice.contains(phrase), "{phrase:?} missing from {notice}");
    }
}

/// Before any sweep there is no verdict, and the row says so with `null`.
#[test]
fn a_workspace_row_carries_no_verdict_before_the_first_sweep() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    assert_eq!(lifecycle(&state, &ws), Value::Null);
}

/// A quiet workspace whose task is still open is announced, but held: the
/// row names the open task, the project agent is told once, and the task's
/// timeline records Build noticing.
#[test]
fn a_quiet_workspace_is_announced_to_the_project_agent_and_on_its_task() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();

    AppState::sweep_workspaces(&state, &impatient(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["idle"], true, "{verdict:?}");
    assert_eq!(verdict["reclaimable"], false);
    assert_eq!(verdict["holds"], json!(["task_open"]));
    assert_eq!(verdict["tasks"][0]["number"], 1);
    assert!(verdict["noticed_at_ms"].is_i64(), "{verdict:?}");
    assert!(verdict["size_bytes"].as_u64().unwrap() > 0);

    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_says_everything(
        &notices[0],
        &[
            "1 workspace has had no activity",
            "quiet (",
            "#1 Quiet work (Backlog)",
            "a task linked to it is not Done",
            "Only message the user if necessary",
            "without posting to the user",
        ],
    );

    let entries = timeline_kinds(&state, &task);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "workspace_idle");
    assert_eq!(entries[0]["actor"], json!({ "kind": "build" }));
    assert_eq!(entries[0]["payload"]["workspace_id"], ws.as_str());
    assert_eq!(
        entries[0]["payload"]["idle_after_secs"], 0,
        "the threshold it went quiet by, which Settings can move (#167)"
    );

    // The next sweep the same day says nothing more.
    AppState::sweep_workspaces(&state, &daily(), now_ms());
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    assert_eq!(timeline_kinds(&state, &task).len(), 1);
}

/// A locked workspace is listed every day, in its own group with what else
/// holds it, beside its unlocked sibling (#958).
#[test]
fn locked_workspaces_are_listed_daily_in_their_own_group() {
    let (_tmp, state, project_id, locked, _task) = linked_workspace();
    let unlocked = workspace(&mut state.lock().unwrap(), &project_id, "unlocked");
    let reply = call(
        &state,
        "workspace.set_locked",
        json!({"workspace_id": locked, "locked": true}),
    );
    assert_eq!(reply["ok"], true, "{reply:?}");
    let start = quiet_day();
    for day in 0..3 {
        AppState::sweep_workspaces(&state, &daily(), start + day * DAY_MS);
        let notices = project_agent_notices(&state, &project_id);
        assert_eq!(notices.len(), (day + 1) as usize, "{notices:?}");
        let latest = notices.last().unwrap();
        let (open, held) = latest
            .split_once("Locked by the user, so kept on purpose.")
            .unwrap_or_else(|| panic!("no locked group in {latest}"));
        assert!(
            open.contains(&unlocked) && !open.contains(&locked),
            "{latest}"
        );
        assert_says_everything(
            held,
            &[
                &locked,
                "Do not reclaim these or ask the user to unlock them",
                "evaluate whether the user should be reminded",
                "kept because the user locked it; also held: a task linked to it is not Done",
            ],
        );
        assert!(!held.contains(&unlocked), "{latest}");
        assert_eq!(
            lifecycle(&state, &locked)["noticed_at_ms"],
            start + day * DAY_MS
        );
    }
}

/// Settle a sweep whose measurements were taken before the lock changed.
fn settle_after_lock_changes(state: &Arc<Mutex<AppState>>, ws: &str, locked: bool, at: i64) {
    let policy = daily();
    let subjects = state.lock().unwrap().reclaim_subjects();
    let measured = subjects
        .into_iter()
        .map(|subject| {
            let record = subject.measure(at, &policy, &policy.budget(Default::default()));
            (subject, record)
        })
        .collect();
    let reply = call(
        state,
        "workspace.set_locked",
        json!({"workspace_id": ws, "locked": locked}),
    );
    assert_eq!(reply["ok"], true, "{reply:?}");
    state
        .lock()
        .unwrap()
        .settle_workspace_sweep(measured, at, &policy);
}

/// A lock that lands while the sweep measures puts the workspace in the
/// locked group of that day's digest.
#[test]
fn locking_during_measurement_lists_the_workspace_as_locked() {
    let (_tmp, state, project_id, ws, _task) = linked_workspace();
    let sibling = workspace(&mut state.lock().unwrap(), &project_id, "sibling");
    let start = quiet_day();
    AppState::sweep_workspaces(&state, &daily(), start);

    settle_after_lock_changes(&state, &ws, true, start + DAY_MS);

    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 2, "{notices:?}");
    let latest = notices.last().unwrap();
    let (open, held) = latest
        .split_once("Locked by the user")
        .unwrap_or_else(|| panic!("no locked group in {latest}"));
    assert!(open.contains(&sibling) && !open.contains(&ws), "{latest}");
    assert!(held.contains(&ws), "{latest}");
    let record = lifecycle(&state, &ws);
    assert_eq!(record["noticed_at_ms"], start + DAY_MS);
    assert_eq!(record["holds"], json!(["locked", "task_open"]));
    assert_eq!(record["reclaimable"], false);
}

/// An unlock that lands while the sweep measures takes the workspace out of
/// the locked group.
#[test]
fn unlocking_during_measurement_lists_the_workspace_as_unlocked() {
    let (_tmp, state, project_id, ws, _task) = linked_workspace();
    let reply = call(
        &state,
        "workspace.set_locked",
        json!({"workspace_id": ws, "locked": true}),
    );
    assert_eq!(reply["ok"], true, "{reply:?}");
    let start = quiet_day();
    AppState::sweep_workspaces(&state, &daily(), start);

    settle_after_lock_changes(&state, &ws, false, start + DAY_MS);

    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("Locked by the user"), "{}", notices[0]);
    let latest = notices.last().unwrap();
    assert!(latest.contains(&ws), "{latest}");
    assert!(!latest.contains("Locked by the user"), "{latest}");
    let record = lifecycle(&state, &ws);
    assert_eq!(record["noticed_at_ms"], start + DAY_MS);
    assert_eq!(record["holds"], json!(["task_open"]));
    assert_eq!(
        record["reclaimable"], false,
        "the unrelated task hold stays"
    );
}

// --------------------------------------------------- the daily digest ---
// #958: each sweep is given its time, and the digest's time is 09:00 UTC.

fn at_nine() -> ReclaimPolicy {
    ReclaimPolicy {
        digest: digest_at(9),
        ..ReclaimPolicy::default()
    }
}

/// A workspace going quiet before the day's scan time sends nothing, and
/// writes nothing on its task, however many sweeps see it.
#[test]
fn a_workspace_going_quiet_sends_nothing_by_itself() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    let day = quiet_day();
    for at in [day + 7 * HOUR_MS, day + 8 * HOUR_MS, day + 9 * HOUR_MS - 1] {
        AppState::sweep_workspaces(&state, &at_nine(), at);
    }
    assert_eq!(lifecycle(&state, &ws)["idle"], true);
    assert_eq!(
        project_agent_notices(&state, &project_id),
        Vec::<String>::new()
    );
    assert_eq!(timeline_kinds(&state, &task), Vec::<Value>::new());
    assert_eq!(lifecycle(&state, &ws)["noticed_at_ms"], Value::Null);
}

/// The sweep at the scan time sends exactly one digest, and the day's later
/// sweeps send no other. The next day's lists the same workspace again, and
/// its task hears about it once.
#[test]
fn the_scan_at_its_time_sends_exactly_one_digest_a_day() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    let day = quiet_day();
    AppState::sweep_workspaces(&state, &at_nine(), day + 9 * HOUR_MS);
    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_says_everything(
        &notices[0],
        &["1 workspace has had no activity for 24 hours or more", &ws],
    );
    for at in [
        day + 10 * HOUR_MS,
        day + 23 * HOUR_MS,
        day + DAY_MS + 8 * HOUR_MS,
    ] {
        AppState::sweep_workspaces(&state, &at_nine(), at);
        assert_eq!(project_agent_notices(&state, &project_id).len(), 1, "{at}");
    }
    AppState::sweep_workspaces(&state, &at_nine(), day + DAY_MS + 9 * HOUR_MS);
    let notices = project_agent_notices(&state, &project_id);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[1].contains(&ws), "listed again: {}", notices[1]);
    let entries = timeline_kinds(&state, &task);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "workspace_idle");
}

/// The digest's turn is sent as the idle-workspaces message (#957), not as
/// a plain delivery of the thread.
#[test]
fn the_digest_is_sent_as_an_idle_workspaces_message() {
    let (_tmp, state, _project_id, _ws, _task) = linked_workspace();
    AppState::sweep_workspaces(&state, &impatient(), now_ms());
    let mut app = state.lock().unwrap();
    app.delivery_queue.lapse_settle_windows();
    let queued = app.delivery_queue.take_ready(|_| false);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].phase, "workspace_idle");
    assert_eq!(
        queued[0].say.as_ref().and_then(|say| say.kind),
        Some(crate::agent_message::MessageKind::IdleWorkspaces)
    );
}

/// A day with nothing quiet at its scan sends nothing, and a workspace that
/// goes quiet later that day waits for the next day's digest.
#[test]
fn nothing_is_sent_when_nothing_is_quiet() {
    let (_tmp, state, project_id, ws, _task) = linked_workspace();
    let day = quiet_day();
    let patient = ReclaimPolicy {
        idle_after: std::time::Duration::from_secs(1000 * 24 * 3600),
        ..at_nine()
    };
    AppState::sweep_workspaces(&state, &patient, day + 9 * HOUR_MS);
    assert_eq!(lifecycle(&state, &ws)["idle"], false);
    assert_eq!(
        project_agent_notices(&state, &project_id),
        Vec::<String>::new()
    );

    AppState::sweep_workspaces(&state, &at_nine(), day + 15 * HOUR_MS);
    assert_eq!(lifecycle(&state, &ws)["idle"], true);
    assert_eq!(
        project_agent_notices(&state, &project_id),
        Vec::<String>::new(),
        "crossing after the scan waits for tomorrow"
    );

    AppState::sweep_workspaces(&state, &at_nine(), day + DAY_MS + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
}

/// A bridge restarted after the day's digest went out does not send it
/// again: the day it was sent is in the store.
#[test]
fn a_restart_does_not_send_the_digest_twice() {
    let (tmp, state, project_id, _ws, _task) = linked_workspace();
    let day = quiet_day();
    AppState::sweep_workspaces(&state, &at_nine(), day + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    drop(state);

    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let reopened = rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let reopened = Arc::new(Mutex::new(reopened));
    AppState::sweep_workspaces(&reopened, &at_nine(), day + 10 * HOUR_MS);
    assert_eq!(project_agent_notices(&reopened, &project_id).len(), 1);
    AppState::sweep_workspaces(&reopened, &at_nine(), day + DAY_MS + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&reopened, &project_id).len(), 2);
}

/// A machine that moves west after its digest went out is back on a day it
/// has already had: it is not sent again.
#[test]
fn moving_zones_never_sends_a_day_twice() {
    let (_tmp, state, project_id, _ws, _task) = linked_workspace();
    let day = quiet_day();
    let in_zone = |hours: i8| ReclaimPolicy {
        digest: DigestSchedule {
            at: DigestTime::new(9, 0).unwrap(),
            zone: Some(jiff::tz::TimeZone::fixed(jiff::tz::offset(hours))),
        },
        ..ReclaimPolicy::default()
    };
    // 01:00Z the next day is 10:00 in Tokyo, and 17:00 the day before in
    // Los Angeles.
    let next_morning = day + DAY_MS + HOUR_MS;
    AppState::sweep_workspaces(&state, &in_zone(9), next_morning);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    AppState::sweep_workspaces(&state, &in_zone(-8), next_morning + HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
}

/// A clock set years ahead once, then put right, does not silence the
/// digest until the date catches up with the day it wrote.
#[test]
fn a_clock_set_far_ahead_and_corrected_still_sends() {
    let (_tmp, state, project_id, _ws, _task) = linked_workspace();
    let day = quiet_day();
    AppState::sweep_workspaces(&state, &at_nine(), day + 3650 * DAY_MS + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    AppState::sweep_workspaces(&state, &at_nine(), day + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 2);
    AppState::sweep_workspaces(&state, &at_nine(), day + 10 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 2);
}

/// A bridge that was down at the scan time sends the day's digest at its
/// first sweep after, once. A day missed entirely is not made up the next
/// morning before that day's time.
#[test]
fn a_missed_scan_is_caught_up_once() {
    let (_tmp, state, project_id, _ws, _task) = linked_workspace();
    let day = quiet_day();
    AppState::sweep_workspaces(&state, &at_nine(), day + 14 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    AppState::sweep_workspaces(&state, &at_nine(), day + 15 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);

    // The next day passes with the bridge down; the morning after, its
    // first sweep before 09:00 owes nothing.
    AppState::sweep_workspaces(&state, &at_nine(), day + 2 * DAY_MS + 8 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    AppState::sweep_workspaces(&state, &at_nine(), day + 2 * DAY_MS + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 2);
}

/// Put a file where the project's conversation scratch goes, so the project
/// agent cannot be reached and every digest to it fails to land; the
/// returned path is removed to let it through again.
fn block_project_agent(state: &Arc<Mutex<AppState>>, project_id: &str) -> PathBuf {
    let app = state.lock().unwrap();
    let repo = app.projects.get(project_id).unwrap().repo_path.clone();
    let scratch = crate::app::projects::scratch_dir(&app.state_root, repo.to_str().unwrap());
    std::fs::create_dir_all(scratch.parent().unwrap()).unwrap();
    std::fs::write(&scratch, "not a directory").unwrap();
    scratch
}

fn digest_days_stored(state: &Arc<Mutex<AppState>>) -> BTreeMap<String, String> {
    let app = state.lock().unwrap();
    app.store
        .as_ref()
        .unwrap()
        .load_idle_digests_sent()
        .unwrap()
}

/// A digest that fails to land counts as not sent: the day stays owed, the
/// workspace stays unlisted and its task unwritten, and the next sweep that
/// day sends it.
#[test]
fn a_digest_that_fails_to_land_is_sent_at_the_next_sweep() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    let day = quiet_day();
    let blocker = block_project_agent(&state, &project_id);
    AppState::sweep_workspaces(&state, &at_nine(), day + 9 * HOUR_MS);
    assert_eq!(lifecycle(&state, &ws)["idle"], true);
    assert_eq!(digest_days_stored(&state), BTreeMap::new());
    assert_eq!(lifecycle(&state, &ws)["noticed_at_ms"], Value::Null);
    assert_eq!(timeline_kinds(&state, &task), Vec::<Value>::new());

    std::fs::remove_file(blocker).unwrap();
    AppState::sweep_workspaces(&state, &at_nine(), day + 10 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &project_id).len(), 1);
    assert_eq!(
        lifecycle(&state, &ws)["noticed_at_ms"],
        json!(day + 10 * HOUR_MS)
    );
    assert_eq!(timeline_kinds(&state, &task).len(), 1);
    let sent = digest_days_stored(&state);
    assert_eq!(sent.keys().collect::<Vec<_>>(), [&project_id]);
}

/// Each project's digest stands alone: one failing does not hold back
/// another, and one sent is not sent again while another catches up. A
/// deleted project's day is forgotten.
#[test]
fn each_project_has_its_own_digest_and_a_deleted_one_is_forgotten() {
    let (tmp, state, first, _ws, _task) = linked_workspace();
    let second = {
        let state_root = std::fs::canonicalize(tmp.path()).unwrap();
        let repo = std::fs::canonicalize(init_repo_named(&state_root, "second")).unwrap();
        let mut app = state.lock().unwrap();
        let second = super::project_agent::added_project(&mut app, &repo);
        workspace(&mut app, &second, "also-quiet");
        second
    };
    let day = quiet_day();
    let blocker = block_project_agent(&state, &first);
    AppState::sweep_workspaces(&state, &at_nine(), day + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &second).len(), 1);
    assert_eq!(
        digest_days_stored(&state).keys().collect::<Vec<_>>(),
        [&second]
    );

    std::fs::remove_file(blocker).unwrap();
    AppState::sweep_workspaces(&state, &at_nine(), day + 10 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &first).len(), 1);
    assert_eq!(project_agent_notices(&state, &second).len(), 1);

    let deleted = call(
        &state,
        "project.delete",
        json!({ "project_id": second, "confirm": true }),
    );
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    AppState::sweep_workspaces(&state, &at_nine(), day + DAY_MS + 9 * HOUR_MS);
    assert_eq!(project_agent_notices(&state, &first).len(), 2);
    assert_eq!(
        digest_days_stored(&state).keys().collect::<Vec<_>>(),
        [&first]
    );
}

/// Once its task is Done and its work is pushed, a quiet workspace is
/// reclaimable. Its build output stays unless pruning is switched on.
#[test]
fn a_finished_quiet_workspace_is_reclaimable_and_keeps_its_build_output_by_default() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);

    AppState::sweep_workspaces(&state, &impatient(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["reclaimable"], true, "{verdict:?}");
    assert_eq!(verdict["pruned_bytes"], 0);
    assert!(output.exists(), "pruning is off unless it is switched on");
}

/// With pruning switched on, the build output of a finished quiet workspace
/// goes and its source stays, and the task records it.
#[test]
fn with_pruning_on_a_finished_quiet_workspace_loses_its_build_output() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);

    AppState::sweep_workspaces(&state, &pruning(), now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["reclaimable"], true, "{verdict:?}");
    assert_eq!(verdict["holds"], json!([]));
    assert!(
        verdict["pruned_bytes"].as_u64().unwrap() >= 32 * 1024,
        "{verdict:?}"
    );
    assert!(!output.exists());
    assert!(checkout.join("README.md").exists(), "the source stays");
    assert_eq!(
        std::fs::read_dir(root.join(".build/reclaim"))
            .unwrap()
            .count(),
        0,
        "the trash is emptied"
    );
    assert!(!state.lock().unwrap().workspace_reserved(&ws), "released");
    let kinds: Vec<Value> = timeline_kinds(&state, &task)
        .into_iter()
        .map(|entry| entry["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        json!(["workspace_pruned", "workspace_idle"])
            .as_array()
            .unwrap()
            .clone()
    );
}

/// Reclaim refuses in a sentence while anything holds the workspace, and
/// removes nothing: an open task at once, uncommitted work once Git has been
/// measured off the lock.
#[test]
fn reclaim_refuses_while_a_task_is_open_or_work_is_only_here() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    let (root, checkout) = root_and_checkout(&state, &ws);

    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "conflict");
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: a task linked to it is not Done."
    );

    finish(&state, &task);
    std::fs::write(checkout.join("unsaved.txt"), "mine\n").unwrap();
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: it has uncommitted changes."
    );
    assert!(root.is_dir(), "a refused reclaim removes nothing");
    assert!(
        !state.lock().unwrap().workspace_reserved(&ws),
        "a refusal ends the reservation"
    );
}

/// A reclaim removes the workspace, logs itself on the linked task with who
/// did it, and leaves the task where it was.
#[test]
fn reclaim_removes_the_workspace_and_logs_it_on_the_task() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let source = state
        .lock()
        .unwrap()
        .workspaces
        .get(&ws)
        .unwrap()
        .directories[0]
        .source_path
        .clone();
    let name = crate::isolation::checkout_name(&checkout).unwrap();
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&name)
        .is_ok());

    let reclaimed = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));

    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");
    assert_eq!(reclaimed["result"]["workspace_id"], ws.as_str());
    assert_eq!(reclaimed["result"]["deleted"], true);
    // What became of its branch is #167's (workspace_reclaim_branches.rs).
    assert_eq!(reclaimed["result"]["branches"][0]["outcome"], "deleted");
    assert!(!root.exists());
    assert!(git2::Repository::open(&source)
        .unwrap()
        .find_worktree(&name)
        .is_err());
    let entries = timeline_kinds(&state, &task);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "workspace_reclaimed");
    assert_eq!(entries[0]["actor"], json!({ "kind": "user" }));
    assert_eq!(entries[0]["payload"]["workspace_name"], "quiet");
    let read = call(&state, "tasks.get", json!({ "task_id": task }));
    assert_eq!(
        read["result"]["task"]["state"], "open",
        "reclaim closes nothing"
    );
}

/// The project agent reclaims through its own tool, and the task names it.
#[test]
fn the_project_agent_reclaims_under_its_own_name() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    finish(&state, &task);
    let (owner, agent_id) =
        super::project_agent::project_agent(&mut state.lock().unwrap(), &project_id);

    let reclaimed = state.lock().unwrap().agent_action(
        &owner,
        &agent_id,
        crate::mcp::BridgeAction::ReclaimWorkspace {
            workspace_id: ws.clone(),
        },
    );

    assert!(reclaimed.is_ok(), "{reclaimed:?}");
    let entries = timeline_kinds(&state, &task);
    assert_eq!(entries[0]["actor"]["agent_id"], agent_id.as_str());
}

/// The running service sweeps by the device's settings as they stand at each
/// sweep (#167), not by what it was started with: a threshold of a second,
/// chosen after it started, makes a workspace a second old quiet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_service_sweeps_by_the_device_setting() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    let stop = AppState::spawn_workspace_reclaim(
        state.clone(),
        ReclaimPolicy {
            first_sweep_after: std::time::Duration::from_millis(1500),
            ..ReclaimPolicy::default()
        },
    )
    .await;
    let chosen = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            call(&state, "settings.set", json!({ "workspace_idle_secs": 1 }))
        })
        .await
        .unwrap()
    };
    assert_eq!(chosen["ok"], true, "{chosen:?}");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let verdict = loop {
        let read = {
            let state = state.clone();
            let ws = ws.clone();
            tokio::task::spawn_blocking(move || lifecycle(&state, &ws))
                .await
                .unwrap()
        };
        if read != Value::Null || std::time::Instant::now() > deadline {
            break read;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(verdict["idle"], true, "{verdict:?}");
}

/// A verdict survives a restart, so the bridge does not announce every quiet
/// workspace again on the way back up.
#[test]
fn the_verdict_is_kept_across_a_restart() {
    let (tmp, state, _project, ws, _task) = linked_workspace();
    AppState::sweep_workspaces(&state, &impatient(), now_ms());
    let before = lifecycle(&state, &ws);
    drop(state);

    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let reopened = rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .expect("the store opens");
    let reopened = Arc::new(Mutex::new(reopened));
    assert_eq!(lifecycle(&reopened, &ws), before);
}
