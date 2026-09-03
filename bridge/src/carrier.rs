//! The carrier boundary: what the app pushes to a client session, and who owns
//! the session keys.
//!
//! A carrier is a wire, not a protocol. The relay socket is one; a WebRTC
//! DataChannel is another. Everything above this line — the session key, the
//! encrypted frames, dispatch and teardown — is the same on either, so nothing
//! above it learns which wire is carrying.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::transport::{self, Envelope, Frame, OuterFields};

/// One encrypted frame bound for one client session, before any wire wrapper
/// exists. The carrier that takes it decides how to frame it: the relay writer
/// wraps it as `{"type":"e2ee_envelope",…}`, a DataChannel sends the envelope
/// JSON directly.
#[derive(Debug, Clone)]
pub struct OutboundEnvelope {
    pub session_id: String,
    pub envelope: Envelope,
}

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl SessionSender {
    /// The session this sender targets.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    fn keyed(
        session_id: &str,
        session_key: String,
        out: mpsc::UnboundedSender<OutboundEnvelope>,
    ) -> Self {
        SessionSender {
            session_id: session_id.to_string(),
            session_key,
            out,
        }
    }

    /// A sender not bound to a live connection — for tests and request/response
    /// callers that never push. `push` succeeds-into-the-void.
    pub fn detached(session_id: impl Into<String>) -> Self {
        let (out, _rx) = mpsc::unbounded_channel();
        SessionSender {
            session_id: session_id.into(),
            session_key: String::new(),
            out,
        }
    }

    /// Test-only: a sender with a real session key and a captured channel, so
    /// tests can decrypt every pushed frame (`term.output`, `term.closed`, …)
    /// with [`decrypt_push`](Self::decrypt_push).
    #[cfg(test)]
    pub fn observable(
        session_id: impl Into<String>,
    ) -> (Self, mpsc::UnboundedReceiver<OutboundEnvelope>, String) {
        let (out, rx) = mpsc::unbounded_channel();
        let session_key = transport::generate_session_key();
        (
            SessionSender {
                session_id: session_id.into(),
                session_key: session_key.clone(),
                out,
            },
            rx,
            session_key,
        )
    }

    /// Test-only: decode one captured [`Self::observable`] envelope back to the
    /// pushed inner payload.
    #[cfg(test)]
    pub fn decrypt_push(session_key: &str, outbound: &OutboundEnvelope) -> Value {
        transport::decrypt_envelope(session_key, &outbound.envelope)
            .expect("push decrypts with the session key")
            .payload
    }

    /// Encrypt `payload` as an inner frame and hand it to the carrier. Returns
    /// false once the carrier is gone (so the app can drop the stale sender).
    pub fn push(&self, payload: Value) -> bool {
        let envelope = match transport::encrypt_frame(
            &self.session_key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: format!("session:{}", self.session_id),
            },
            &transport::FrameFields {
                frame_type: "data".into(),
                sender: "device".into(),
                payload,
                message_id: None,
                created_at: None,
            },
            None,
        ) {
            Ok(env) => env,
            Err(_) => return false,
        };
        self.out
            .send(OutboundEnvelope {
                session_id: self.session_id.clone(),
                envelope,
            })
            .is_ok()
    }
}

/// What went wrong with one frame on one carrier. Never fatal to the carrier:
/// a session the device does not know is a frame it drops, not a wire it closes.
#[derive(Debug, thiserror::Error)]
pub enum CarrierError {
    #[error("no session key for {0}")]
    UnknownSession(String),
    #[error("session {0} is already open under a different key")]
    KeyMismatch(String),
    #[error("transport error: {0}")]
    Transport(#[from] transport::TransportError),
}

/// A carrier, as the registry knows one: process-unique, so "which carriers does
/// this session ride" has an answer that outlives any one of them.
type CarrierId = u64;

/// One live wire — a relay socket generation, or a DataChannel — as everything
/// above the wire sees it: somewhere to put envelopes for any session, since one
/// wire carries every client session of the device.
#[derive(Clone)]
pub struct CarrierHandle {
    id: CarrierId,
    out: mpsc::UnboundedSender<OutboundEnvelope>,
}

impl CarrierHandle {
    pub fn new(out: mpsc::UnboundedSender<OutboundEnvelope>) -> Self {
        static NEXT_CARRIER_ID: AtomicU64 = AtomicU64::new(1);
        CarrierHandle {
            id: NEXT_CARRIER_ID.fetch_add(1, Ordering::Relaxed),
            out,
        }
    }

    pub(crate) fn id(&self) -> CarrierId {
        self.id
    }
}

/// One open session: the key it was minted with and the carriers riding it.
struct OpenSession {
    key: String,
    carriers: HashSet<CarrierId>,
}

/// The one home of `session_id → (session key, carriers riding it)`.
///
/// It is shared and it outlives every carrier, which is what lets a session
/// minted over one relay socket keep working over a DataChannel and over the
/// next relay socket. It owns the teardown rule and is the only place that rule
/// is written: **a session ends when its last carrier is gone, or when its
/// client says so**. A caller gets a decrypted frame and a sender back; the key
/// material never leaves this module.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<String, OpenSession>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry::default()
    }

    /// Mint a session, or attach another carrier to one already open.
    ///
    /// Idempotent for a known session whose key matches — that is the re-attach
    /// a browser performs when its relay socket reconnects. A known session
    /// presented under a different key is refused; the frame is dropped.
    pub fn open(
        &self,
        session_id: &str,
        session_key: String,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(session_id) {
            Some(open) if open.key != session_key => {
                Err(CarrierError::KeyMismatch(session_id.to_string()))
            }
            Some(open) => {
                open.carriers.insert(carrier.id);
                Ok(())
            }
            None => {
                sessions.insert(
                    session_id.to_string(),
                    OpenSession {
                        key: session_key,
                        carriers: HashSet::from([carrier.id]),
                    },
                );
                Ok(())
            }
        }
    }

    /// Take one envelope off a carrier: decrypt it with the session's key and
    /// hand back the frame with a sender that pushes to this carrier. The ride
    /// is recorded only once the frame decrypts — the envelope is authenticated,
    /// so a frame that does not is the one thing that must never bind a session
    /// to a wire — and recording it is what makes a session reachable from
    /// whichever wire its frames arrive on.
    pub fn admit(
        &self,
        envelope: &Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(Frame, SessionSender), CarrierError> {
        let session_key = self.key_of(&envelope.session_id)?;
        let frame = transport::decrypt_envelope(&session_key, envelope)?;
        {
            let mut sessions = self.sessions.lock().unwrap();
            let open = sessions
                .get_mut(&envelope.session_id)
                .ok_or_else(|| CarrierError::UnknownSession(envelope.session_id.clone()))?;
            open.carriers.insert(carrier.id);
        }
        let sender = SessionSender::keyed(&envelope.session_id, session_key, carrier.out.clone());
        Ok((frame, sender))
    }

    fn key_of(&self, session_id: &str) -> Result<String, CarrierError> {
        self.sessions
            .lock()
            .unwrap()
            .get(session_id)
            .map(|open| open.key.clone())
            .ok_or_else(|| CarrierError::UnknownSession(session_id.to_string()))
    }

    /// One carrier stops carrying one session.
    pub fn release_session(&self, session_id: &str, carrier: CarrierId) -> Vec<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(open) = sessions.get_mut(session_id) else {
            return Vec::new();
        };
        open.carriers.remove(&carrier);
        if !open.carriers.is_empty() {
            return Vec::new();
        }
        sessions.remove(session_id);
        vec![session_id.to_string()]
    }

    /// One carrier is gone: every session that rode nothing else ends with it.
    pub fn release_carrier(&self, carrier: CarrierId) -> Vec<String> {
        let mut ended = Vec::new();
        self.sessions.lock().unwrap().retain(|session_id, open| {
            open.carriers.remove(&carrier);
            if open.carriers.is_empty() {
                ended.push(session_id.clone());
                return false;
            }
            true
        });
        ended
    }

    /// The client said so: the session ends however many carriers it rides.
    pub fn end(&self, session_id: &str) -> Vec<String> {
        match self.sessions.lock().unwrap().remove(session_id) {
            Some(_) => vec![session_id.to_string()],
            None => Vec::new(),
        }
    }
}

/// "One envelope arrived on some carrier" — admit it, honour a `close` frame,
/// else dispatch it, and emit the synthetic `close` frame of every session the
/// registry reports ended.
///
/// One of these is built in `main.rs` and shared by every carrier, so the
/// worker pool, the ordered terminal lanes and the read fold are one set of
/// resources however many wires the device is carrying. It reads the session id
/// off the envelope: a carrier binds to no session.
pub struct FrameIntake {
    registry: Arc<SessionRegistry>,
    dispatcher: Dispatcher,
}

impl FrameIntake {
    pub fn new(registry: Arc<SessionRegistry>, handler: FrameHandler) -> Arc<Self> {
        Arc::new(FrameIntake {
            registry,
            dispatcher: Dispatcher::new(handler),
        })
    }

    /// A client opened a session on this carrier (or re-attached an open one).
    pub fn open(
        &self,
        session_id: &str,
        session_key: String,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        self.registry.open(session_id, session_key, carrier)
    }

    /// One envelope arrived on this carrier: admit it through the registry,
    /// honour a `close` frame, else dispatch it. A frame for a session the
    /// device does not know is refused, and the carrier carries on.
    pub async fn accept(
        &self,
        envelope: Envelope,
        carrier: &CarrierHandle,
    ) -> Result<(), CarrierError> {
        let (frame, sender) = self.registry.admit(&envelope, carrier)?;
        if frame.frame_type == "close" {
            self.close_ended(self.registry.end(&envelope.session_id));
            return Ok(());
        }
        self.dispatcher.dispatch(sender, frame).await;
        Ok(())
    }

    /// This carrier stops carrying this session — the relay's `session_closed`,
    /// or a channel that closed under one session.
    pub fn close_session(&self, session_id: &str, carrier: CarrierId) {
        self.close_ended(self.registry.release_session(session_id, carrier));
    }

    /// This carrier is gone.
    pub fn close_carrier(&self, carrier: CarrierId) {
        self.close_ended(self.registry.release_carrier(carrier));
    }

    fn close_ended(&self, ended: Vec<String>) {
        for session_id in ended {
            self.dispatcher.close_session(&session_id);
        }
    }
}

/// Handles a decrypted request frame. Receives a [`SessionSender`] (so it can
/// register the session for server-initiated pushes) and returns the response
/// payload to send back.
pub type FrameHandler = Arc<dyn Fn(SessionSender, Frame) -> Value + Send + Sync>;

/// How many handlers may run at once. Handlers are blocking (they take the app
/// mutex, and some of them walk a worktree with libgit2), so they run on the
/// blocking pool and this is the width of the useful work, not of the CPU: wide
/// enough that a couple of slow diffs still leave room for the cheap calls a
/// browser makes beside them, narrow enough that a flood of expensive frames
/// cannot put dozens of concurrent worktree scans on the disk at once.
const DISPATCH_WORKERS: usize = 8;

/// How many frames may wait for a worker. The socket is read as fast as frames
/// arrive and each waiting frame is a small JSON payload, so this is deep enough
/// to absorb a multi-second stall of every poller a few browser tabs can run
/// (the SPA polls a couple of times a second per tab) without the memory ever
/// mattering. Past it, `dispatch` waits: the flood backs up in a queue whose size
/// is known instead of growing until the process dies.
const DISPATCH_QUEUE_DEPTH: usize = 256;

/// How many frames may wait on one ordered lane. A lane carries one terminal's
/// input, acks and resizes — frames whose handlers are a write to a pty fd — so a
/// lane this deep holds far more than the largest paste burst a client can make
/// before the caller is asked to wait.
const LANE_QUEUE_DEPTH: usize = 64;

/// The method that ends a terminal — and with it the terminal's ordered lane.
const TERMINAL_CLOSE_METHOD: &str = "term.close";

/// The read verbs whose answer is a snapshot of state the bridge already holds:
/// asking twice in a row changes nothing and the two answers describe the same
/// moment. Only these are folded together while they wait for a worker.
///
/// The list is explicit, never a prefix rule. `git.stash` and `git.status` share
/// a namespace and nothing else, and a verb that changes something must run every
/// time it is sent — folding two `git.commit` frames would drop a commit the
/// human asked for. Adding a verb here is a claim that running it once for two
/// callers is the same as running it twice; make that claim deliberately.
const COALESCED_READ_METHODS: [&str; 4] = ["board.list", "project.list", "git.status", "git.log"];

/// How many callers one fold may answer. A folded read costs no queue slot, so
/// the fold needs its own bound or it becomes the unbounded growth the queue's
/// limit exists to prevent. Well past what a real stall produces — a few browser
/// tabs polling twice a second reach this only after minutes with no worker free —
/// and past it a read simply queues on its own, where the queue holds the caller.
const MAX_FOLDED_READS: usize = 256;

/// One decrypted frame waiting for a handler.
struct Job {
    sender: SessionSender,
    frame: Frame,
}

/// What identifies a read that can stand in for another: one client asking one
/// question. `(session_id, method, params)`, with the params carried as their
/// JSON text rather than a hash — a hash collision here would answer one request
/// with another request's result, and the text is a few dozen bytes.
type ReadKey = (String, String, String);

/// The identical reads queued behind one worker, waiting to be answered together.
struct FoldedRead {
    /// The newest of them: the one whose handler actually runs. They ask the same
    /// question of the same session, so any of them would do, and the newest is
    /// the one whose arrival the answer is guaranteed to postdate.
    sender: SessionSender,
    frame: Frame,
    /// The request id of every folded frame, in arrival order. Each one is
    /// answered — see [`Dispatcher`] on why none of them may simply be dropped.
    ids: Vec<Value>,
}

/// What became of a read offered to the fold.
enum Folded {
    /// It joined a fold already waiting; that fold's one answer covers it.
    Joined,
    /// It is the first of its kind: it now heads a fold that needs a queue slot.
    Heads,
    /// The fold it would have joined is full; it runs on its own. Boxed to keep
    /// the enum small: the other two arms carry nothing and this one is the rare
    /// case — only a fold already holding hundreds of callers reaches it.
    Overflowed(Box<Job>),
}

/// What the shared queue carries.
enum QueuedWork {
    /// One frame, run on its own.
    Frame(Job),
    /// A place in line for a set of identical reads. The frames themselves wait
    /// in [`Dispatcher::folded_reads`] until a worker takes this marker, so a
    /// poll that arrives while its twin is still queued costs no queue slot.
    FoldedRead(ReadKey),
}

/// What an ordered lane can be asked to do.
enum LaneMessage {
    /// Run this frame's handler; the lane runs one at a time, in arrival order.
    Run(Job),
    /// Tell me when everything queued before you has run. Used to make a session
    /// close the last thing that happens to that session.
    Fence(tokio::sync::oneshot::Sender<()>),
}

/// Runs frame handlers off the read loop.
///
/// The read loop reads, decrypts and enqueues; nothing else. Handlers run on a
/// bounded pool of workers, each of which calls the handler on a blocking thread
/// (handlers take the app mutex and can walk a worktree), so no handler — however
/// slow — can keep the socket from being drained. That was the wedge: one
/// `board.list` diff on a churning 56k-file worktree took seconds, and the frames
/// behind it piled up in the kernel until the relay called the device dead.
///
/// Frames that mean something as a stream keep their order: anything naming a
/// `term_id` goes to a serial lane keyed by that terminal and that session, so a
/// client's input and acks are handled in the order it sent them. Everything else
/// is an independent request and runs concurrently.
///
/// Identical queued reads are folded together. The same stall left hundreds of
/// `board.list` polls waiting, each answered with its own full recompute; now the
/// second and later arrivals of a read already waiting for a worker join the one
/// in front of them, and one compute answers them all.
///
/// **Answer every caller; never drop the older frame.** The browser correlates a
/// reply by the per-call id it minted (`spa/src/core/session.js`: a `pending` map
/// keyed by `r<n>`, each entry rejected by a 12 s timer). A dropped frame is not a
/// request the client forgets about — it is one that hangs until that timer fires
/// and surfaces as "board.list timed out". So the fold saves the *compute*, not
/// the reply: the one result is pushed back once per waiting id.
///
/// A read that arrives after its twin has started running is not folded into it —
/// it gets its own compute. The client asked at a moment the running answer
/// predates, and serving a snapshot older than the question is how a board goes
/// stale and stays stale.
struct Dispatcher {
    handler: FrameHandler,
    /// The shared pool queue. Bounded: a full queue makes the read loop wait.
    jobs: mpsc::Sender<QueuedWork>,
    /// (session_id, term_id) → its serial lane. Behind its own lock, held
    /// only long enough to clone a lane's sender, so no lock crosses an await
    /// and every carrier can dispatch through one shared dispatcher.
    lanes: Mutex<HashMap<(String, String), mpsc::Sender<LaneMessage>>>,
    /// The reads queued but not yet started, by what they ask. Shared with the
    /// workers: a worker takes an entry out at the moment it starts computing,
    /// which is exactly the moment further arrivals must stop joining it.
    folded_reads: Arc<Mutex<HashMap<ReadKey, FoldedRead>>>,
}

impl Dispatcher {
    fn new(handler: FrameHandler) -> Self {
        Self::with_capacity(handler, DISPATCH_QUEUE_DEPTH, DISPATCH_WORKERS)
    }

    fn with_capacity(handler: FrameHandler, queue_depth: usize, workers: usize) -> Self {
        let (jobs, rx) = mpsc::channel::<QueuedWork>(queue_depth.max(1));
        // One queue, many workers: whoever is free takes the next frame.
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let folded_reads: Arc<Mutex<HashMap<ReadKey, FoldedRead>>> =
            Arc::new(Mutex::new(HashMap::new()));
        for _ in 0..workers.max(1) {
            let rx = rx.clone();
            let handler = handler.clone();
            let folded_reads = folded_reads.clone();
            tokio::spawn(async move {
                loop {
                    let work = rx.lock().await.recv().await;
                    let Some(work) = work else { break };
                    match work {
                        QueuedWork::Frame(job) => run_job(&handler, job).await,
                        QueuedWork::FoldedRead(key) => {
                            // Taking the entry out is what closes the fold: from
                            // here on, the same read queues afresh behind us.
                            let folded = folded_reads.lock().unwrap().remove(&key);
                            if let Some(folded) = folded {
                                run_folded_read(&handler, folded).await;
                            }
                        }
                    }
                }
            });
        }
        Dispatcher {
            handler,
            jobs,
            lanes: Mutex::new(HashMap::new()),
            folded_reads,
        }
    }

    /// Hand one decrypted request frame to a worker. Waits only when every
    /// worker is busy and the queue is full.
    async fn dispatch(&self, sender: SessionSender, frame: Frame) {
        if let Some(key) = ordered_lane(&sender, &frame) {
            let closing =
                frame.payload.get("method").and_then(Value::as_str) == Some(TERMINAL_CLOSE_METHOD);
            let lane = self.lane(key.clone());
            let _ = lane.send(LaneMessage::Run(Job { sender, frame })).await;
            if closing {
                // A terminal id is minted once and never reused, so its close
                // is the last frame its lane can carry. Letting the sender go
                // ends the lane as soon as it has run that close.
                self.lanes.lock().unwrap().remove(&key);
            }
            return;
        }
        let job = match read_key(&sender, &frame) {
            Some(key) => match self.fold_into_queued_read(key.clone(), sender, frame) {
                Folded::Joined => return, // an identical read is waiting; it answers both
                Folded::Heads => {
                    if self
                        .jobs
                        .send(QueuedWork::FoldedRead(key.clone()))
                        .await
                        .is_err()
                    {
                        // No workers left to take the marker: don't leave the
                        // frames parked in a map nothing will ever drain.
                        self.folded_reads.lock().unwrap().remove(&key);
                    }
                    return;
                }
                Folded::Overflowed(job) => *job,
            },
            None => Job { sender, frame },
        };
        let _ = self.jobs.send(QueuedWork::Frame(job)).await;
    }

    /// Join a read to an identical one already waiting for a worker, if there is
    /// one and it has room.
    fn fold_into_queued_read(&self, key: ReadKey, sender: SessionSender, frame: Frame) -> Folded {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let mut folded_reads = self.folded_reads.lock().unwrap();
        match folded_reads.get_mut(&key) {
            Some(waiting) if waiting.ids.len() < MAX_FOLDED_READS => {
                waiting.ids.push(id);
                // Run the newest frame of the fold, so the answer postdates the
                // last question it answers.
                waiting.sender = sender;
                waiting.frame = frame;
                Folded::Joined
            }
            // A full fold: this read takes a queue slot of its own, which is what
            // puts the caller back under the queue's bound.
            Some(_) => Folded::Overflowed(Box::new(Job { sender, frame })),
            None => {
                folded_reads.insert(
                    key,
                    FoldedRead {
                        sender,
                        frame,
                        ids: vec![id],
                    },
                );
                Folded::Heads
            }
        }
    }

    /// The lane for a terminal, started on first use.
    fn lane(&self, key: (String, String)) -> mpsc::Sender<LaneMessage> {
        self.lanes
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| {
                let (tx, mut rx) = mpsc::channel::<LaneMessage>(LANE_QUEUE_DEPTH);
                let handler = self.handler.clone();
                tokio::spawn(async move {
                    while let Some(message) = rx.recv().await {
                        match message {
                            LaneMessage::Run(job) => run_job(&handler, job).await,
                            LaneMessage::Fence(reply) => {
                                let _ = reply.send(());
                            }
                        }
                    }
                });
                tx
            })
            .clone()
    }

    /// A session ended: tell the app so it releases the session's attachments.
    ///
    /// This is the session's last frame and it is treated as one — it runs only
    /// after every frame that arrived before it, or an attach still sitting on a
    /// lane would register a sender into a session already closed and push
    /// terminal output at a browser that is gone. Taking the lanes out of the
    /// map is all of that this call does: draining them is a wait on however
    /// slow the terminal's handler is, and the carrier teardown that reports the
    /// end — the one a relay reconnect is queued behind — never waits on it.
    fn close_session(&self, session_id: &str) {
        let session_lanes: Vec<mpsc::Sender<LaneMessage>> = {
            let mut lanes = self.lanes.lock().unwrap();
            let keys: Vec<(String, String)> = lanes
                .keys()
                .filter(|(session, _)| session == session_id)
                .cloned()
                .collect();
            keys.iter().filter_map(|key| lanes.remove(key)).collect()
        };

        let handler = self.handler.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            for lane in session_lanes {
                let (reply, wait) = tokio::sync::oneshot::channel();
                if lane.send(LaneMessage::Fence(reply)).await.is_ok() {
                    let _ = wait.await;
                }
                // Dropping the lane sender ends the lane once it has drained.
            }
            let closed = Frame {
                session_id: session_id.clone(),
                message_id: String::new(),
                frame_type: "close".into(),
                sender: "relay".into(),
                created_at: String::new(),
                payload: Value::Null,
            };
            let sender = SessionSender::detached(&session_id);
            // No response: nobody is left to read one.
            let _ = tokio::task::spawn_blocking(move || handler(sender, closed)).await;
        });
    }
}

/// The serial lane a frame belongs to, if its order is part of its meaning.
/// Terminal traffic — input, acks, resizes, attach and close — is a stream per
/// terminal per client; everything else is an independent request.
fn ordered_lane(sender: &SessionSender, frame: &Frame) -> Option<(String, String)> {
    let term_id = frame
        .payload
        .get("params")
        .and_then(|params| params.get("term_id"))
        .and_then(Value::as_str)?;
    Some((sender.session_id().to_string(), term_id.to_string()))
}

/// What this frame asks, if it is a read whose answer can serve another caller
/// asking the same thing. `None` for every other frame — including any read that
/// names a `term_id`, which never reaches here (terminal traffic is a lane).
///
/// The params go into the key as their JSON text. `serde_json` orders object keys
/// (its map is a `BTreeMap`), so two frames carrying the same params produce the
/// same text; were that ever to stop holding, the only effect is a fold that does
/// not happen — equal text always means equal params, never the reverse.
fn read_key(sender: &SessionSender, frame: &Frame) -> Option<ReadKey> {
    let method = frame.payload.get("method").and_then(Value::as_str)?;
    if !COALESCED_READ_METHODS.contains(&method) {
        return None;
    }
    let params = frame
        .payload
        .get("params")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Some((
        sender.session_id().to_string(),
        method.to_string(),
        params.to_string(),
    ))
}

/// Run one read for every caller that asked it: compute once, then push that one
/// result back under each waiting request id.
async fn run_folded_read(handler: &FrameHandler, folded: FoldedRead) {
    let FoldedRead { sender, frame, ids } = folded;
    let handler = handler.clone();
    let answering = sender.clone();
    let answer = match tokio::task::spawn_blocking(move || handler(answering, frame)).await {
        Ok(payload) => payload,
        // The handler panicked (or the runtime is shutting down). Answer anyway:
        // a client that never hears back waits forever.
        Err(_) => json!({ "ok": false, "error": "handler failed" }),
    };
    for id in ids {
        let mut for_caller = answer.clone();
        match for_caller.as_object_mut() {
            // The handler stamped the running frame's id; each caller needs its
            // own, or its `pending` entry never resolves.
            Some(payload) => {
                payload.insert("id".into(), id);
            }
            None => continue,
        }
        sender.push(for_caller);
    }
}

/// Run one handler on a blocking thread and send its answer back.
///
/// Blocking, not async: a handler takes the app mutex and may sit in libgit2 for
/// seconds. On a runtime worker that would block the read loop and the writer
/// with it — the very thing this queue exists to prevent.
async fn run_job(handler: &FrameHandler, job: Job) {
    let Job { sender, frame } = job;
    let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
    let handler = handler.clone();
    let answering = sender.clone();
    match tokio::task::spawn_blocking(move || handler(answering, frame)).await {
        Ok(payload) => {
            sender.push(payload);
        }
        // The handler panicked (or the runtime is shutting down). Answer anyway:
        // a client that never hears back waits forever.
        Err(_) => {
            sender.push(json!({ "id": id, "ok": false, "error": "handler failed" }));
        }
    }
}

#[cfg(test)]
mod sender_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_push_hands_the_carrier_an_envelope_and_no_wire_wrapper() {
        let (sender, mut pushes, key) = SessionSender::observable("s-1");

        assert!(sender.push(json!({ "id": 7, "ok": true })));

        let outbound = pushes.try_recv().expect("the push arrived");
        assert_eq!(outbound.session_id, "s-1");
        assert_eq!(outbound.envelope.route_to, "session:s-1");
        assert_eq!(
            SessionSender::decrypt_push(&key, &outbound),
            json!({ "id": 7, "ok": true })
        );
    }

    #[test]
    fn a_push_to_a_gone_carrier_reports_failure() {
        let (sender, pushes, _key) = SessionSender::observable("s-gone");
        drop(pushes);

        assert!(!sender.push(json!({ "ok": true })));
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use crate::transport::FrameFields;
    use serde_json::json;

    pub(super) fn test_carrier() -> (CarrierHandle, mpsc::UnboundedReceiver<OutboundEnvelope>) {
        let (out, rx) = mpsc::unbounded_channel();
        (CarrierHandle::new(out), rx)
    }

    pub(super) fn client_envelope(
        session_key: &str,
        session_id: &str,
        frame_type: &str,
    ) -> Envelope {
        transport::encrypt_frame(
            session_key,
            &OuterFields {
                session_id: session_id.to_string(),
                route_to: "device:d-1".into(),
            },
            &FrameFields {
                frame_type: frame_type.into(),
                sender: "client".into(),
                payload: json!({ "id": 1, "method": "board.list" }),
                message_id: None,
                created_at: None,
            },
            None,
        )
        .expect("the client can encrypt to its own session key")
    }

    #[test]
    fn a_session_opened_on_a_carrier_admits_that_carrier_s_frames() {
        let registry = SessionRegistry::new();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();

        registry
            .open("s-1", key.clone(), &carrier)
            .expect("a fresh session opens");

        let (frame, sender) = registry
            .admit(&client_envelope(&key, "s-1", "data"), &carrier)
            .expect("a frame for a known session is admitted");
        assert_eq!(frame.payload["method"], "board.list");
        assert_eq!(sender.session_id(), "s-1");
    }

    #[test]
    fn a_frame_for_an_unknown_session_is_refused() {
        let registry = SessionRegistry::new();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();

        let refused = registry.admit(&client_envelope(&key, "s-unknown", "data"), &carrier);

        assert!(matches!(refused, Err(CarrierError::UnknownSession(id)) if id == "s-unknown"));
    }

    #[test]
    fn a_session_that_ends_is_forgotten_with_its_key() {
        let registry = SessionRegistry::new();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &carrier).unwrap();

        assert_eq!(registry.end("s-1"), vec!["s-1".to_string()]);

        assert!(matches!(
            registry.admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
        assert!(
            registry.end("s-1").is_empty(),
            "a session ends once, however often it is asked to"
        );
    }

    #[test]
    fn a_session_ends_with_the_last_carrier_that_rides_it() {
        let registry = SessionRegistry::new();
        let (relay, _relay_out) = test_carrier();
        let (peer, _peer_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();
        registry.open("s-1", key.clone(), &peer).unwrap();

        assert!(
            registry.release_session("s-1", relay.id()).is_empty(),
            "one carrier gone is not the session gone"
        );
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .is_ok());

        assert_eq!(
            registry.release_session("s-1", peer.id()),
            vec!["s-1".to_string()],
            "the last carrier takes the session with it"
        );
    }

    #[test]
    fn a_carrier_gone_ends_only_the_sessions_that_rode_it_alone() {
        let registry = SessionRegistry::new();
        let (relay, _relay_out) = test_carrier();
        let (peer, _peer_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-alone", key.clone(), &relay).unwrap();
        registry.open("s-shared", key.clone(), &relay).unwrap();
        registry.open("s-shared", key.clone(), &peer).unwrap();

        assert_eq!(
            registry.release_carrier(relay.id()),
            vec!["s-alone".to_string()]
        );
        assert!(registry
            .admit(&client_envelope(&key, "s-shared", "data"), &peer)
            .is_ok());
    }

    #[test]
    fn a_frame_arriving_on_a_new_carrier_records_the_ride() {
        let registry = SessionRegistry::new();
        let (relay, _relay_out) = test_carrier();
        let (peer, _peer_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();

        registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .expect("any carrier may deliver a frame for a known session");

        assert!(
            registry.release_session("s-1", relay.id()).is_empty(),
            "the session rides the carrier its frame arrived on"
        );
    }

    #[test]
    fn reopening_a_session_with_its_own_key_re_attaches_the_new_carrier() {
        let registry = SessionRegistry::new();
        let (first, _first_out) = test_carrier();
        let (second, _second_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &first).unwrap();

        registry
            .open("s-1", key.clone(), &second)
            .expect("the same session on a second carrier is a re-attach");

        assert!(registry.release_carrier(first.id()).is_empty());
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &second)
            .is_ok());
    }

    #[test]
    fn reopening_a_session_under_a_different_key_is_refused() {
        let registry = SessionRegistry::new();
        let (first, _first_out) = test_carrier();
        let (second, _second_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &first).unwrap();

        let refused = registry.open("s-1", transport::generate_session_key(), &second);

        assert!(matches!(refused, Err(CarrierError::KeyMismatch(id)) if id == "s-1"));
        assert!(
            registry
                .admit(&client_envelope(&key, "s-1", "data"), &first)
                .is_ok(),
            "the session keeps the key it was opened with"
        );
    }

    #[test]
    fn a_frame_that_does_not_decrypt_records_no_ride() {
        let registry = SessionRegistry::new();
        let (relay, _relay_out) = test_carrier();
        let (forged, _forged_out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &relay).unwrap();

        let refused = registry.admit(
            &client_envelope(&transport::generate_session_key(), "s-1", "data"),
            &forged,
        );

        assert!(matches!(refused, Err(CarrierError::Transport(_))));
        assert_eq!(
            registry.release_session("s-1", relay.id()),
            vec!["s-1".to_string()],
            "a frame that proves no key possession puts the session on no carrier"
        );
    }

    #[test]
    fn a_sender_the_registry_builds_pushes_to_the_admitting_carrier() {
        let registry = SessionRegistry::new();
        let (carrier, mut out) = test_carrier();
        let key = transport::generate_session_key();
        registry.open("s-1", key.clone(), &carrier).unwrap();

        let (_frame, sender) = registry
            .admit(&client_envelope(&key, "s-1", "data"), &carrier)
            .unwrap();
        assert!(sender.push(json!({ "id": 1, "ok": true })));

        let outbound = out.try_recv().expect("the push rode the carrier");
        assert_eq!(
            SessionSender::decrypt_push(&key, &outbound),
            json!({ "id": 1, "ok": true })
        );
    }
}

#[cfg(test)]
mod intake_tests {
    use super::registry_tests::*;
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    /// The intake's effect side: a session that ended gets its synthetic `close`
    /// frame, whoever reported the end.
    fn watching_intake() -> (
        Arc<FrameIntake>,
        Arc<SessionRegistry>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (seen, closes) = mpsc::unbounded_channel();
        let handler: FrameHandler = Arc::new(move |sender, frame| {
            let _ = seen.send(format!("{}:{}", frame.frame_type, sender.session_id()));
            json!({ "ok": true })
        });
        let registry = Arc::new(SessionRegistry::new());
        (
            FrameIntake::new(registry.clone(), handler),
            registry,
            closes,
        )
    }

    async fn next_seen(closes: &mut mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(Duration::from_secs(5), closes.recv())
            .await
            .expect("the handler ran in time")
            .expect("the channel is open")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_close_frame_ends_the_session_outright() {
        let (intake, registry, mut seen) = watching_intake();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake
            .accept(client_envelope(&key, "s-1", "close"), &carrier)
            .await
            .expect("a close frame is accepted");

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
        assert!(matches!(
            registry.admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_carrier_gone_closes_the_sessions_that_ended_with_it() {
        let (intake, _registry, mut seen) = watching_intake();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake.close_carrier(carrier.id());

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_ends_a_session_riding_only_that_carrier() {
        let (intake, registry, mut seen) = watching_intake();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &carrier).unwrap();

        intake.close_session("s-1", carrier.id());

        assert_eq!(next_seen(&mut seen).await, "close:s-1");
        assert!(matches!(
            registry.admit(&client_envelope(&key, "s-1", "data"), &carrier),
            Err(CarrierError::UnknownSession(_))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_session_closed_leaves_a_session_a_second_carrier_still_rides() {
        let (intake, registry, mut seen) = watching_intake();
        let (relay, _relay_out) = test_carrier();
        let (peer, _peer_out) = test_carrier();
        let key = transport::generate_session_key();
        intake.open("s-1", key.clone(), &relay).unwrap();
        intake.open("s-1", key.clone(), &peer).unwrap();

        intake.close_session("s-1", relay.id());

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            seen.try_recv().is_err(),
            "the session is still carried, so nothing closed"
        );
        assert!(registry
            .admit(&client_envelope(&key, "s-1", "data"), &peer)
            .is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frame_for_an_unknown_session_reaches_no_handler() {
        let (intake, _registry, mut seen) = watching_intake();
        let (carrier, _out) = test_carrier();
        let key = transport::generate_session_key();

        let refused = intake
            .accept(client_envelope(&key, "s-unknown", "data"), &carrier)
            .await;

        assert!(matches!(refused, Err(CarrierError::UnknownSession(_))));
        assert!(seen.try_recv().is_err(), "nothing was dispatched");
    }
}

#[cfg(test)]
mod dispatcher_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc as blocking_channel;
    use std::sync::Mutex;
    use std::time::Duration;

    fn request(id: u64, method: &str, params: Value) -> Frame {
        Frame {
            session_id: "s".into(),
            message_id: format!("m-{id}"),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: String::new(),
            payload: json!({ "id": id, "method": method, "params": params }),
        }
    }

    /// Wait for the next push on an observable session, decoded.
    async fn next_push(
        rx: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
        key: &str,
        within: Duration,
    ) -> Value {
        let outbound = tokio::time::timeout(within, rx.recv())
            .await
            .expect("a response arrives in time")
            .expect("the session channel is open");
        SessionSender::decrypt_push(key, &outbound)
    }

    /// How long a test waits for something it expects to happen. Generous on
    /// purpose: these tests assert an order of events, never a speed, so the
    /// deadline only has to outlast a loaded machine.
    const PATIENTLY: Duration = Duration::from_secs(10);

    /// The test half of a handler gate. A gated handler announces that it is
    /// running and then blocks until [`HandlerGate::release`] lets it go, so a
    /// test can say "the other frame overtook this one" as a fact about order
    /// rather than a bet on wall-clock time.
    struct HandlerGate {
        held: blocking_channel::Receiver<()>,
        release: blocking_channel::Sender<()>,
    }

    /// The handler half of a gate, captured by the handler closure.
    struct GatedHandler {
        held: blocking_channel::SyncSender<()>,
        release: Mutex<blocking_channel::Receiver<()>>,
    }

    impl HandlerGate {
        fn new() -> (Self, GatedHandler) {
            let (held_tx, held_rx) = blocking_channel::sync_channel(1);
            let (release_tx, release_rx) = blocking_channel::channel();
            (
                HandlerGate {
                    held: held_rx,
                    release: release_tx,
                },
                GatedHandler {
                    held: held_tx,
                    release: Mutex::new(release_rx),
                },
            )
        }

        /// Block until the gated handler is running. Fails the test rather than
        /// hanging it if the dispatcher never gets there.
        fn wait_until_held(&self) {
            self.held
                .recv_timeout(PATIENTLY)
                .expect("the gated handler is running");
        }

        fn release(&self) {
            self.release
                .send(())
                .expect("the gated handler is waiting to be released");
        }
    }

    impl GatedHandler {
        /// Occupy this worker until the test releases us.
        fn hold(&self) {
            self.held.send(()).expect("the test is still watching");
            // Only ever one frame is held at a time, so blocking while holding
            // the lock is the point: the worker running us stays occupied.
            let _ = self.release.lock().unwrap().recv();
        }
    }

    /// Teardown bookkeeping is fast by construction: a lane whose handler is
    /// wedged fills up, and a carrier ending must not park behind it — the relay
    /// reconnect that follows is what would never happen.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_close_does_not_wait_behind_a_saturated_lane() {
        let (gate, gated) = HandlerGate::new();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            if frame.payload["id"] == 0 {
                gated.hold();
            }
            json!({ "ok": true })
        });
        let dispatcher = Arc::new(Dispatcher::with_capacity(handler, 16, 4));
        let (sender, _rx, _key) = SessionSender::observable("s-wedged");

        let dispatched = Arc::new(AtomicUsize::new(0));
        let feeding = tokio::spawn({
            let dispatcher = dispatcher.clone();
            let dispatched = dispatched.clone();
            async move {
                for id in 0..(LANE_QUEUE_DEPTH as u64 + 8) {
                    dispatcher
                        .dispatch(
                            sender.clone(),
                            request(id, "term.input", json!({ "term_id": "term-1" })),
                        )
                        .await;
                    dispatched.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        gate.wait_until_held();
        tokio::time::timeout(PATIENTLY, async {
            while dispatched.load(Ordering::SeqCst) <= LANE_QUEUE_DEPTH {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the wedged lane fills up");

        tokio::time::timeout(PATIENTLY, async { dispatcher.close_session("s-wedged") })
            .await
            .expect("the close does not wait on the lane it is tearing down");

        gate.release();
        feeding.abort();
    }

    /// The incident in one test: a handler that takes seconds must not keep the
    /// next frame from being handled. Frames are dispatched from one task (as the
    /// read loop does), so a stall here is a stall of the socket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_handler_does_not_hold_up_the_next_frame() {
        let (gate, gated) = HandlerGate::new();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            if frame.payload["method"] == "slow" {
                gated.hold();
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 8, 4);

        let (slow_sender, mut slow_rx, slow_key) = SessionSender::observable("s-slow");
        let (fast_sender, mut fast_rx, fast_key) = SessionSender::observable("s-fast");

        dispatcher
            .dispatch(slow_sender, request(1, "slow", json!({})))
            .await;
        dispatcher
            .dispatch(fast_sender, request(2, "fast", json!({})))
            .await;
        gate.wait_until_held();

        let fast = next_push(&mut fast_rx, &fast_key, PATIENTLY).await;
        assert_eq!(
            fast["id"], 2,
            "the fast frame answered while the slow one ran"
        );
        assert!(
            slow_rx.try_recv().is_err(),
            "the slow handler is still running — this is what the fast frame overtook"
        );

        gate.release();
        let slow = next_push(&mut slow_rx, &slow_key, PATIENTLY).await;
        assert_eq!(slow["id"], 1, "the slow frame still gets its answer");
    }

    /// Terminal input and acks are a stream: their order is the client's meaning.
    /// Frames carrying the same `term_id` run one at a time, in arrival order,
    /// even while independent frames run beside them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn frames_for_one_terminal_keep_their_order() {
        let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let (gate, gated) = HandlerGate::new();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            let id = frame.payload["id"].as_u64().unwrap_or_default();
            // The first frame is the held one: without a serial lane the ones
            // behind it would finish first and record out of order.
            if id == 0 {
                gated.hold();
            }
            recorder.lock().unwrap().push(id);
            json!({ "id": id, "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 64, 8);

        let (sender, mut rx, key) = SessionSender::observable("s-1");
        for id in 0..12u64 {
            dispatcher
                .dispatch(
                    sender.clone(),
                    request(id, "term.input", json!({ "term_id": "term-1" })),
                )
                .await;
        }
        // A second terminal is a second lane: it answers without waiting for the
        // held frame ahead of it on the first.
        let (other, mut other_rx, other_key) = SessionSender::observable("s-1");
        dispatcher
            .dispatch(
                other,
                request(99, "term.input", json!({ "term_id": "term-2" })),
            )
            .await;
        gate.wait_until_held();

        let overtaking = next_push(&mut other_rx, &other_key, PATIENTLY).await;
        assert_eq!(
            overtaking["id"], 99,
            "a different terminal runs concurrently"
        );
        assert!(
            rx.try_recv().is_err(),
            "nothing on the first terminal answered past the frame that is still running"
        );

        gate.release();
        for id in 0..12u64 {
            let answered = next_push(&mut rx, &key, PATIENTLY).await;
            assert_eq!(answered["id"], id, "responses come back in order");
        }
        let on_the_lane: Vec<u64> = seen
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|id| *id != 99)
            .collect();
        assert_eq!(
            on_the_lane,
            (0..12).collect::<Vec<u64>>(),
            "handlers ran in arrival order"
        );
    }

    /// `term.input` and `term.ack` are the pairing the protocol names: an ack the
    /// bridge handles before the input it acknowledges is flow control run
    /// backwards. Interleaved on one terminal, the two methods share a lane and
    /// run in arrival order — even with a slow frame at the head and idle workers
    /// that would happily run the acks early.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn input_and_ack_on_one_terminal_stay_in_arrival_order() {
        let seen: Arc<Mutex<Vec<(u64, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            let id = frame.payload["id"].as_u64().unwrap_or_default();
            let method = frame.payload["method"].as_str().unwrap_or_default();
            // The input at the head is slow: were the acks behind it dispatched
            // to the shared pool, the idle workers would record them first.
            if id == 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            recorder.lock().unwrap().push((id, method.to_string()));
            json!({ "id": id, "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 64, 8);

        let (sender, mut rx, key) = SessionSender::observable("s-1");
        let sent: Vec<(u64, String)> = (0..12u64)
            .map(|id| {
                let method = if id % 2 == 0 {
                    "term.input"
                } else {
                    "term.ack"
                };
                (id, method.to_string())
            })
            .collect();
        for (id, method) in &sent {
            dispatcher
                .dispatch(
                    sender.clone(),
                    request(*id, method, json!({ "term_id": "term-1" })),
                )
                .await;
        }

        for (id, _) in &sent {
            let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
            assert_eq!(answered["id"], *id, "responses come back in arrival order");
        }
        assert_eq!(
            *seen.lock().unwrap(),
            sent,
            "input and ack handlers ran in arrival order on the shared lane"
        );
    }

    /// A flood parks in a bounded queue: once it is full the dispatch call waits
    /// instead of growing memory without end.
    ///
    /// Each frame here asks a different question (`page` differs), so none of them
    /// folds into another — the bound is what is being tested, not the fold.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_full_queue_makes_the_caller_wait() {
        let blocked = Arc::new(AtomicBool::new(true));
        let gate = blocked.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            while gate.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        // One worker, two waiting slots: the fourth frame has nowhere to go.
        let dispatcher = Dispatcher::with_capacity(handler, 2, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-flood");

        for id in 0..3u64 {
            tokio::time::timeout(
                Duration::from_millis(500),
                dispatcher.dispatch(
                    sender.clone(),
                    request(id, "board.list", json!({ "page": id })),
                ),
            )
            .await
            .expect("the worker and the queue take the first three");
        }
        assert!(
            tokio::time::timeout(
                Duration::from_millis(200),
                dispatcher.dispatch(
                    sender.clone(),
                    request(3, "board.list", json!({ "page": 3 }))
                )
            )
            .await
            .is_err(),
            "a full queue holds the caller instead of accepting without limit"
        );

        blocked.store(false, Ordering::SeqCst);
        tokio::time::timeout(
            Duration::from_secs(5),
            dispatcher.dispatch(sender, request(4, "board.list", json!({ "page": 4 }))),
        )
        .await
        .expect("a drained queue takes frames again");
        let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
        assert_eq!(answered["id"], 0, "the queued work still ran");
    }

    /// A fold costs no queue slot, so it must carry its own bound: without one, a
    /// client stuck in a reconnect loop could park unbounded frames in it and get
    /// back the memory growth the bounded queue exists to prevent. Past the cap a
    /// read queues normally, where the queue's own limit holds the caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_fold_stops_growing_and_hands_the_overflow_back_to_the_queue() {
        let gate = Arc::new(AtomicBool::new(true));
        let computed: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let handler = counting_handler(gate.clone(), computed.clone(), started);
        let dispatcher = Dispatcher::with_capacity(handler, 256, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-flood");

        dispatcher
            .dispatch(sender.clone(), request(0, "hold", json!({})))
            .await;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the only worker is busy");

        let overflowing = MAX_FOLDED_READS as u64 + 2;
        for id in 1..=overflowing {
            dispatcher
                .dispatch(sender.clone(), request(id, "board.list", json!({})))
                .await;
        }
        // Read the fold while the worker is still held, but assert after letting
        // it go: a `hold` handler left spinning by a panicking assertion hangs the
        // whole test binary at runtime shutdown instead of failing.
        let folded_now: usize = dispatcher
            .folded_reads
            .lock()
            .unwrap()
            .values()
            .map(|folded| folded.ids.len())
            .sum();
        gate.store(false, Ordering::SeqCst);
        assert_eq!(folded_now, MAX_FOLDED_READS, "the fold stops at its cap");

        let mut ids = answered_ids(&mut rx, &key, overflowing as usize + 1).await;
        ids.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(
            ids,
            (0..=overflowing).map(Value::from).collect::<Vec<Value>>(),
            "the two that overflowed the fold are answered too"
        );
        assert_eq!(
            computed.lock().unwrap().len(),
            3,
            "one compute for the fold, one for each read that overflowed it"
        );
    }

    /// A terminal's lane lives as long as the terminal: the close still runs, and
    /// nothing is left behind for an id that will never be minted again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_closed_terminal_leaves_no_lane_behind() {
        let (ran, mut ran_rx) = mpsc::unbounded_channel::<u64>();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            let _ = ran.send(frame.payload["id"].as_u64().unwrap_or_default());
            json!({ "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 8, 2);
        let (sender, _rx, _key) = SessionSender::observable("s-term");

        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.input", json!({ "term_id": "term-1" })),
            )
            .await;
        assert_eq!(
            dispatcher.lanes.lock().unwrap().len(),
            1,
            "the terminal has a lane"
        );

        dispatcher
            .dispatch(
                sender,
                request(2, "term.close", json!({ "term_id": "term-1" })),
            )
            .await;
        assert!(
            dispatcher.lanes.lock().unwrap().is_empty(),
            "the lane goes with the terminal"
        );

        let mut ran_ids = Vec::new();
        for _ in 0..2 {
            ran_ids.push(
                tokio::time::timeout(Duration::from_secs(5), ran_rx.recv())
                    .await
                    .expect("both frames ran")
                    .expect("the channel is open"),
            );
        }
        assert_eq!(ran_ids, vec![1, 2], "the close ran, after the input");
    }

    /// Handlers run on blocking threads now, and a handler that opens a terminal
    /// spawns its output pump onto the runtime (a pump that fails to start is
    /// silently a terminal with no output). The runtime must still be reachable
    /// from there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_handler_can_still_spawn_onto_the_runtime() {
        let (spawned, mut spawned_rx) = mpsc::unbounded_channel::<&'static str>();
        let handler: FrameHandler = Arc::new(move |_sender, _frame| {
            let reachable = tokio::runtime::Handle::try_current().is_ok();
            if reachable {
                let spawned = spawned.clone();
                tokio::spawn(async move {
                    let _ = spawned.send("pump");
                });
            }
            json!({ "ok": reachable })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 4, 2);

        let (sender, mut rx, key) = SessionSender::observable("s-pump");
        dispatcher
            .dispatch(sender, request(1, "term.create", json!({})))
            .await;

        let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
        assert_eq!(
            answered["ok"], true,
            "the runtime is reachable from a handler"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), spawned_rx.recv())
                .await
                .expect("the spawned task ran"),
            Some("pump")
        );
    }

    /// A handler that blocks until `gate` is cleared, recording every method it
    /// was asked to compute and answering with the caller's id.
    fn counting_handler(
        gate: Arc<AtomicBool>,
        computed: Arc<Mutex<Vec<(String, Value)>>>,
        started: mpsc::UnboundedSender<()>,
    ) -> FrameHandler {
        Arc::new(move |_sender, frame| {
            let method = frame.payload["method"].as_str().unwrap_or("").to_string();
            let params = frame.payload["params"].clone();
            if method == "hold" {
                let _ = started.send(());
                while gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            } else {
                computed.lock().unwrap().push((method, params));
            }
            json!({ "id": frame.payload["id"], "ok": true, "result": { "served": true } })
        })
    }

    /// Collect `count` answers off one session, as ids.
    async fn answered_ids(
        rx: &mut mpsc::UnboundedReceiver<OutboundEnvelope>,
        key: &str,
        count: usize,
    ) -> Vec<Value> {
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(next_push(rx, key, Duration::from_secs(5)).await["id"].clone());
        }
        ids
    }

    /// The tail of the incident: after a stall, hundreds of identical `board.list`
    /// polls sit queued and the old bridge answered every one with a full
    /// recompute. Now the queue holds one of them: it is computed once and every
    /// caller that asked for it gets that answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn identical_queued_reads_are_computed_once_and_answered_to_every_caller() {
        let gate = Arc::new(AtomicBool::new(true));
        let computed: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let handler = counting_handler(gate.clone(), computed.clone(), started);
        // One worker: the hold occupies it, so every read below waits in the queue.
        let dispatcher = Dispatcher::with_capacity(handler, 256, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-poll");

        dispatcher
            .dispatch(sender.clone(), request(0, "hold", json!({})))
            .await;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the only worker is busy before the reads queue behind it");

        for id in 1..=40u64 {
            dispatcher
                .dispatch(sender.clone(), request(id, "board.list", json!({})))
                .await;
        }
        gate.store(false, Ordering::SeqCst);

        let mut ids = answered_ids(&mut rx, &key, 41).await;
        ids.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(
            ids,
            (0..=40u64).map(Value::from).collect::<Vec<Value>>(),
            "every poller gets an answer — a dropped frame is an RPC that times out in the browser"
        );
        assert_eq!(
            *computed.lock().unwrap(),
            vec![("board.list".to_string(), json!({}))],
            "forty identical polls cost one recompute"
        );
    }

    /// Coalescing folds together only requests whose answer is the same answer:
    /// same client, same method, same params. Anything else is its own compute.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn reads_are_folded_together_only_when_client_method_and_params_all_match() {
        let gate = Arc::new(AtomicBool::new(true));
        let computed: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let handler = counting_handler(gate.clone(), computed.clone(), started);
        let dispatcher = Dispatcher::with_capacity(handler, 256, 1);
        let (one, mut one_rx, one_key) = SessionSender::observable("s-one");
        let (two, mut two_rx, two_key) = SessionSender::observable("s-two");

        dispatcher
            .dispatch(one.clone(), request(0, "hold", json!({})))
            .await;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the only worker is busy");

        for id in 1..=3u64 {
            // Same client, same params: one compute.
            dispatcher
                .dispatch(one.clone(), request(id, "git.log", json!({ "p": "a" })))
                .await;
            // Same client, different params: a different answer, its own compute.
            dispatcher
                .dispatch(
                    one.clone(),
                    request(id + 10, "git.log", json!({ "p": "b" })),
                )
                .await;
            // A different client asking the same thing: its own compute, because a
            // client's answer is pushed into its own session.
            dispatcher
                .dispatch(
                    two.clone(),
                    request(id + 20, "git.log", json!({ "p": "a" })),
                )
                .await;
        }
        gate.store(false, Ordering::SeqCst);

        let mut on_one = answered_ids(&mut one_rx, &one_key, 7).await;
        on_one.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(
            on_one,
            vec![0u64, 1, 2, 3, 11, 12, 13]
                .into_iter()
                .map(Value::from)
                .collect::<Vec<Value>>()
        );
        let mut on_two = answered_ids(&mut two_rx, &two_key, 3).await;
        on_two.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(
            on_two,
            vec![21u64, 22, 23]
                .into_iter()
                .map(Value::from)
                .collect::<Vec<Value>>()
        );

        let mut ran: Vec<String> = computed
            .lock()
            .unwrap()
            .iter()
            .map(|(method, params)| format!("{method} {params}"))
            .collect();
        ran.sort();
        assert_eq!(
            ran,
            vec![
                "git.log {\"p\":\"a\"}".to_string(),
                "git.log {\"p\":\"a\"}".to_string(),
                "git.log {\"p\":\"b\"}".to_string(),
            ],
            "three distinct answers, three computes — one per (client, method, params)"
        );
    }

    /// A verb that changes something means what it says every time it is said.
    /// Two `git.commit` frames are two commits; folding them would silently drop
    /// work the human asked for.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_verb_that_changes_something_is_never_folded() {
        let gate = Arc::new(AtomicBool::new(true));
        let computed: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let handler = counting_handler(gate.clone(), computed.clone(), started);
        let dispatcher = Dispatcher::with_capacity(handler, 256, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-writer");

        dispatcher
            .dispatch(sender.clone(), request(0, "hold", json!({})))
            .await;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the only worker is busy");

        for id in 1..=5u64 {
            dispatcher
                .dispatch(
                    sender.clone(),
                    request(id, "git.commit", json!({ "message": "same" })),
                )
                .await;
        }
        gate.store(false, Ordering::SeqCst);

        let _ = answered_ids(&mut rx, &key, 6).await;
        assert_eq!(
            computed.lock().unwrap().len(),
            5,
            "every commit runs — a write is never folded into the one before it"
        );
    }

    /// A session's close is its last word: it runs after the frames that arrived
    /// before it, so an attach can never register into an already-closed session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_close_waits_for_the_frames_ahead_of_it() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            if frame.frame_type == "close" {
                recorder.lock().unwrap().push("close".into());
                return json!({ "ok": true });
            }
            std::thread::sleep(Duration::from_millis(200));
            recorder
                .lock()
                .unwrap()
                .push(frame.payload["id"].to_string());
            json!({ "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 16, 4);

        let (sender, _rx, _key) = SessionSender::observable("s-closing");
        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.attach", json!({ "term_id": "term-1" })),
            )
            .await;
        dispatcher.close_session("s-closing");

        for _ in 0..100 {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["1".to_string(), "close".to_string()],
            "the close ran last"
        );
    }
}
