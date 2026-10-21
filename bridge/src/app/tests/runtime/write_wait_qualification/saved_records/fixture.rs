//! Small actual-wire fixture for concurrent accepted-record controls.
use super::super::rpc::{ReplyProof, RpcHarness};
use crate::app::accepted_records::AcceptedRecords;
use crate::app::mcp::{ControlPlane, handle_done_stream};
use crate::app::tests::*;
use crate::mcp::BridgeAction;
use crate::timing::write_wait::WriteWaitRegistry;
use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{Notify, Semaphore, oneshot};

const BUDGET: Duration = Duration::from_secs(5);

pub(super) enum SavedRequest {
    Mcp {
        original_id: String,
        action: BridgeAction,
    },
    Rpc {
        original_id: String,
        method: &'static str,
        params: Value,
    },
}

pub(super) struct SavedFixture {
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) rpc: RpcHarness,
    pub(super) owner: String,
    pub(super) caller: String,
    pub(super) token: String,
    pub(super) store: Store,
    pub(super) project: String,
    pub(super) task: String,
    plane: Arc<ControlPlane>,
    credits: Arc<Semaphore>,
    cold_attempts: Arc<AtomicUsize>,
    transport_errors: Mutex<Vec<String>>,
    _directory: tempfile::TempDir,
}

impl SavedFixture {
    pub(super) fn new(records: Arc<AcceptedRecords>) -> Self {
        let (directory, repo) = init_repo();
        let mut app = qa_state(&repo, directory.path());
        app.default_harness = AgentProvider::Claude;
        let cold_attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&cold_attempts);
        let denied = Agent::WarmBuilder(Arc::new(move |_, _, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(HarnessError::Unsupported(
                "accepted-record controls forbid cold providers".into(),
            ))
        }));
        app.agent = denied.clone();
        let worktrees = app.worktrees_root.clone();
        app.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            denied,
            Templates::default(),
            test_bridge_exe(),
        );
        let owner = adopted_run(&mut app, &repo, directory.path(), "saved-record-control");
        let caller = primary_agent_id(&app, &owner);
        let project = app.project_at(0).id.clone();
        let created = app.handle(req("tasks.create", json!({
            "project_id":project,"title":"Concurrent accepted status","body":"Bounded saved-record control",
        })));
        assert_eq!(created["ok"], true, "{created}");
        let task = created["result"]["task"]["id"].as_str().unwrap().to_owned();
        app.delivery_queue.clear_queued();
        let token = "saved-record-issued-coding-capability".to_owned();
        app.test_install_mcp_token(caller.clone(), token.clone());
        let store = app.store.as_ref().unwrap().clone();
        records.seed_known_durable_user_session(store.load_user_session().unwrap());
        app.test_capture_accepted_records(Arc::clone(&records));
        app.initialize_read_context();
        app.initialize_snapshot_projection();
        // These controls mutate records only; no background provider service is needed.
        let state = Arc::new(Mutex::new(app));
        let rpc = RpcHarness::new(Arc::clone(&state), WriteWaitRegistry::new());
        let plane = ControlPlane::new(&state.lock().unwrap());
        Self {
            state,
            rpc,
            owner,
            caller,
            token,
            store,
            project,
            task,
            plane,
            credits: Arc::new(Semaphore::new(2)),
            cold_attempts,
            transport_errors: Mutex::new(Vec::new()),
            _directory: directory,
        }
    }

    pub(super) async fn rpc_exact(
        &self,
        caller: usize,
        original: &str,
        method: &str,
        params: Value,
    ) -> Result<ReplyProof, String> {
        let result = AssertUnwindSafe(self.rpc.request(caller, original, method, params))
            .catch_unwind()
            .await
            .map_err(|_| format!("RPC infrastructure failed for {original}"));
        if let Err(error) = &result {
            self.transport_errors.lock().unwrap().push(error.clone());
        }
        result
    }

    pub(super) async fn mcp_exact(
        &self,
        original: &str,
        action: &BridgeAction,
    ) -> Result<ReplyProof, String> {
        self.mcp_sent(original, action, None).await
    }

    async fn mcp_sent(
        &self,
        original: &str,
        action: &BridgeAction,
        progress: Option<&Offers>,
    ) -> Result<ReplyProof, String> {
        let result = async {
        let _credit = Arc::clone(&self.credits)
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        let (client, daemon) = tokio::net::UnixStream::pair().map_err(|error| error.to_string())?;
        let mut server = tokio::spawn(handle_done_stream(
            Arc::clone(&self.state),
            daemon,
            Arc::clone(&self.plane),
        ));
        let mut client = tokio::io::BufReader::new(client);
        let frame = json!({"id":original,"task_id":self.caller,"session_token":self.token,"request":action});
        let sent_at = Instant::now();
        let written = client
            .get_mut()
            .write_all(format!("{frame}\n").as_bytes())
            .await;
        let mut response = String::new();
        let result = match written {
            Ok(()) => {
                if let Some(progress) = progress {
                    progress.written.fetch_add(1, Ordering::SeqCst);
                    progress.changed.notify_one();
                }
                match tokio::time::timeout(BUDGET, client.read_line(&mut response)).await {
                    Ok(Ok(0)) => Err("MCP closed without a final reply".into()),
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("MCP final reply exceeded five seconds".into()),
                }
            }
            Err(error) => Err(error.to_string()),
        };
        let final_reply_at = Instant::now();
        drop(client);
        // Even an error closes and joins its real server before returning.
        match tokio::time::timeout(BUDGET, &mut server).await {
            Ok(joined) => joined.map_err(|error| error.to_string())?,
            Err(_) => {
                server.abort();
                let stopped = server.await;
                return Err(format!(
                    "MCP server exceeded cleanup deadline; stopped handler: {stopped:?}"
                ));
            }
        }
        result?;
        let reply = serde_json::from_str(&response).map_err(|error| error.to_string())?;
        Ok(ReplyProof {
            original_id: original.into(),
            method: format!("mcp.control:{}", action.tool_name()),
            reply,
            sent_at,
            final_reply_at,
        })
        }.await;
        if let Err(error) = &result {
            self.transport_errors
                .lock()
                .unwrap()
                .push(format!("{original}: {error}"));
        }
        result
    }

    pub(super) async fn compete(
        &self,
        requests: Vec<SavedRequest>,
    ) -> Vec<Result<ReplyProof, String>> {
        let count = requests.len();
        let required_sockets = requests
            .iter()
            .filter(|request| matches!(request, SavedRequest::Mcp { .. }))
            .count()
            .min(2);
        let held = match HeldApp::acquire(Arc::clone(&self.state)).await {
            Ok(held) => held,
            Err(error) => {
                self.transport_errors.lock().unwrap().push(error.clone());
                return (0..count).map(|_| Err(error.clone())).collect();
            }
        };
        let offers = Offers::default();
        let mut calls: FuturesUnordered<_> = requests
            .into_iter()
            .enumerate()
            .map(|(index, request)| {
                let offers = &offers;
                async move {
                    offers.entered.fetch_add(1, Ordering::SeqCst);
                    offers.changed.notify_one();
                    let result = match request {
                        SavedRequest::Mcp {
                            original_id,
                            action,
                        } => self.mcp_sent(&original_id, &action, Some(offers)).await,
                        SavedRequest::Rpc {
                            original_id,
                            method,
                            params,
                        } => {
                            self.rpc_exact(index % 4, &original_id, method, params)
                                .await
                        }
                    };
                    (index, result)
                }
            })
            .collect();
        let gather = async {
            let mut out: Vec<_> = (0..count).map(|_| None).collect();
            while let Some((index, result)) = calls.next().await {
                out[index] = Some(result);
            }
            out.into_iter()
                .map(|result| result.unwrap())
                .collect::<Vec<_>>()
        };
        let release = async {
            let offered = offers.wait(count, required_sockets).await;
            let released = held.release().await;
            offered.and(released)
        };
        let (results, released) = tokio::join!(gather, release);
        match released {
            Ok(()) => results,
            Err(error) => {
                self.transport_errors.lock().unwrap().push(error.clone());
                results
                    .into_iter()
                    .map(|result| {
                        Err(format!(
                            "concurrent offer barrier failed: {error}; final reply: {}",
                            result
                                .map(|proof| proof.reply.to_string())
                                .unwrap_or_else(|error| error)
                        ))
                    })
                    .collect()
            }
        }
    }

    pub(super) async fn finish(self) -> Result<tempfile::TempDir, String> {
        let Self {
            state,
            rpc,
            plane,
            cold_attempts,
            transport_errors,
            _directory,
            ..
        } = self;
        let weak = Arc::downgrade(&state);
        let registry = Arc::clone(&rpc.registry);
        drop(plane);
        drop(state);
        let mut errors = transport_errors
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match AssertUnwindSafe(rpc.finish_report()).catch_unwind().await {
            Ok(report) if report.is_clean() => {}
            Ok(report) => errors.push(format!("RPC cleanup {report:?}")),
            Err(_) => errors.push("RPC cleanup panicked".into()),
        }
        let cold = cold_attempts.load(Ordering::SeqCst);
        if cold != 0 {
            errors.push(format!("unexpected cold attempts={cold}"));
        }
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            let pending = registry.snapshot().into_iter().any(|row| {
                !row.root_finished || !row.outstanding.is_empty() || !row.active.is_empty()
            });
            if weak.strong_count() == 0 && !pending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let owners = weak.strong_count();
        if owners != 0 {
            errors.push("accepted work still owns AppState after cleanup deadline".into());
        }
        let unsettled: Vec<_> = registry
            .snapshot()
            .into_iter()
            .filter(|row| {
                !row.root_finished || !row.outstanding.is_empty() || !row.active.is_empty()
            })
            .map(|row| row.original_id)
            .collect();
        if !unsettled.is_empty() {
            errors.push(format!("unsettled originals={unsettled:?}"));
        }
        if errors.is_empty() {
            Ok(_directory)
        } else {
            let preserved = _directory.keep();
            Err(format!(
                "{}; private root preserved: {}; pid={}; AppState owners={owners}; unsettled originals={unsettled:?}",
                errors.join("; "),
                preserved.display(),
                std::process::id()
            ))
        }
    }
}

#[derive(Default)]
struct Offers {
    entered: AtomicUsize,
    written: AtomicUsize,
    changed: Notify,
}
impl Offers {
    async fn wait(&self, count: usize, sockets: usize) -> Result<(), String> {
        let wait = async {
            while self.entered.load(Ordering::SeqCst) < count
                || self.written.load(Ordering::SeqCst) < sockets
            {
                self.changed.notified().await;
            }
        };
        tokio::time::timeout(BUDGET, wait).await.map_err(|_| {
            format!(
                "entered={}/{count}, actual sockets written={}/{sockets}",
                self.entered.load(Ordering::SeqCst),
                self.written.load(Ordering::SeqCst)
            )
        })
    }
}

struct HeldApp {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HeldApp {
    async fn acquire(state: Arc<Mutex<AppState>>) -> Result<Self, String> {
        let (release, waiting) = std::sync::mpsc::channel();
        let (ready, acquired) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            let _guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = ready.send(());
            let _ = waiting.recv();
        });
        let mut held = Self {
            release: Some(release),
            thread: Some(thread),
        };
        if acquired.await.is_err() {
            held.stop();
            return Err("app holder did not acquire".into());
        }
        Ok(held)
    }
    async fn release(mut self) -> Result<(), String> {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let thread = self.thread.take().unwrap();
        tokio::task::spawn_blocking(move || thread.join())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|_| "app holder panicked".into())
    }
    fn stop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for HeldApp {
    fn drop(&mut self) {
        self.stop();
    }
}
