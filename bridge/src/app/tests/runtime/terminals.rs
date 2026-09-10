use super::*;

/// Poll an observable sender's captured pushes until the decrypted history
/// satisfies `pred` (returning everything seen), or panic after 10 s.
pub(in crate::app::tests) async fn wait_for_pushes(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    session_key: &str,
    pred: impl Fn(&[Value]) -> bool,
) -> Vec<Value> {
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(message) = rx.try_recv() {
            seen.push(SessionSender::decrypt_push(session_key, &message));
        }
        if pred(&seen) {
            return seen;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for a matching push; saw: {seen:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait until one push matches `pred`.
pub(in crate::app::tests) async fn wait_for_push(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    session_key: &str,
    pred: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    wait_for_pushes(rx, session_key, |seen| seen.iter().any(&pred)).await
}

/// The concatenated bytes of every `term.output` push for `term_id`.
pub(in crate::app::tests) fn output_text(pushes: &[Value], term_id: &str) -> String {
    let mut bytes = Vec::new();
    for p in pushes {
        if p["type"] == "term.output" && p["term_id"] == term_id {
            bytes.extend_from_slice(&b64decode(p["data"].as_str().unwrap()).unwrap());
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The OS pid behind a tab, addressed the way a client addresses it.
fn tab_pid(state: &Arc<Mutex<AppState>>, wire_id: &str) -> Option<u32> {
    let s = state.lock().unwrap();
    let key = s.tab_key_of_wire_id(wire_id).ok()?;
    agent_pid(&s.tabs[&key])
}

/// True once `pid` is fully gone from the process table (killed AND reaped —
/// a zombie still shows up in `ps` with state Z).
///
/// Waited out rather than asked once: the kill and the reap run on a
/// [`Retirement`]'s own thread, so a verb answers before its harness is
/// gone. What the assertion means is unchanged — the process IS reaped —
/// only when it can first be observed.
pub(in crate::app::tests) fn process_reaped(pid: u32) -> bool {
    settles(|| {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        !out.status.success() || String::from_utf8_lossy(&out.stdout).trim().is_empty()
    })
}

/// Poll `settled` until it answers true, or give up after 10 s. For the
/// facts a background thread makes true shortly after the frame answers.
pub(in crate::app::tests) fn settles(settled: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if settled() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[tokio::test]
async fn keyed_terminal_create_attach_io_close_roundtrip() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    // Create in the primary scope: bash starts in the repo root and the
    // pump runs before any attach.
    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "cols": 80, "rows": 24 }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(created["result"]["term_id"], "term-1");
    assert_eq!(created["result"]["cols"], 80);
    assert_eq!(created["result"]["rows"], 24);

    // Listed under its scope, with metadata.
    let listed = handler.call(
        SessionSender::detached("s1"),
        req("term.list", json!({ "project_id": project_id })),
    );
    let terminals = listed["result"]["terminals"].as_array().unwrap();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["term_id"], "term-1");
    assert!(terminals[0]["created_at"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));

    // Attach with an observable sender, then type a command: the echo comes
    // back as keyed term.output pushes.
    let (sender, mut pushes, key) = SessionSender::observable("s1");
    let attached = handler.call(
        sender,
        req(
            "term.attach",
            json!({ "term_id": "term-1", "cols": 80, "rows": 24 }),
        ),
    );
    assert_eq!(attached["ok"], true, "{attached:?}");
    assert_eq!(attached["result"]["term_id"], "term-1");
    assert!(attached["result"]["snapshot"].is_string());
    assert!(attached["result"]["cursor"].is_u64());

    let input = b64encode(b"echo keyed-term-ok\r");
    let wrote = handler.call(
        SessionSender::detached("s1"),
        req("term.input", json!({ "term_id": "term-1", "data": input })),
    );
    assert_eq!(wrote["ok"], true, "{wrote:?}");
    wait_for_pushes(&mut pushes, &key, |seen| {
        output_text(seen, "term-1").contains("keyed-term-ok")
    })
    .await;

    // Close: the PTY is killed AND reaped, the entry is gone, and every
    // attached client hears term.closed{reason:"closed"}.
    let pid = tab_pid(&state, "term-1").expect("the shell is registered");
    let closed = handler.call(
        SessionSender::detached("s1"),
        req("term.close", json!({ "term_id": "term-1" })),
    );
    assert_eq!(closed["ok"], true, "{closed:?}");
    let seen = wait_for_push(&mut pushes, &key, |p| {
        p["type"] == "term.closed" && p["term_id"] == "term-1" && p["reason"] == "closed"
    })
    .await;
    assert!(!seen.is_empty());
    assert_eq!(state.lock().unwrap().shell_tab_count(), 0);
    assert!(process_reaped(pid), "the shell must be killed and reaped");

    let relisted = handler.call(
        SessionSender::detached("s1"),
        req("term.list", json!({ "project_id": project_id })),
    );
    assert_eq!(relisted["result"]["terminals"].as_array().unwrap().len(), 0);
}

/// `term.ack` is the client's half of flow control: it reports the cursor it
/// has actually applied, on the same id space every other `term.*` verb
/// takes. A stale client acking a terminal that is gone gets the same
/// "unknown term_id" as any other verb, so it drops the tab instead of
/// acking into the void forever.
#[tokio::test]
async fn term_ack_reports_a_cursor_and_rejects_an_unknown_term_id() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    let (sender, _pushes, _key) = SessionSender::observable("s1");
    let attached = handler.call(sender, req("term.attach", json!({ "term_id": "term-1" })));
    assert_eq!(attached["ok"], true, "{attached:?}");
    let cursor = attached["result"]["cursor"].as_u64().unwrap();

    let acked = handler.call(
        SessionSender::detached("s1"),
        req("term.ack", json!({ "term_id": "term-1", "cursor": cursor })),
    );
    assert_eq!(acked["ok"], true, "{acked:?}");
    assert_eq!(acked["result"]["ok"], true, "{acked:?}");

    let unknown = handler.call(
        SessionSender::detached("s1"),
        req("term.ack", json!({ "term_id": "term-404", "cursor": 1 })),
    );
    assert_eq!(unknown["ok"], false, "{unknown:?}");
    assert_eq!(unknown["error"], "unknown term_id", "{unknown:?}");
}

#[tokio::test]
async fn keyed_terminal_snapshot_reflects_input_across_reattach() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    let created = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
    let a = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.attach",
            json!({ "term_id": term_id, "cols": 80, "rows": 24 }),
        ),
    );
    assert_eq!(a["ok"], true);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Send a command (the PTY echoes it and runs it).
    let input = b64encode(b"echo build-terminal-ok\n");
    handler.call(
        SessionSender::detached("s1"),
        req("term.input", json!({ "term_id": term_id, "data": input })),
    );
    tokio::time::sleep(Duration::from_millis(700)).await;

    // Reconnect = a fresh attach. The screen snapshot (vt100 model) must reflect
    // the prior output — that's snapshot-based resync, not byte replay.
    let b = handler.call(
        SessionSender::detached("s2"),
        req(
            "term.attach",
            json!({ "term_id": term_id, "cols": 80, "rows": 24 }),
        ),
    );
    let snap =
        String::from_utf8_lossy(&b64decode(b["result"]["snapshot"].as_str().unwrap()).unwrap())
            .into_owned();
    assert!(
        snap.contains("build-terminal-ok"),
        "reattach snapshot should reflect prior output; got: {snap:?}"
    );
    assert!(b["result"]["cursor"].as_u64().unwrap() > 0);
}

/// A terminal belongs to the WORKTREE it was opened in, not to whichever
/// entity happened to name that worktree when it was created.
///
/// The client addresses an unadopted worktree as `{project_id,
/// worktree_id}` and an adopted one as `{run_id}` — two scope shapes over
/// one directory. Keying the registry by the canonical root is what makes
/// adoption invisible to an open shell; keying it by scope made the shell
/// vanish from the tab row while its process kept running.
#[tokio::test]
async fn term_list_follows_a_worktree_across_adoption() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();
    add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("feature-x"))
        .expect("the external worktree is discoverable")
        .id;

    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
    let shell_pid = tab_pid(&state, &term_id).expect("the shell is registered");

    let adopted = handler.call(
        SessionSender::detached("s1"),
        req(
            "run.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ),
    );
    assert_eq!(adopted["ok"], true, "{adopted:?}");
    let run_id = run_id_of(&adopted);

    let listed = handler.call(
        SessionSender::detached("s1"),
        req("term.list", json!({ "run_id": run_id })),
    );
    let terminals = listed["result"]["terminals"].as_array().unwrap();
    assert_eq!(
        terminals.len(),
        1,
        "the shell survives adoption on the run scope: {listed:?}"
    );
    assert_eq!(terminals[0]["term_id"], json!(term_id));
    // The SAME shell, not a fresh one: adoption is a record change, and the
    // process the human was typing into never noticed it.
    assert_eq!(
        tab_pid(&state, &term_id),
        Some(shell_pid),
        "adoption must not restart the human's shell"
    );
}

/// One directory, two spellings, ONE tab registry.
///
/// The same worktree reaches the daemon under literally different paths: a
/// run's is `worktrees_root/<name>` while the scanner canonicalizes, and on
/// macOS `/tmp` IS `/private/tmp`. Every scope funnels through the canonical
/// form for exactly this reason — keyed by the spelling it was asked with, a
/// shell opened through one path is invisible through the other while its
/// process keeps running, which is the orphan tab this design dissolves.
#[tokio::test]
async fn a_worktree_spelled_two_ways_holds_one_set_of_tabs() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let canonical_root = {
        let mut s = state.lock().unwrap();
        let root = insert_run(
            &mut s,
            &repo,
            dir.path(),
            "run-canonical",
            RunState::Building,
        );
        insert_run(&mut s, &repo, dir.path(), "run-aliased", RunState::Building);
        // The second run addresses that SAME directory under another name.
        let alias = dir.path().join("alias-to-worktree");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        s.runs.get_mut("run-aliased").unwrap().worktree.path = alias;
        root
    };

    let created = call(&handler, "term.create", json!({ "run_id": "run-aliased" }));
    assert_eq!(created["ok"], true, "{created:?}");
    let term_id = created["result"]["term_id"].as_str().unwrap().to_string();

    // Asked about through the OTHER spelling, the same directory holds the
    // same shell.
    let listed = call(&handler, "term.list", json!({ "run_id": "run-canonical" }));
    let listed_ids: Vec<&str> = listed["result"]["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["term_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed_ids,
        vec![term_id.as_str()],
        "both spellings name one worktree, so both see its shell: {listed:?}"
    );

    let s = state.lock().unwrap();
    let keys: Vec<&TabKey> = s.tabs.keys().collect();
    assert!(
        keys.iter().all(|key| key.root == canonical_root),
        "every tab is keyed by the canonical root ({canonical_root:?}): {keys:?}"
    );
}

/// A worktree's tab row is its own. The registry is one map over every
/// worktree the daemon holds, so the only thing keeping one directory's
/// shells out of another's row is the filter on the resolved root.
#[tokio::test]
async fn term_list_shows_only_the_shells_of_the_worktree_asked_about() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    {
        let mut s = state.lock().unwrap();
        insert_run(&mut s, &repo, dir.path(), "run-here", RunState::Building);
        insert_run(&mut s, &repo, dir.path(), "run-there", RunState::Building);
    }
    let shell_in = |run_id: &str| {
        let created = call(&handler, "term.create", json!({ "run_id": run_id }));
        assert_eq!(created["ok"], true, "{created:?}");
        created["result"]["term_id"].as_str().unwrap().to_string()
    };
    let here = shell_in("run-here");
    let there = shell_in("run-there");
    assert_ne!(here, there, "two worktrees, two shells");

    for (run_id, own, other) in [("run-here", &here, &there), ("run-there", &there, &here)] {
        let listed = call(&handler, "term.list", json!({ "run_id": run_id }));
        let ids: Vec<&str> = listed["result"]["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["term_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![own.as_str()],
            "{run_id} must show its own shell and not {other}: {listed:?}"
        );
    }
}

/// One attach verb over one id space. A client holds a row of tabs — some
/// shells, one agent — and must not need to know which RPC each one
/// answers to; the id says everything.
#[tokio::test]
async fn one_attach_verb_serves_shells_and_the_agent() {
    let (dir, repo) = init_repo();
    let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-attach");
    let (agent_wire_id, _) = ensure_agent_tab(
        &state,
        &root,
        "run-attach",
        &crate::agent::derived_agent_id("run-attach"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();

    let attached = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.attach",
            json!({ "term_id": agent_wire_id, "cols": 120, "rows": 40 }),
        ),
    );
    assert_eq!(
        attached["ok"], true,
        "term.attach serves an agent id too: {attached:?}"
    );
    assert_eq!(attached["result"]["term_id"], json!(agent_wire_id));
    assert_eq!(attached["result"]["live"], true);
    assert!(attached["result"]["snapshot"].is_string());
    assert!(attached["result"]["cursor"].is_u64());

    // A well-formed agent id for a worktree with no tab is still "unknown
    // term_id", so a stale client drops the tab instead of hanging on one
    // that swallows every keystroke.
    let stale = handler.call(
        SessionSender::detached("s1"),
        req("term.attach", json!({ "term_id": "agent:nope" })),
    );
    assert_eq!(stale["ok"], false, "{stale:?}");
    assert_eq!(stale["error"], "unknown term_id");
}

/// The agent tab is not one of the human's tabs to close.
///
/// `term.close` serves one id space, so the agent's wire id resolves there
/// like any other — and closing it would kill the one PTY every human→agent
/// path lands in, from a surface that renders it as a `×`-less fixture. The
/// refusal is what makes "always reachable" survive a stale or hand-rolled
/// client; the tab's life belongs to the worktree.
#[tokio::test]
async fn term_close_refuses_the_agent_tab() {
    let (dir, repo) = init_repo();
    let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-unclosable");
    let (agent_wire_id, _) = ensure_agent_tab(
        &state,
        &root,
        "run-unclosable",
        &crate::agent::derived_agent_id("run-unclosable"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();

    let refused = handler.call(
        SessionSender::detached("s1"),
        req("term.close", json!({ "term_id": agent_wire_id })),
    );
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error"], "cannot close an agent terminal");

    let s = state.lock().unwrap();
    let key = derived_agent_key(&AppState::canonical_root(&root), "run-unclosable");
    let tab = s.tabs.get(&key).expect("the agent tab is still registered");
    assert!(tab.live, "and its session was never killed");
    assert!(tab.session_is_live());
}

/// A user terminal is the user's own login shell and nothing else. The
/// daemon owns the argv, so a client naming a kind can never turn a tab
/// into an arbitrary command line.
#[test]
fn a_user_terminal_only_ever_launches_the_login_shell() {
    let shell = shell_harness_spec("/bin/zsh");
    assert_eq!(shell.binary, "/bin/zsh");
    assert_eq!(shell.args, ["-i", "-l"]);
}

/// The `+` menu no longer offers to start an agent, and the daemon refuses
/// to if asked.
///
/// A `claude`/`codex` tab carried the provider's approvals bypass and NO
/// `done` MCP server: an agent in a worktree that Build could not talk to
/// and could not route a report from. A branch may carry as many agents as
/// the human adds, but every one of them arrives through `agent.add` and is
/// therefore Build-owned — which is what this refusal keeps true. An old
/// client asking must fail loudly and be told where an agent comes from —
/// never fall back to a shell, which would silently run a different program
/// than was asked for.
#[test]
fn a_terminal_kind_naming_an_agent_is_refused_and_points_at_agent_add() {
    assert!(require_shell_kind(&json!({})).is_ok());
    assert!(require_shell_kind(&json!({ "kind": "" })).is_ok());
    assert!(require_shell_kind(&json!({ "kind": "shell" })).is_ok());

    for named_agent in AgentProvider::ALL.map(AgentProvider::wire_id) {
        let refused = require_shell_kind(&json!({ "kind": named_agent })).unwrap_err();
        assert!(
            refused.contains("agent.add"),
            "{named_agent}: {refused:?} must name where an agent comes from"
        );
    }
    assert_eq!(
        require_shell_kind(&json!({ "kind": "sh -c curl evil" })).unwrap_err(),
        "unknown terminal kind \"sh -c curl evil\" — a user terminal is always the shell"
    );
}

/// The kind rides the wire both ways: `term.create` echoes it and
/// `term.list` carries it, so a reloaded client labels the tab by what is
/// actually running in it. There is only one answer now — `shell` — and it
/// stays on the wire because the SPA reads it.
#[tokio::test]
async fn term_create_carries_its_kind_onto_the_tab_list() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    let created = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "kind": "shell" }),
        ),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    assert_eq!(created["result"]["kind"], "shell");

    let listed = handler.call(
        SessionSender::detached("s1"),
        req("term.list", json!({ "project_id": project_id })),
    );
    let terminals = listed["result"]["terminals"].as_array().unwrap();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["kind"], "shell");

    // An unknown kind is refused BEFORE anything is spawned.
    let bogus = handler.call(
        SessionSender::detached("s1"),
        req(
            "term.create",
            json!({ "project_id": project_id, "kind": "bash -c evil" }),
        ),
    );
    assert_eq!(bogus["ok"], false, "{bogus:?}");
    assert!(
        bogus["error"]
            .as_str()
            .unwrap()
            .contains("unknown terminal kind"),
        "{bogus:?}"
    );
    assert_eq!(state.lock().unwrap().shell_tab_count(), 1);
}

#[tokio::test]
async fn term_create_enforces_the_daemon_wide_cap() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    for _ in 0..MAX_USER_TERMINALS {
        let created = handler.call(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(created["ok"], true, "{created:?}");
    }
    let over = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    assert_eq!(over["ok"], false);
    assert_eq!(
        over["error"],
        "terminal limit reached (16 open terminals) — close one first"
    );
}

#[tokio::test]
async fn pump_eof_reaps_the_terminal_and_pushes_exited() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().projects[0].id.clone();

    handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    let (sender, mut pushes, key) = SessionSender::observable("s1");
    handler.call(sender, req("term.attach", json!({ "term_id": "term-1" })));
    let pid = tab_pid(&state, "term-1").expect("the shell is registered");

    // The user types `exit`: the shell ends on its own (PTY EOF).
    handler.call(
        SessionSender::detached("s1"),
        req(
            "term.input",
            json!({ "term_id": "term-1", "data": b64encode(b"exit\r") }),
        ),
    );
    wait_for_push(&mut pushes, &key, |p| {
        p["type"] == "term.closed" && p["term_id"] == "term-1" && p["reason"] == "exited"
    })
    .await;
    assert_eq!(state.lock().unwrap().shell_tab_count(), 0);
    assert!(process_reaped(pid), "an exited shell must still be reaped");
}
