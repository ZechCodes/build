//! What can happen to a workspace while the reclaim service or
//! `workspace.reclaim` has it reserved (#135 review): an agent starting, a
//! task reopening, a file changing, a terminal opening, the tracker failing,
//! the daemon stopping, a Git verb or a file write arriving. Each race is run
//! at a point it can hurt: after the reservation, before the second
//! measurement; or after the build output is inspected, before the last look
//! and the move.

use super::tracker_tools::coding_agent;
use super::workspace_reclaim::{
    build_output_in, call, finish, impatient, lifecycle, linked_workspace, now_ms, pruning,
    root_and_checkout, timeline_kinds,
};
use super::*;
use crate::app::workspaces::PrunePhase;
use crate::app::{DeferredNext, PendingAgentTurn, TurnText};
use crate::carrier::SessionSender;
use crate::git_fixture::git_in;
use crate::reclaim::{GitProbe, ReclaimPolicy};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// An agent on the workspace's own conversation, as `(owner, agent_id)`, with
/// nothing queued for it.
fn agent_in(state: &Arc<Mutex<AppState>>, ws: &str) -> (String, String) {
    let conversation = call(
        state,
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    );
    let owner = conversation["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = call(state, "agent.add", json!({ "entity_id": owner }));
    assert_eq!(added["ok"], true, "{added:?}");
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    state.lock().unwrap().delivery_queue.clear_queued();
    (owner, agent_id)
}

/// A turn for `agent` rooted at `root`, the way one in flight is counted.
fn turn_at(root: &Path, (owner, agent_id): &(String, String)) -> PendingAgentTurn {
    PendingAgentTurn {
        operation_id: None,
        root: std::fs::canonicalize(root).unwrap(),
        owner: owner.clone(),
        agent_id: agent_id.clone(),
        conversation_id: format!("conversation-{agent_id}"),
        model_choice: crate::models::ModelChoice::default(),
        choice_revision: 0,
        interrupt: false,
        say: Some(TurnText {
            cold: "cold".into(),
            warm: "warm".into(),
        }),
        phase: "test",
        wants_catch_up: false,
        survives_refusal: false,
    }
}

/// The turns a drain would hand out now, settled again at once.
fn deliverable(state: &Arc<Mutex<AppState>>) -> Vec<String> {
    let mut app = state.lock().unwrap();
    let mut taken = app.take_pending_turns();
    let mut agents = Vec::new();
    while let Some((turn, mark)) = taken.next_turn() {
        agents.push(turn.agent_id.clone());
        mark.settle(&mut app);
    }
    agents
}

fn holds(verdict: &Value) -> Vec<String> {
    verdict["holds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hold| hold.as_str().unwrap().to_string())
        .collect()
}

/// `race`, run only when a prune reaches `phase`.
fn at(phase: PrunePhase, race: impl Fn()) -> impl Fn(PrunePhase) {
    move |reached| {
        if reached == phase {
            race();
        }
    }
}

/// A finished quiet workspace with build output: `(tmp, state, project, ws,
/// task, checkout, output)`.
#[allow(clippy::type_complexity)]
fn ready_to_prune() -> (
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    String,
    String,
    String,
    PathBuf,
    PathBuf,
) {
    let (tmp, state, project_id, ws, task) = linked_workspace();
    finish(&state, &task);
    let (_root, checkout) = root_and_checkout(&state, &ws);
    let output = build_output_in(&checkout);
    (tmp, state, project_id, ws, task, checkout, output)
}

#[test]
fn a_redirected_managed_root_never_launches_a_probe_or_prunes_the_target() {
    let (tmp, state, _project, ws, _task, checkout, _output) = ready_to_prune();
    let (root, _) = root_and_checkout(&state, &ws);
    let source = state
        .lock()
        .unwrap()
        .workspaces
        .get(&ws)
        .unwrap()
        .directories[0]
        .source_path
        .clone();
    let saved = tmp.path().join("original-workspace");
    std::fs::rename(&root, &saved).unwrap();
    let outside = tmp.path().join("unrelated-clone");
    std::fs::create_dir(&outside).unwrap();
    let target_checkout = outside.join(checkout.file_name().unwrap());
    git_in(
        tmp.path(),
        &[
            "clone",
            source.to_str().unwrap(),
            target_checkout.to_str().unwrap(),
        ],
    );
    let target_output = target_checkout.join("node_modules/pkg");
    std::fs::create_dir_all(&target_output).unwrap();
    std::fs::write(target_output.join("keep"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, &root).unwrap();
    let pid = tmp.path().join("probe.pid");
    let policy = ReclaimPolicy {
        git: GitProbe::command(
            "/bin/sh",
            &["-c", &format!("echo $$ > '{}'; exit 1", pid.display())],
        ),
        ..pruning()
    };

    AppState::sweep_workspaces(&state, &policy, now_ms());

    assert!(!pid.exists(), "unsafe root reached Git measurement");
    assert!(target_output.join("keep").exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
}

#[test]
fn a_redirected_checkout_never_launches_a_probe_or_prunes_the_target() {
    let (tmp, state, _project, ws, _task, checkout, _output) = ready_to_prune();
    let outside = tmp.path().join("unrelated-clone");
    std::fs::rename(&checkout, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &checkout).unwrap();
    let pid = tmp.path().join("probe.pid");
    let policy = ReclaimPolicy {
        git: GitProbe::command(
            "/bin/sh",
            &["-c", &format!("echo $$ > '{}'; exit 1", pid.display())],
        ),
        ..pruning()
    };

    AppState::sweep_workspaces(&state, &policy, now_ms());

    assert!(!pid.exists(), "unsafe checkout reached Git measurement");
    assert!(outside.join("node_modules/pkg/index.js").exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
}

#[test]
fn a_root_redirected_after_inspection_keeps_its_build_output() {
    let (tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let (root, _checkout) = root_and_checkout(&state, &ws);
    let outside = tmp.path().join("unrelated-clone");
    let redirected = std::cell::Cell::new(false);

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            std::fs::rename(&root, &outside).unwrap();
            std::os::unix::fs::symlink(&outside, &root).unwrap();
            redirected.set(true);
        }),
    );

    assert!(redirected.get());
    assert!(outside.join(output.strip_prefix(&root).unwrap()).exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
}

/// A message to the workspace's agent after the reservation: its turn waits
/// until the reservation ends, and the workspace is no longer quiet, so
/// nothing is pruned.
#[test]
fn a_message_during_a_prune_waits_for_it_and_keeps_the_build_output() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let agent = agent_in(&state, &ws);
    let posted = std::cell::Cell::new(false);

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            let sent = call(
                &state,
                "thread.post",
                json!({ "entity_id": agent.0, "agent_id": agent.1, "body": "one more thing" }),
            );
            assert_eq!(sent["ok"], true, "{sent:?}");
            assert!(
                deliverable(&state).is_empty(),
                "no turn starts in a reserved workspace"
            );
            posted.set(true);
        }),
    );

    assert!(posted.get(), "the race ran");
    assert!(output.exists(), "{:?}", lifecycle(&state, &ws));
    assert_eq!(lifecycle(&state, &ws)["idle"], false);
    assert_eq!(
        deliverable(&state),
        vec![agent.1],
        "released with the reservation"
    );
}

/// An agent that starts working in one of the workspace's checkouts anyway
/// holds the prune at the last check, and holds `workspace.reclaim` too.
#[test]
fn an_agent_working_in_a_checkout_holds_the_prune_and_the_reclaim() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();
    let agent = agent_in(&state, &ws);

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            let turn = turn_at(&checkout, &agent);
            let _in_flight = state.lock().unwrap().delivery_queue.start(&turn);
        }),
    );

    assert!(output.exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"agent_working".to_string()));
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(
        refused["error"], "Build cannot reclaim quiet yet: an agent is working in it.",
        "{refused:?}"
    );
}

/// The task reopening after the reservation keeps the build output, and the
/// verdict says why.
#[test]
fn reopening_the_task_during_a_prune_keeps_the_build_output() {
    let (_tmp, state, _project, ws, task, _checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            let moved = call(
                &state,
                "tasks.update",
                json!({ "task_id": task, "status": "in_progress" }),
            );
            assert_eq!(moved["ok"], true, "{moved:?}");
        }),
    );

    assert!(output.exists(), "{:?}", lifecycle(&state, &ws));
    assert_eq!(holds(&lifecycle(&state, &ws)), ["task_open"]);
}

/// An edit after the reservation is found by the second measurement.
#[test]
fn an_edit_during_a_prune_keeps_the_build_output() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Reserved, || {
            std::fs::write(checkout.join("README.md"), "changed after the first look\n").unwrap();
        }),
    );

    assert!(output.exists());
    assert_eq!(holds(&lifecycle(&state, &ws)), ["dirty"]);
}

/// An edit after the build output was inspected, when nothing measures the
/// workspace again before the move, is found by the last look.
#[test]
fn an_edit_after_inspection_keeps_the_build_output() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            std::fs::write(checkout.join("README.md"), "changed after inspection\n").unwrap();
        }),
    );

    assert!(output.join("pkg/index.js").exists());
    let verdict = lifecycle(&state, &ws);
    assert_eq!(holds(&verdict), ["dirty"], "{verdict:?}");
    assert_eq!(verdict["dirty_files"], 1);
    assert_eq!(verdict["pruned_bytes"], 0);
}

/// Build output force-added to the index after it was inspected is somebody's
/// source now, and stays.
#[test]
fn a_forced_add_after_inspection_keeps_the_build_output() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            git_in(&checkout, &["add", "-f", "node_modules/pkg/index.js"]);
        }),
    );

    assert!(output.join("pkg/index.js").exists());
    assert_eq!(holds(&lifecycle(&state, &ws)), ["dirty"]);
}

/// Build output committed and pushed after it was inspected leaves the tree
/// clean and everything pushed, so only asking each directory again finds
/// it: it is tracked now, and stays.
#[test]
fn build_output_committed_and_pushed_after_inspection_stays() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            git_in(&checkout, &["add", "-f", "node_modules/pkg/index.js"]);
            git_in(&checkout, &["commit", "-m", "vendor the package"]);
            git_in(&checkout, &["push", "origin", "HEAD"]);
        }),
    );

    assert!(output.join("pkg/index.js").exists());
    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["pruned_bytes"], 0, "{verdict:?}");
    assert!(verdict["pruned_at_ms"].is_null());
}

/// While a workspace is reserved, nothing Build does writes in it: every Git
/// verb that changes a tree or its refs, every file write, Done and every
/// change to its directories answers busy, and nothing lands.
#[test]
fn no_git_verb_or_file_write_starts_during_a_prune() {
    let (_tmp, state, _project, ws, _task, checkout, output) = ready_to_prune();
    let detail = call(&state, "workspace.get", json!({ "workspace_id": ws }));
    let directory = &detail["result"]["directories"][0];
    let source_id = directory["source_id"].as_str().unwrap().to_string();
    let directory_id = directory["id"].as_str().unwrap().to_string();
    let head_before = std::fs::read_to_string(checkout.join("README.md")).unwrap();
    let answers = std::cell::RefCell::new(Vec::new());

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Reserved, || {
            let scoped = |more: Value| {
                let mut params = json!({ "workspace_id": ws, "source_id": source_id });
                params
                    .as_object_mut()
                    .unwrap()
                    .extend(more.as_object().unwrap().clone());
                params
            };
            let read = call(&state, "fs.read", scoped(json!({ "path": "README.md" })));
            assert_eq!(read["ok"], true, "reading is not writing: {read:?}");
            let attempts = [
                ("git.stage", scoped(json!({ "paths": ["README.md"] }))),
                ("git.discard", scoped(json!({ "paths": ["README.md"] }))),
                ("git.commit", scoped(json!({ "message": "sneak" }))),
                ("git.stash", scoped(json!({}))),
                ("git.fetch", scoped(json!({}))),
                (
                    "fs.write",
                    scoped(json!({
                        "path": "README.md",
                        "expected_revision": read["result"]["revision"],
                        "content_b64": "c25lYWsK",
                    })),
                ),
                (
                    "fs.mkdir",
                    json!({ "parent": checkout.display().to_string(), "name": "sneak" }),
                ),
                ("workspace.finish", json!({ "workspace_id": ws })),
                ("workspace.delete", json!({ "workspace_id": ws })),
                (
                    "workspace.rename",
                    json!({ "workspace_id": ws, "name": "sneak" }),
                ),
                (
                    "workspace.remove_directory",
                    json!({ "workspace_id": ws, "directory_id": directory_id }),
                ),
                (
                    "workspace.init_git",
                    json!({ "workspace_id": ws, "source_id": source_id, "target": "workspace" }),
                ),
            ];
            for (method, params) in attempts {
                let answer = call(&state, method, params);
                answers.borrow_mut().push((method, answer));
            }
        }),
    );

    let answers = answers.into_inner();
    assert!(!answers.is_empty(), "the race ran");
    for (method, answer) in answers {
        assert_eq!(answer["ok"], false, "{method}: {answer:?}");
        assert_eq!(answer["error_code"], "busy", "{method}: {answer:?}");
    }
    assert_eq!(
        std::fs::read_to_string(checkout.join("README.md")).unwrap(),
        head_before
    );
    assert!(!checkout.join("sneak").exists());
    assert!(!output.exists(), "the prune itself went ahead");
}

/// A daemon stopping mid-prune stops it: nothing goes, and the workspace is
/// released.
#[test]
fn a_stop_during_a_prune_keeps_the_build_output_and_releases_the_workspace() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Reserved, || {
            AppState::stop_workspace_reclaim(&state);
        }),
    );

    assert!(output.exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
}

/// A Git probe that reads Git the way the service does until `hang` exists,
/// then writes its pid to `pid` and never finishes.
fn probe_hanging_once(hang: &Path, pid: &Path) -> GitProbe {
    let reader = std::env::current_exe().unwrap();
    let script = format!(
        "if [ -e '{}' ]; then echo $$ > '{}'; exec sleep 60; fi; \
         exec '{}' --exact reclaim::tests::git_reading_child --nocapture",
        hang.display(),
        pid.display(),
        reader.display()
    );
    GitProbe::command("/bin/sh", &["-c", &script])
}

/// Stop the service once the reading `pid` names has started.
fn stop_when_reading(state: &Arc<Mutex<AppState>>, pid: &Path) -> std::thread::JoinHandle<()> {
    let (state, pid) = (Arc::clone(state), pid.to_path_buf());
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !pid.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        AppState::stop_workspace_reclaim(&state);
    })
}

/// Whether the process `pid` names is gone: killed, and reaped.
fn gone(pid: &Path) -> bool {
    let pid: i32 = std::fs::read_to_string(pid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) != 0 }
}

/// The daemon stopping while the first measurement reads Git: the reading
/// is killed, the workspace is unmeasured, and nothing is pruned.
#[test]
fn a_stop_while_git_is_read_prunes_nothing() {
    let (tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let (hang, pid) = (tmp.path().join("hang"), tmp.path().join("reading.pid"));
    std::fs::write(&hang, "").unwrap();
    let policy = ReclaimPolicy {
        git: probe_hanging_once(&hang, &pid),
        ..pruning()
    };
    let stopper = stop_when_reading(&state, &pid);

    AppState::sweep_workspaces(&state, &policy, now_ms());

    stopper.join().unwrap();
    assert!(output.join("pkg/index.js").exists());
    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["idle"], false, "{verdict:?}");
    assert!(holds(&verdict).contains(&"unmeasured".to_string()));
    assert!(gone(&pid), "the reading was killed");
}

/// The daemon stopping while the prune reads Git again, under the
/// reservation: the reading is killed, nothing moves, and the workspace is
/// released.
#[test]
fn a_stop_while_the_prune_reads_git_again_prunes_nothing() {
    let (tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let (hang, pid) = (tmp.path().join("hang"), tmp.path().join("reading.pid"));
    let policy = ReclaimPolicy {
        git: probe_hanging_once(&hang, &pid),
        ..pruning()
    };
    let stopper = stop_when_reading(&state, &pid);

    AppState::sweep_workspaces_racing(
        &state,
        &policy,
        now_ms(),
        &at(PrunePhase::Reserved, || std::fs::write(&hang, "").unwrap()),
    );

    stopper.join().unwrap();
    assert!(output.join("pkg/index.js").exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
    assert!(gone(&pid), "the reading was killed");
}

/// The daemon stopping after the build output was inspected: the last look
/// finds it, and nothing moves.
#[test]
fn a_stop_after_inspection_prunes_nothing() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            AppState::stop_workspace_reclaim(&state)
        }),
    );

    assert!(output.join("pkg/index.js").exists());
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
}

/// `workspace.reclaim` reads Git on the service's budget: a reading that
/// outlives it is killed, the reclaim is refused as unmeasured, and the
/// workspace is released and kept.
#[test]
fn a_reclaim_whose_git_reading_runs_out_is_refused() {
    let (tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, _checkout) = root_and_checkout(&state, &ws);
    let (hang, pid) = (tmp.path().join("hang"), tmp.path().join("reading.pid"));
    std::fs::write(&hang, "").unwrap();
    state.lock().unwrap().reclaim_policy = ReclaimPolicy {
        git: probe_hanging_once(&hang, &pid),
        measure_time: std::time::Duration::from_millis(500),
        ..ReclaimPolicy::default()
    };

    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));

    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: Build could not finish measuring it."
    );
    assert!(root.exists());
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
    assert!(gone(&pid), "the reading was killed");
}

/// A stopped service measures nothing more: the last verdict stands.
#[test]
fn a_stopped_service_keeps_the_last_verdict() {
    let (_tmp, state, _project, ws, _task) = linked_workspace();
    AppState::sweep_workspaces(&state, &impatient(), now_ms());
    let before = lifecycle(&state, &ws);

    AppState::stop_workspace_reclaim(&state);
    AppState::sweep_workspaces(&state, &impatient(), now_ms() + 60_000);

    assert_eq!(lifecycle(&state, &ws), before);
}

/// A measurement that runs out of budget is held as unmeasured and is not
/// called idle, so it is neither pruned nor announced.
#[test]
fn a_measurement_over_budget_is_held_and_not_announced() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let starved = ReclaimPolicy {
        measure_entries: 2,
        ..pruning()
    };

    AppState::sweep_workspaces(&state, &starved, now_ms());

    let verdict = lifecycle(&state, &ws);
    assert_eq!(verdict["idle"], false, "{verdict:?}");
    assert!(holds(&verdict).contains(&"unmeasured".to_string()));
    assert!(verdict["noticed_at_ms"].is_null());
    assert!(output.exists());
}

/// Tasks that cannot be read hold every workspace: none of them can be called
/// Done.
#[test]
fn an_unreadable_tracker_holds_the_workspace() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .damage_tracker_tasks();

    AppState::sweep_workspaces(&state, &pruning(), now_ms());

    assert!(output.exists());
    assert_eq!(holds(&lifecycle(&state, &ws)), ["tasks_unread"]);
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: Build could not read the tasks linked to it.",
        "{refused:?}"
    );
}

/// A terminal open anywhere in the workspace holds it, for pruning and for
/// `workspace.reclaim`.
#[tokio::test]
async fn an_open_terminal_holds_the_workspace() {
    let (_tmp, state, _project, ws, _task, _checkout, output) = ready_to_prune();
    let handler = AppState::handler(Arc::clone(&state));
    let opened = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "workspace_id": ws })),
    );
    assert_eq!(opened["ok"], true, "{opened:?}");

    AppState::sweep_workspaces(&state, &pruning(), now_ms());

    assert!(output.exists());
    assert_eq!(holds(&lifecycle(&state, &ws)), ["terminal_open"]);
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(
        refused["error"],
        "Build cannot reclaim quiet yet: a terminal is open in it."
    );
    handler.call(
        SessionSender::detached("s1"),
        req(
            "term.close",
            json!({ "term_id": opened["result"]["term_id"] }),
        ),
    );
}

/// No terminal opens in a workspace while it is reserved.
#[tokio::test]
async fn no_terminal_opens_during_a_prune() {
    let (_tmp, state, _project, ws, _task, _checkout, _output) = ready_to_prune();
    let handler = AppState::handler(Arc::clone(&state));
    let refused = std::cell::RefCell::new(Value::Null);

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Inspected, || {
            *refused.borrow_mut() = handler.call(
                SessionSender::detached("s1"),
                req("term.create", json!({ "workspace_id": ws })),
            );
        }),
    );

    let refused = refused.into_inner();
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"],
        "Build is measuring this workspace. Try again in a moment."
    );
    assert_eq!(refused["error_code"], "busy", "{refused:?}");
}

/// `workspace.reclaim` measures Git with the mutex released, holds the
/// workspace while it does, and only then hands over the removal.
#[test]
fn reclaim_measures_off_the_lock_and_holds_the_workspace_meanwhile() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, _checkout) = root_and_checkout(&state, &ws);
    let agent = agent_in(&state, &ws);
    let params = json!({ "workspace_id": ws });

    let (answered, deferred) = state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answered.is_ok(), "{answered:?}");
    let measuring = deferred.expect("the Git measurement leaves the lock");
    assert!(state.lock().unwrap().workspace_reserved(&ws));
    call(
        &state,
        "thread.post",
        json!({ "entity_id": agent.0, "agent_id": agent.1, "body": "still there?" }),
    );
    assert!(deliverable(&state).is_empty(), "held while measuring");

    let measured = measuring.run();
    let removing =
        match state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, measured)
        {
            DeferredNext::Again(removing) => removing,
            DeferredNext::Answered(answer) => panic!("the removal was not handed on: {answer:?}"),
        };
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
    let removed = removing.run();
    let answered =
        match state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, removed)
        {
            DeferredNext::Answered(answer) => answer,
            DeferredNext::Again(_) => panic!("a third stage"),
        };
    let answered = answered.unwrap();
    assert_eq!(answered["workspace_id"], ws.as_str());
    assert_eq!(answered["deleted"], true);
    assert!(!root.exists());
}

#[test]
fn a_root_redirected_after_explicit_reclaim_decides_is_not_deleted() {
    let (tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let params = json!({ "workspace_id": ws });
    let (answered, deferred) = state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answered.is_ok(), "{answered:?}");
    let measured = deferred.unwrap().run();
    let removing =
        match state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, measured)
        {
            DeferredNext::Again(removing) => removing,
            DeferredNext::Answered(answer) => panic!("reclaim did not reach deletion: {answer:?}"),
        };
    let outside = tmp.path().join("unrelated-clone");
    std::fs::rename(&root, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &root).unwrap();

    let result = removing.run();
    let settled = state
        .lock()
        .unwrap()
        .apply_deferred_stage("workspace.reclaim", &params, result);
    let DeferredNext::Answered(answer) = settled else {
        panic!("a third stage");
    };
    assert!(answer.is_err(), "substituted root was removed");
    assert!(outside.join(checkout.strip_prefix(&root).unwrap()).exists());
}

#[test]
fn a_real_directory_replacing_the_root_after_measurement_is_refused() {
    let (tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let params = json!({ "workspace_id": ws });
    let (answered, deferred) = state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answered.is_ok(), "{answered:?}");
    let measured = deferred.unwrap().run();
    let saved = tmp.path().join("original-workspace");
    std::fs::rename(&root, &saved).unwrap();
    let replacement = root.join(checkout.file_name().unwrap());
    std::fs::create_dir_all(&replacement).unwrap();
    std::fs::write(replacement.join("keep"), b"keep").unwrap();

    let decided =
        state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, measured);

    let DeferredNext::Answered(answer) = decided else {
        panic!("replacement reached removal");
    };
    assert!(answer.is_err());
    assert!(replacement.join("keep").exists());
    assert!(saved.join(checkout.file_name().unwrap()).exists());
}

#[test]
fn a_real_directory_replacing_the_registered_root_before_a_sweep_is_refused() {
    let (tmp, state, _project, ws, task) = linked_workspace();
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
    let saved = tmp.path().join("original-workspace");
    std::fs::rename(&root, &saved).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::copy(
        saved.join(crate::workspace::MANIFEST_FILE),
        root.join(crate::workspace::MANIFEST_FILE),
    )
    .unwrap();
    let replacement = root.join(checkout.file_name().unwrap());
    git_in(
        tmp.path(),
        &[
            "clone",
            source.to_str().unwrap(),
            replacement.to_str().unwrap(),
        ],
    );
    let target_output = build_output_in(&replacement);
    state.lock().unwrap().workspaces.reload().unwrap();

    AppState::sweep_workspaces(&state, &pruning(), now_ms());

    assert!(
        target_output.join("pkg/index.js").exists(),
        "unrelated clone was pruned"
    );
    assert!(holds(&lifecycle(&state, &ws)).contains(&"unmeasured".to_string()));
    let refused = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(
        refused["ok"], false,
        "unrelated clone was accepted for reclaim"
    );
    assert!(target_output.join("pkg/index.js").exists());
}

/// A commit that lands after `workspace.reclaim` measured, by something Build
/// did not start, is found by the last look before the removal.
#[test]
fn a_commit_after_the_reclaim_measured_keeps_the_workspace() {
    let (_tmp, state, _project, ws, task) = linked_workspace();
    finish(&state, &task);
    let (root, checkout) = root_and_checkout(&state, &ws);
    let params = json!({ "workspace_id": ws });
    let (answered, deferred) = state
        .lock()
        .unwrap()
        .dispatch_deferring("workspace.reclaim", &params);
    assert!(answered.is_ok(), "{answered:?}");
    let measured = deferred.expect("the Git measurement leaves the lock").run();

    std::fs::write(checkout.join("late.txt"), "late\n").unwrap();
    git_in(&checkout, &["add", "late.txt"]);
    git_in(&checkout, &["commit", "-m", "late work"]);

    let decided =
        state
            .lock()
            .unwrap()
            .apply_deferred_stage("workspace.reclaim", &params, measured);
    let DeferredNext::Answered(refused) = decided else {
        panic!("the removal was handed on after a late commit");
    };
    assert_eq!(
        refused.unwrap_err(),
        "Build cannot reclaim quiet yet: it has commits no remote has."
    );
    assert!(root.exists());
    assert!(!state.lock().unwrap().workspace_reserved(&ws));
}

/// Reclaiming writes the task's timeline and wakes nobody watching it.
#[test]
fn a_reclaim_wakes_nobody_watching_the_task() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    let watcher = coding_agent(&mut state.lock().unwrap(), &project_id, "watcher");
    let tracked = set_task_tracking(&mut state.lock().unwrap(), &task, &watcher.1, true);
    assert_eq!(tracked["ok"], true, "{tracked:?}");
    finish(&state, &task);
    {
        let mut app = state.lock().unwrap();
        app.delivery_queue.lapse_settle_windows();
        app.delivery_queue.take_ready(|_| false);
    }

    let reclaimed = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");

    let woken = {
        let mut app = state.lock().unwrap();
        app.delivery_queue.lapse_settle_windows();
        app.delivery_queue.take_ready(|_| false)
    };
    assert!(
        woken.iter().all(|turn| turn.agent_id != watcher.1),
        "the watcher is not woken for a reclaim"
    );
    let entries = timeline_kinds(&state, &task);
    assert_eq!(entries[0]["kind"], "workspace_reclaimed");
}

/// A size walk the Workspaces tab asked for, landing while a sweep prunes the
/// workspace (#273): the sweep's older reading does not replace it.
#[test]
fn a_size_walk_landing_mid_sweep_keeps_its_newer_size() {
    let (_tmp, state, _project, ws, _task, _checkout, _output) = ready_to_prune();
    let now = now_ms();
    let walked = std::cell::RefCell::new(Value::Null);

    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now,
        &at(PrunePhase::Inspected, || {
            let asked = call(
                &state,
                "workspace.measure_sizes",
                json!({ "workspace_ids": [ws] }),
            );
            assert_eq!(asked["result"]["queued"], json!([ws]), "{asked:?}");
            assert!(AppState::measure_next_workspace_size(&state, now + 1));
            *walked.borrow_mut() = lifecycle(&state, &ws);
        }),
    );

    let walked = walked.into_inner();
    assert_eq!(walked["size_measured_at_ms"], now + 1, "{walked:?}");
    let swept = lifecycle(&state, &ws);
    assert_eq!(swept["pruned_at_ms"], now, "the sweep pruned: {swept:?}");
    assert_eq!(swept["size_measured_at_ms"], now + 1, "{swept:?}");
    assert_eq!(swept["size_bytes"], walked["size_bytes"]);
}
#[test]
fn fs_upload_verbs_and_directory_creation_refuse_a_reserved_workspace() {
    let (_tmp, state, _project, ws, _task, checkout, _output) = ready_to_prune();
    let detail = call(&state, "workspace.get", json!({"workspace_id": ws}));
    let source_id = detail["result"]["directories"][0]["source_id"]
        .as_str()
        .unwrap();
    let sender = SessionSender::detached("upload-user");
    let scoped_call = |method, params| {
        crate::api::v1::changes::with_session(&sender, || call(&state, method, params))
    };
    let begun = scoped_call(
        "fs.uploadBegin",
        json!({
            "workspace_id": ws, "source_id": source_id, "parent": "node_modules", "name": "new.bin", "size": 1,
        }),
    );
    assert_eq!(begun["ok"], true, "{begun:?}");
    let id = &begun["result"]["upload_id"];
    AppState::sweep_workspaces_racing(
        &state,
        &pruning(),
        now_ms(),
        &at(PrunePhase::Reserved, || {
            let attempts = [
                (
                    "fs.createDirectory",
                    json!({"workspace_id": ws, "source_id": source_id, "parent": "", "name": "new-dir"}),
                ),
                (
                    "fs.uploadBegin",
                    json!({"workspace_id": ws, "source_id": source_id, "parent": "", "name": "another.bin", "size": 0}),
                ),
                (
                    "fs.uploadChunk",
                    json!({"upload_id": id, "offset": 0, "content_b64": "AA=="}),
                ),
                ("fs.uploadFinish", json!({"upload_id": id})),
                ("fs.uploadAbort", json!({"upload_id": id})),
            ];
            for (method, params) in attempts {
                let response = scoped_call(method, params);
                assert_eq!(response["error_code"], "busy", "{method}: {response:?}");
            }
        }),
    );
    assert!(!checkout.join("new-dir").exists());
    assert!(!checkout.join("another.bin").exists());
}
