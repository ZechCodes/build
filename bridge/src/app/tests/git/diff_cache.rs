use super::*;

// ==== stale-while-revalidate for the poll diff caches ======================
//
// The wedge (2026-08-13): `board.list` recomputed a run's worktree diff
// inline, under the app mutex, whenever its 10s cache had aged out. On a
// churning 56k-file worktree that took seconds, the relay read loop stopped
// draining behind it, and the relay called the device dead. These tests hold
// the caches to the rule that follows from it: a poll is answered from the
// last value the moment there is one, the recompute happens behind the
// answer, several stale polls share one recompute, and no diff ever runs
// while the app mutex is held.

/// One run's `stat` off a `board.list` poll, over the frame handler — the
/// path the relay actually takes, warm step and all.
fn polled_run_stat(handler: &FrameHandler, run_id: &str) -> Value {
    let board = call(handler, "board.list", json!({}));
    board["result"]["runs"]
        .as_array()
        .unwrap_or_else(|| panic!("the board ships runs: {board:?}"))
        .iter()
        .find(|run| run["run_id"] == json!(run_id))
        .unwrap_or_else(|| panic!("{run_id} is on the board: {board:?}"))["stat"]
        .clone()
}

/// `board.list` on a blocking thread, as the relay's dispatcher runs it.
async fn poll_board(handler: &FrameHandler) {
    let handler = handler.clone();
    tokio::task::spawn_blocking(move || call(&handler, "board.list", json!({})))
        .await
        .expect("the poll does not panic");
}

/// Poll until the refresh a first poll claimed has published its diffstat.
/// No read computes any more, so this is what "the cache is seeded" means.
async fn seeded_run_stat(handler: &FrameHandler, run_id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let handler = handler.clone();
            let run_id = run_id.to_string();
            let stat = tokio::task::spawn_blocking(move || polled_run_stat(&handler, &run_id))
                .await
                .unwrap();
            if !stat.is_null() {
                return stat;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the refresh the first poll claimed publishes a diffstat")
}

/// A shared QA daemon with one adopted run whose worktree holds one
/// uncommitted file — the shape a poll pays a diff for.
fn daemon_with_a_run_to_diff(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (Arc<Mutex<AppState>>, FrameHandler, String, PathBuf) {
    let (state, handler) = shared_qa_state_and_handler(repo, dir);
    let run_id = {
        let mut app = state.lock().unwrap();
        adopted_run(&mut app, repo, dir, "swr-run")
    };
    let worktree = state.lock().unwrap().runs[&run_id].worktree.path.clone();
    std::fs::write(worktree.join("first.txt"), "one\n").unwrap();
    (state, handler, run_id, worktree)
}

/// Make every run-stat compute take `hold`, and count them.
fn watch_run_stat_computes(
    state: &Arc<Mutex<AppState>>,
    hold: Duration,
) -> Arc<std::sync::atomic::AtomicUsize> {
    let computes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&computes);
    state.lock().unwrap().diff_compute_observer = Some(Arc::new(move |key| {
        if matches!(key, DiffCacheKey::RunStat(_)) {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(hold);
        }
    }));
    computes
}

/// A poll that finds an aged-out diffstat is answered from the value it
/// already has, immediately — and the recompute that runs behind it
/// publishes what it found.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_diffstat_answers_the_poll_while_it_refreshes_behind_it() {
    let (dir, repo) = init_repo();
    let (state, handler, run_id, worktree) = daemon_with_a_run_to_diff(&repo, dir.path());

    // The first poll has nothing to serve and answers null; the refresh it
    // claimed is what fills the cache.
    let first = seeded_run_stat(&handler, &run_id).await;
    let before = first["uncommitted"]["files_changed"]
        .as_u64()
        .unwrap_or_else(|| panic!("the first poll counted the tree: {first:?}"));
    assert!(
        before >= 1,
        "the worktree carries the file just written: {first:?}"
    );

    // One more file, and every cache aged past its TTL: the next poll is a
    // stale one, and its recompute is held open long enough to be caught in
    // the act.
    std::fs::write(worktree.join("second.txt"), "two\n").unwrap();
    watch_run_stat_computes(&state, Duration::from_millis(600));
    state.lock().unwrap().force_stale_diff_caches = true;

    let started = std::time::Instant::now();
    let stale = {
        let handler = handler.clone();
        let run_id = run_id.clone();
        tokio::task::spawn_blocking(move || polled_run_stat(&handler, &run_id))
            .await
            .unwrap()
    };
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(300),
        "the poll waited for the recompute instead of answering from cache: {waited:?}"
    );
    assert_eq!(
        stale["uncommitted"]["files_changed"],
        json!(before),
        "the poll was answered with the value it already had: {stale:?}"
    );

    // …and what the refresh computed replaces it.
    let refreshed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let handler = handler.clone();
            let run_id = run_id.clone();
            let stat = tokio::task::spawn_blocking(move || polled_run_stat(&handler, &run_id))
                .await
                .unwrap();
            if stat["uncommitted"]["files_changed"] == json!(before + 1) {
                return stat;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the background refresh publishes the second file");
    assert_eq!(
        refreshed["uncommitted"]["files_changed"],
        json!(before + 1),
        "the refresh counted the file written while the stale value was serving: {refreshed:?}"
    );
}

/// Single-flight: a recompute already running absorbs every poll that
/// arrives while it runs. Otherwise four browser tabs polling twice a second
/// put a worktree scan per poll on the disk — the flood that starved the
/// socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_stale_polls_share_one_recompute() {
    let (dir, repo) = init_repo();
    let (state, handler, run_id, _worktree) = daemon_with_a_run_to_diff(&repo, dir.path());

    // Seed the cache: after this every poll is a stale read.
    seeded_run_stat(&handler, &run_id).await;
    let computes = watch_run_stat_computes(&state, Duration::from_millis(800));
    state.lock().unwrap().force_stale_diff_caches = true;

    let mut polls = Vec::new();
    for _ in 0..6 {
        let handler = handler.clone();
        polls.push(tokio::task::spawn_blocking(move || {
            call(&handler, "board.list", json!({}));
        }));
    }
    for poll in polls {
        tokio::time::timeout(Duration::from_secs(5), poll)
            .await
            .expect("no poll waits on the recompute")
            .expect("no poll panics");
    }

    assert_eq!(
        computes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "six stale polls must share one recompute"
    );
}

/// A mutation that lands while a refresh is computing takes the refresh's
/// claim with it, and a refresh without a claim publishes nothing. Otherwise
/// a commit would be followed by the pre-commit numbers reappearing on the
/// board, put there by the scan the commit interrupted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refresh_overtaken_by_a_mutation_publishes_nothing() {
    let (dir, repo) = init_repo();
    let (state, handler, run_id, _worktree) = daemon_with_a_run_to_diff(&repo, dir.path());

    // Seed the cache, then start a refresh and hold it open.
    seeded_run_stat(&handler, &run_id).await;
    watch_run_stat_computes(&state, Duration::from_millis(600));
    state.lock().unwrap().force_stale_diff_caches = true;
    poll_board(&handler).await;

    // The mutation lands while that refresh is still walking the worktree.
    tokio::time::sleep(Duration::from_millis(100)).await;
    state.lock().unwrap().invalidate_run_stat(&run_id);

    // Whatever the refresh found describes the tree from before it.
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(
        !state.lock().unwrap().has_cached_run_stat(&run_id),
        "the overtaken refresh put its numbers back on the board"
    );
}

/// The rule the incident was a violation of: the git work never runs with
/// the app mutex in hand — not the background refresh, and not the
/// first-ever compute a caller waits for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_diff_is_computed_while_the_app_mutex_is_held() {
    let (dir, repo) = init_repo();
    let (state, handler, _run_id, _worktree) = daemon_with_a_run_to_diff(&repo, dir.path());

    // Each compute reports whether the app mutex could be taken while it ran.
    // A compute holding it (or running under a caller that holds it) never
    // sees it free.
    type Observation = (DiffCacheKey, bool);
    let observations: Arc<Mutex<Vec<Observation>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&observations);
    let weak = Arc::downgrade(&state);
    state.lock().unwrap().diff_compute_observer = Some(Arc::new(move |key| {
        let mut free = false;
        for _ in 0..20 {
            if weak.upgrade().is_some_and(|state| state.try_lock().is_ok()) {
                free = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        recorded.lock().unwrap().push((key.clone(), free));
    }));

    // The refreshes a first poll claims …
    poll_board(&handler).await;
    // … then background refreshes of what is now stale.
    state.lock().unwrap().force_stale_diff_caches = true;
    poll_board(&handler).await;

    tokio::time::timeout(Duration::from_secs(20), async {
        while observations.lock().unwrap().len() < 4 {
            poll_board(&handler).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the caches refresh");

    let seen = observations.lock().unwrap().clone();
    assert!(
        seen.iter().all(|(_, free)| *free),
        "a diff ran while the app mutex was held: {seen:?}"
    );
}

// ==== a board read never computes ========================================
//
// The rule step 3 of the concurrency spec adds to stale-while-revalidate: a
// read with NOTHING to serve answers anyway. It says what it does not know
// yet, claims the scan, and the scan invalidates the browser when it lands.
// Nothing waits under the app mutex for a first value ever again.

/// Hold every checkout scan open at the point its git work starts, so a
/// test can look at the daemon while one is running.
fn gate_scan_computes(state: &Arc<Mutex<AppState>>) -> OffLockGateHandle {
    gate_diff_computes(state, |key| matches!(key, DiffCacheKey::ExternalScan(_)))
}

/// Hold every diff compute whose key `wanted` picks open until the test
/// lets it go, so a read has to answer from what it has.
fn gate_diff_computes(
    state: &Arc<Mutex<AppState>>,
    wanted: impl Fn(&DiffCacheKey) -> bool + Send + Sync + 'static,
) -> OffLockGateHandle {
    let (gate, handle) = OffLockGate::new();
    state.lock().unwrap().diff_compute_observer = Some(Arc::new(move |key| {
        if wanted(key) {
            gate.arrive();
        }
    }));
    handle
}

/// Poll until the board has stopped saying it is scanning.
async fn settled_board(handler: &FrameHandler) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let handler = handler.clone();
            let board =
                tokio::task::spawn_blocking(move || call(&handler, "board.list", json!({})))
                    .await
                    .unwrap();
            if board["result"]["scanning"] == json!(false) {
                return board;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the scan the board claimed lands")
}

/// A board with nothing cached answers at once and says so. The old
/// behaviour — fall through to `discover_external_worktrees` under the app
/// mutex because there is no number to serve — is what made the first poll
/// after a restart the slowest frame of the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_list_answers_scanning_when_nothing_has_ever_been_computed() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let gate = gate_scan_computes(&state);

    let started = std::time::Instant::now();
    let board = {
        let handler = handler.clone();
        tokio::task::spawn_blocking(move || call(&handler, "board.list", json!({})))
            .await
            .unwrap()
    };
    let waited = started.elapsed();

    assert!(
        waited < Duration::from_millis(100),
        "the board waited for a scan it had claimed: {waited:?}"
    );
    assert_eq!(
        board["result"]["external_worktrees"],
        json!([]),
        "{board:?}"
    );
    assert_eq!(
        board["result"]["scanning"],
        json!(true),
        "an empty rail with no scan behind it is a board still looking: {board:?}"
    );

    gate.wait_for_arrival();
    gate.release();
    let settled = settled_board(&handler).await;
    let listed = settled["result"]["external_worktrees"]
        .as_array()
        .expect("the rail ships checkouts");
    assert!(
        listed.iter().any(|w| w["branch"] == json!("feature-loose")),
        "the scan that landed put the checkout on the board: {settled:?}"
    );
}

/// And the scan it claimed runs with the mutex free: every other frame is
/// served while the very first scan of a repository is still walking it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_scan_never_runs_under_the_app_mutex() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let gate = gate_scan_computes(&state);

    poll_board(&handler).await;
    gate.wait_for_arrival();

    // The scan is inside its git work. Every frame behind it still answers.
    let started = std::time::Instant::now();
    for _ in 0..3 {
        let board = {
            let handler = handler.clone();
            tokio::task::spawn_blocking(move || call(&handler, "board.list", json!({})))
                .await
                .unwrap()
        };
        assert_eq!(board["ok"], true, "{board:?}");
        assert_eq!(
            board["result"]["scanning"],
            json!(true),
            "the scan is still running, and no second one was started: {board:?}"
        );
    }
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(300),
        "the frames behind the scan queued on the app mutex: {waited:?}"
    );

    gate.release();
    settled_board(&handler).await;
}

/// The board said "scanning" and answered. What tells the browser to ask
/// again is the scan landing — the same push invalidation every other
/// change travels on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_landed_first_scan_invalidates_the_browser() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let gate = gate_scan_computes(&state);

    poll_board(&handler).await;
    gate.wait_for_arrival();
    // Everything the board read itself may have queued, out of the way.
    settled_pushes(&mut rx, &key).await;

    gate.release();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let events = change_events(&settled_pushes(&mut rx, &key).await);
            if events.iter().any(|event| event["type"] == "board.changed") {
                return;
            }
        }
    })
    .await
    .expect("the scan that landed told the browser to ask again");
}

/// The board said `stat: null` and answered. What tells the browser to ask
/// again is that first diffstat landing — the same push invalidation a
/// first scan travels on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_landed_first_diffstat_invalidates_the_browser() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let run_id = {
        let mut app = state.lock().unwrap();
        adopted_run(&mut app, &repo, dir.path(), "stat-run")
    };
    // Every other first landing out of the way, so the only cache this
    // board read is missing is the run's diffstat.
    seeded_run_stat(&handler, &run_id).await;
    settled_board(&handler).await;
    state.lock().unwrap().invalidate_run_stat(&run_id);
    settled_pushes(&mut rx, &key).await;

    poll_board(&handler).await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let events = change_events(&settled_pushes(&mut rx, &key).await);
            if events.iter().any(|event| event["type"] == "board.changed") {
                return;
            }
        }
    })
    .await
    .expect("the diffstat that landed told the browser to ask again");
}

/// An id for a checkout made outside Build since the last scan is refused,
/// and the refusal starts the one scan that will resolve it — rather than
/// paying for that scan under the app mutex the way the old forced retry
/// did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_out_of_band_worktree_id_is_refused_and_claims_one_scan() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .unwrap();

    let path = add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let canonical = std::fs::canonicalize(&path).unwrap();
    let worktree_id = crate::worktree::external_worktree_id(&canonical);
    let gate = gate_scan_computes(&state);

    let refused = state
        .lock()
        .unwrap()
        .resolve_external_worktree(&project_id, &worktree_id)
        .expect_err("the cache cannot know about a worktree made behind Build's back");
    assert!(
        refused.contains(&worktree_id) && refused.contains("scan now running"),
        "the refusal names the id and what will resolve it: {refused}"
    );
    assert!(
        state
            .lock()
            .unwrap()
            .diff_refresh_is_running(&DiffCacheKey::ExternalScan(project_id.clone())),
        "the refusal claimed no scan, so the id would never resolve"
    );

    gate.wait_for_arrival();
    gate.release();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let resolved = state
                .lock()
                .unwrap()
                .resolve_external_worktree(&project_id, &worktree_id);
            if resolved.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the scan the refusal claimed resolves the id");
}

/// A daemon that has not scanned a project yet does not know which of its
/// checkouts are gone — and a bare checkout's attention record lives
/// nowhere but the attention map. The first stamp after a restart must not
/// prune that map against a scan nobody has run.
#[test]
fn attention_survives_a_stamp_taken_before_the_first_scan() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(&repo, dir.path(), "kept", "feature-kept");
    state.scan_external_worktrees_now(&project_id).unwrap();
    let worktree_id = crate::worktree::external_worktree_id(&std::fs::canonicalize(&path).unwrap());

    let seen = state.handle(req("entity.seen", json!({ "entity_id": worktree_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");
    assert!(state.has_attention(&worktree_id));

    // A restart: the map comes back from the store, the scan has not run.
    state.clear_external_scan_for_test(&project_id);
    state.persist_attention();

    let reloaded = Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_attention();
    assert!(
        reloaded.contains_key(&worktree_id),
        "the checkout's attention was pruned against a scan nobody had run: {:?}",
        reloaded.keys().collect::<Vec<_>>()
    );
}

/// Delete a checkout the way a user does: with nothing of Build's own still
/// writing into it. A queued turn scaffolds `.build/` into the checkout on a
/// thread of its own, after the verb that queued it has answered — and a
/// directory being written into is neither one `remove_dir_all` can walk nor
/// one that stays deleted once it has been.
async fn delete_the_checkout(state: &Arc<Mutex<AppState>>, checkout: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            {
                let app = state.lock().unwrap();
                if app.delivery_queue.queued_is_empty() && app.delivery_queue.is_idle() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("every turn the run's verbs queued arrives");
    std::fs::remove_dir_all(checkout).expect("the user deleted their checkout");
}

/// The board sweeps runs whose checkout vanished, and deciding whether each
/// stage's commits were ever published is a fetch and two graph walks per
/// stage. The read answers with the run it still has and the sweep archives
/// it behind them, with the state lock free the whole time it asks git.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vanished_runs_stages_are_judged_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let (_, run_id) = planned_run_in_review_delivered(&state, &handler, "a run that vanishes");
    let worktree = state.lock().unwrap().runs[&run_id].worktree.path.clone();
    delete_the_checkout(&state, &worktree).await;

    let (gate, held) = OffLockGate::new();
    state.lock().unwrap().off_lock_gate = Some(gate);

    let started = std::time::Instant::now();
    let board = {
        let handler = handler.clone();
        tokio::task::spawn_blocking(move || call(&handler, "board.list", json!({})))
            .await
            .unwrap()
    };
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_secs(2),
        "the board waited for the sweep instead of answering from what it had: {waited:?}"
    );
    assert!(
        row_with(&board["result"]["runs"], "run_id", &run_id)["run_id"] == json!(run_id),
        "the run it still has is the run it answers with: {board:?}"
    );

    held.wait_for_arrival();
    // The sweep is inside its git right now, and the daemon is not.
    poll_board(&handler).await;
    held.release();

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if state.lock().unwrap().runs[&run_id].run.state == RunState::Archived {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the sweep archives the run whose checkout is gone");
}

/// A job for the off-lock primitive that reports which half ran. Its claim
/// is the channel, so `apply` and `abandon` — which see no job — can still
/// say what became of it.
struct ProbeJob {
    outcomes: std::sync::mpsc::Sender<&'static str>,
    decide_panics: bool,
}

impl OffLockJob for ProbeJob {
    type Claim = std::sync::mpsc::Sender<&'static str>;
    type Decided = &'static str;

    fn claim(&mut self) -> Self::Claim {
        self.outcomes.clone()
    }

    fn decide(self) -> &'static str {
        assert!(!self.decide_panics, "this probe's decide phase panics");
        "decided"
    }

    fn apply(_state: &mut AppState, claim: Self::Claim, decided: &'static str) {
        claim.send(decided).unwrap();
    }

    fn abandon(_state: &mut AppState, claim: Self::Claim) {
        claim.send("abandoned").unwrap();
    }
}

/// With no runtime and no shared handle — the synchronous tests — the
/// primitive decides inline and applies, so a read that claimed a job is
/// answered from what it found.
#[test]
fn an_off_lock_job_with_no_runtime_decides_inline_and_applies() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (outcomes, seen) = std::sync::mpsc::channel();

    state.run_off_lock(ProbeJob {
        outcomes,
        decide_panics: false,
    });

    assert_eq!(seen.try_recv(), Ok("decided"));
}

/// A decide phase that panics on the blocking pool gives its claim back
/// through `abandon`, under the lock, or the claim it held would never be
/// taken again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_off_lock_job_whose_decide_panics_gives_its_claim_back() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let (outcomes, seen) = std::sync::mpsc::channel();

    state.lock().unwrap().run_off_lock(ProbeJob {
        outcomes,
        decide_panics: true,
    });

    let outcome = tokio::task::spawn_blocking(move || seen.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(outcome, Ok("abandoned"));
}

/// The same job under a runtime runs its decide phase on the blocking
/// pool and applies under the lock afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_off_lock_job_under_a_runtime_applies_what_it_decided() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let (outcomes, seen) = std::sync::mpsc::channel();

    state.lock().unwrap().run_off_lock(ProbeJob {
        outcomes,
        decide_panics: false,
    });

    let outcome = tokio::task::spawn_blocking(move || seen.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(outcome, Ok("decided"));
}

/// "Nothing has looked yet" is not the answer "there is no such checkout",
/// and every read that can miss says which one it means in the same words.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missed_checkout_says_whether_a_scan_has_ever_landed() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    // A second project, whose own checkouts have been scanned. Whether the
    // scan can still show a checkout is a fact about one project, never
    // about the board as a whole.
    let other_repo = init_repo_named(dir.path(), "other");
    let other_id = {
        let mut app = state.lock().unwrap();
        let added = app
            .dispatch(
                "project.add",
                &json!({ "path": other_repo.to_string_lossy() }),
            )
            .expect("the second project registers");
        let id = added["project_id"].as_str().unwrap().to_string();
        app.scan_external_worktrees_now(&id)
            .expect("its checkouts are scanned");
        id
    };
    // The checkout scan is held open, so every read below is answered by a
    // project nothing has scanned.
    let gate = gate_scan_computes(&state);

    let refusals: Vec<String> = {
        let mut app = state.lock().unwrap();
        vec![
            app.resolve_external_worktree(&project_id, "wt-000000000000")
                .expect_err("no scan has landed to resolve an id against"),
            app.dispatch(
                "entity.dismiss",
                &json!({ "project_id": project_id, "branch": "feature-loose" }),
            )
            .expect_err("no scan has landed to find the row in"),
        ]
    };
    for refusal in &refusals {
        assert!(
            refusal.contains("no scan of this project's checkouts has landed"),
            "the refusal blamed the checkout for a scan nobody has run: {refusal}"
        );
    }

    let missed = state
        .lock()
        .unwrap()
        .dispatch(
            "branch.get",
            &json!({ "project_id": other_id, "branch": "nothing-is-on-this" }),
        )
        .expect_err("no checkout of the scanned project is on that branch");
    assert!(
        missed.contains("made outside Build since the last scan"),
        "a scanned project's miss was answered out of an unscanned neighbour's scan: {missed}"
    );

    gate.wait_for_arrival();
    gate.release();
}

/// A project whose scan has never landed has nothing to amend. The create
/// says so and stops: it neither cancels the first scan that is running to
/// find its checkout anyway, nor tells the browser about an edit it did not
/// make.
#[test]
fn a_create_before_the_first_scan_leaves_the_running_scan_alone() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(&repo, dir.path(), "fresh", "feature-fresh");
    let described = crate::worktree::describe_checkout(&path, "main", crate::worktree::unix_now())
        .expect("it is a checkout");
    let scan = DiffCacheKey::ExternalScan(project_id.clone());
    let _claim = state.claim_diff_refresh_for_test(scan.clone());
    state.changes.flush();

    state.note_worktree_appeared(&project_id, described);

    assert!(
        state.diff_refresh_is_running(&scan),
        "the create dropped the first scan its checkout would have arrived on"
    );
    assert!(
        !state.changes.has_pending(),
        "the create pushed an invalidation for an edit it did not make"
    );
}

/// A checkout bound to a run is excluded from the scan, so removing it
/// from the list is routinely a no-op — `run.finish`'s failure branch and
/// the finish epilogue both reach here with a path the list never held.
/// A removal that removed nothing neither overtakes the scan in flight
/// (whose whole fresh list would be dropped on landing) nor tells every
/// browser to refetch a board that did not change.
#[test]
fn a_removal_of_a_checkout_the_scan_never_had_leaves_the_running_scan_alone() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "kept", "feature-kept");
    state.scan_external_worktrees_now(&project_id).unwrap();
    let scan = DiffCacheKey::ExternalScan(project_id.clone());
    let _claim = state.claim_diff_refresh_for_test(scan.clone());
    state.changes.flush();

    state.note_worktree_gone(&project_id, &dir.path().join("never-in-the-list"));

    assert!(
        !state.diff_refresh_is_superseded(&scan),
        "a removal that removed nothing superseded the running scan"
    );
    assert!(
        !state.changes.has_pending(),
        "a removal that removed nothing pushed an invalidation"
    );
    assert_eq!(
        state.external_scan_of(&project_id).unwrap().worktrees.len(),
        1,
        "the removal touched a checkout it was not asked about"
    );
}

/// The mirror for an appearance: describing the checkout the list already
/// holds, unchanged, is not an edit either.
#[test]
fn re_noting_an_unchanged_checkout_leaves_the_running_scan_alone() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "kept", "feature-kept");
    let known = state.scan_external_worktrees_now(&project_id).unwrap()[0].clone();
    let scan = DiffCacheKey::ExternalScan(project_id.clone());
    let _claim = state.claim_diff_refresh_for_test(scan.clone());
    state.changes.flush();

    state.note_worktree_appeared(&project_id, known);

    assert!(
        !state.diff_refresh_is_superseded(&scan),
        "re-noting an unchanged checkout superseded the running scan"
    );
    assert!(
        !state.changes.has_pending(),
        "re-noting an unchanged checkout pushed an invalidation"
    );
}

/// The scan is the liveness test for CHECKOUTS and for nothing else. A
/// project nobody has scanned spares their records; every other kind of key
/// still answers to the map that owns it.
#[test]
fn attention_for_a_dead_run_is_pruned_before_the_first_scan() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let path = add_external_worktree(&repo, dir.path(), "kept", "feature-kept");
    state.scan_external_worktrees_now(&project_id).unwrap();
    let worktree_id = crate::worktree::external_worktree_id(&std::fs::canonicalize(&path).unwrap());
    let dead_run = "run-nobody-has".to_string();
    for entity_id in [&worktree_id, &dead_run] {
        let seen = state.handle(req("entity.seen", json!({ "entity_id": entity_id })));
        assert_eq!(seen["ok"], true, "{seen:?}");
    }

    state.clear_external_scan_for_test(&project_id);
    state.persist_attention();

    let reloaded = Store::new(dir.path().join("store"))
        .expect("store opens")
        .load_attention();
    assert!(
        reloaded.contains_key(&worktree_id),
        "the checkout's attention was pruned against a scan nobody had run: {:?}",
        reloaded.keys().collect::<Vec<_>>()
    );
    assert!(
        !reloaded.contains_key(&dead_run),
        "a run no map knows about was spared because a project was unscanned: {:?}",
        reloaded.keys().collect::<Vec<_>>()
    );
}

/// A repository this daemon cannot read is an answer, not a question. The
/// attempt settles the board and stands until the interval is out, instead
/// of claiming a fresh scan on every poll and saying "scanning" forever.
#[test]
fn a_scan_that_cannot_read_its_repository_settles_the_board() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let computes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&computes);
    state.diff_compute_observer = Some(Arc::new(move |key| {
        if matches!(key, DiffCacheKey::ExternalScan(_)) {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    std::fs::remove_dir_all(&repo).unwrap();

    let first = state.external_worktrees(&project_id);
    assert_eq!(computes.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        first.settled,
        "the attempt that could not read the repository settled the read"
    );
    assert!(first.worktrees.is_empty(), "{:?}", first.worktrees);

    let again = state.external_worktrees(&project_id);
    assert!(again.settled, "the settled answer stands");
    assert_eq!(
        computes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a repository that cannot be read was re-scanned by the next poll"
    );
}

/// A create is not a reason to forget every other checkout. The new one
/// joins the last scan, so the very next board poll ships it without any
/// repository walk at all.
#[test]
fn a_created_worktree_joins_the_scan_cache_instead_of_clearing_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "already-here", "feature-here");
    state.scan_external_worktrees_now(&project_id).unwrap();
    let scanned_at = state
        .external_scan_of(&project_id)
        .expect("seeded above")
        .scanned_at;

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "scratch" }),
    ));
    assert_eq!(created["ok"], true, "{created:?}");
    let worktree_id = created["result"]["worktree_id"]
        .as_str()
        .unwrap()
        .to_string();

    let cache = state
        .external_scan_of(&project_id)
        .expect("the create emptied the whole project's scan");
    assert!(
        cache.worktrees.iter().any(|w| w.id == worktree_id),
        "the new checkout is in the cache the board reads: {:?}",
        cache.worktrees
    );
    assert!(
        cache
            .worktrees
            .iter()
            .any(|w| w.branch.as_deref() == Some("feature-here")),
        "the checkouts that were already there are still there: {:?}",
        cache.worktrees
    );
    assert_eq!(
        cache.scanned_at, scanned_at,
        "an amended list is exactly as old as the scan that filled it"
    );

    // And the board ships it with no scan of its own.
    let scans = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&scans);
    state.diff_compute_observer = Some(Arc::new(move |key| {
        if matches!(key, DiffCacheKey::ExternalScan(_)) {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["result"]["scanning"], json!(false), "{board:?}");
    assert!(
        board["result"]["external_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["worktree_id"] == json!(worktree_id)),
        "{board:?}"
    );
    assert_eq!(
        scans.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the board rescanned the repository for a checkout it had been handed"
    );
}

/// Age a project's last scan past the interval, so the next read of it
/// claims a rescan.
fn age_out_scan(state: &Arc<Mutex<AppState>>, project_id: &str) {
    let mut app = state.lock().unwrap();
    app.age_external_scan_for_test(project_id, EXTERNAL_SCAN_INTERVAL + Duration::from_secs(1));
}

/// A create that lands while a scan of the same repository is walking it.
/// The walk describes the repository as it was before the create, so what
/// it finds is dropped — but its claim is held to the end, because letting
/// it go lets the very next read start a second walk behind the first and
/// then lets the first land on top of the amendment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_create_during_a_scan_outlives_that_scans_landing() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    add_external_worktree(&repo, dir.path(), "already-here", "feature-here");
    settled_board(&handler).await;
    age_out_scan(&state, &project_id);

    let gate = gate_scan_computes(&state);
    poll_board(&handler).await;
    gate.wait_for_arrival();

    let created = {
        let handler = handler.clone();
        let project_id = project_id.clone();
        tokio::task::spawn_blocking(move || {
            call(
                &handler,
                "worktree.create",
                json!({ "project_id": project_id, "name": "scratch" }),
            )
        })
        .await
        .unwrap()
    };
    assert_eq!(created["ok"], true, "{created:?}");
    let worktree_id = created["result"]["worktree_id"]
        .as_str()
        .unwrap()
        .to_string();

    let scan = DiffCacheKey::ExternalScan(project_id.clone());
    assert!(
        state.lock().unwrap().diff_refresh_is_running(&scan),
        "the create un-claimed the scan it had already overtaken"
    );
    poll_board(&handler).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !gate.has_pending_arrival(),
        "a second walk of the same repository started behind the first"
    );

    gate.release();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if !state.lock().unwrap().diff_refresh_is_running(&scan) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the superseded scan lands and lets its claim go");

    let listed = state
        .lock()
        .unwrap()
        .external_scan_of(&project_id)
        .expect("the amended scan is still there")
        .worktrees
        .clone();
    assert!(
        listed.iter().any(|w| w.id == worktree_id),
        "the pre-create walk landed on top of the checkout the create had added: {listed:?}"
    );
}

/// The same rule for the caches a mutation empties rather than amends. A
/// stat computed against the tree as it was before the mutation is dropped,
/// and the claim it held is not handed to a second compute behind it.
#[test]
fn an_invalidated_stat_discards_the_compute_it_overtook() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = "run-1".to_string();
    let key = DiffCacheKey::RunStat(run_id.clone());
    let claim = state.claim_diff_refresh_for_test(key.clone());

    state.invalidate_run_stat(&run_id);
    assert!(
        state.diff_refresh_is_running(&key),
        "the mutation un-claimed a compute that is still running"
    );

    state.publish_diff_refresh(
        claim,
        Some(DiffCacheEntry::RunStat {
            run_id: run_id.clone(),
            stat: json!({ "files_changed": 3 }),
        }),
    );
    assert!(
        !state.has_cached_run_stat(&run_id),
        "a stat read before the mutation was published as the run's current one"
    );
    assert!(
        !state.diff_refresh_is_running(&key),
        "the superseded compute kept its claim after landing"
    );
}
