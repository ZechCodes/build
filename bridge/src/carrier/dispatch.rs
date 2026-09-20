//! How a decrypted frame reaches its handler: the worker pool, the ordered
//! terminal lanes, and the fold that answers identical queued reads once.
//!
//! Scheduling only. What a session is, who may push to it and when it ends are
//! the carrier boundary's questions, one module up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

use super::SessionSender;
use crate::timing::{FrameClock, FrameTimer, Priority, QueuedFrame};
use crate::transport::{Frame, CLOSE_FRAME_TYPE, SENDER_DEVICE};

/// A frame's dispatcher and the clock shared by its queue and app-lock phases.
#[derive(Clone)]
pub struct FrameHandler {
    pub(super) clock: Arc<FrameClock>,
    pub(super) dispatch: Arc<dyn Fn(SessionSender, Frame, FrameTimer) -> Value + Send + Sync>,
}

impl FrameHandler {
    pub fn new(
        clock: Arc<FrameClock>,
        dispatch: impl Fn(SessionSender, Frame, FrameTimer) -> Value + Send + Sync + 'static,
    ) -> Self {
        Self {
            clock,
            dispatch: Arc::new(dispatch),
        }
    }

    /// Run a frame that never waited in the dispatcher, including session close.
    pub fn call(&self, sender: SessionSender, frame: Frame) -> Value {
        self.run(self.clock.queued(), sender, frame)
    }

    fn run(&self, queued: QueuedFrame, sender: SessionSender, frame: Frame) -> Value {
        let method = frame
            .payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or(&frame.frame_type);
        let timer = queued.start(method);
        (self.dispatch)(sender, frame, timer)
    }
}

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

/// How many of the workers may be running background frames at once. The
/// background tier pulls a whole feed — every worktree's status, every file
/// tree — and it does so on a timer nobody is watching, so it gets two workers
/// and no more: six are always there for the surface the human is looking at,
/// however deep the background queue is. Two rather than one because the tier's
/// own work is mostly waiting on git, and one slow worktree must not stop the
/// tier from ever reaching the next one.
const BACKGROUND_WORKERS: usize = 2;

/// The envelope field naming a queue, and the one value that is not the
/// default. Absent, unknown, or not a string all read as foreground: the field
/// is the client's claim about its own urgency, and the safe reading of a claim
/// this bridge does not recognise is "somebody is waiting for this".
const PRIORITY_FIELD: &str = "priority";
const BACKGROUND_PRIORITY: &str = "background";

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
    queued: QueuedFrame,
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
    /// The queue whose marker is expected to take this fold. It only ever moves
    /// one way — background to foreground, when a foreground caller joins — so
    /// nobody folded into a background pull inherits its wait.
    priority: Priority,
    /// The request id of every folded frame, in arrival order. Each one is
    /// answered — see [`Dispatcher`] on why none of them may simply be dropped.
    ids: Vec<Value>,
    queued: QueuedFrame,
}

impl FoldedRead {
    /// Add one more caller to this fold, and answer it from the newest frame of
    /// the set — so the one answer postdates the last question it answers.
    ///
    /// A foreground caller joining a background fold takes the fold with it:
    /// nobody the human is waiting on queues behind the background tier.
    fn join(
        &mut self,
        id: Value,
        sender: SessionSender,
        frame: Frame,
        priority: Priority,
    ) -> Folded {
        self.ids.push(id);
        self.sender = sender;
        self.frame = frame;
        if priority == Priority::Foreground && self.priority == Priority::Background {
            self.priority = Priority::Foreground;
            self.queued.promote();
            return Folded::Promoted;
        }
        Folded::Joined
    }
}

/// What became of a read offered to the fold.
enum Folded {
    /// It joined a fold already waiting; that fold's one answer covers it.
    Joined,
    /// It joined a background fold as a foreground caller: the fold moves to the
    /// foreground queue, which costs one more marker there. Whichever marker a
    /// worker takes first runs the read; the other finds the fold gone and does
    /// nothing, exactly as a marker for an already-computed read always has.
    Promoted,
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
    /// Boxed: a job is a sender and a frame, and the fence beside it is a
    /// oneshot, so the enum would otherwise be the size of its largest arm.
    Run(Box<Job>),
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
/// The pool takes from two queues, not one. A frame whose envelope says
/// `"priority": "background"` — the tier that keeps the unfocused workspaces
/// warm — joins the background queue, and a free worker takes foreground first,
/// so the surface a human is looking at never queues behind a feed-wide pull.
/// At most [`BACKGROUND_WORKERS`] of the pool run background frames at once.
/// The fold crosses the two: a background read that matches a queued foreground
/// one is answered by it, and a foreground read that joins a background one
/// takes the fold to the foreground queue rather than inheriting its wait.
///
/// A read that arrives after its twin has started running is not folded into it —
/// it gets its own compute. The client asked at a moment the running answer
/// predates, and serving a snapshot older than the question is how a board goes
/// stale and stays stale.
pub(super) struct Dispatcher {
    handler: FrameHandler,
    /// The two shared pool queues, foreground and background. Both bounded: a
    /// full queue makes the read loop wait, and it waits per queue, so a flood
    /// of background pulls cannot fill the slot a foreground frame needs.
    jobs: [mpsc::Sender<QueuedWork>; 2],
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
    pub(super) fn new(handler: FrameHandler) -> Self {
        Self::with_capacity(handler, DISPATCH_QUEUE_DEPTH, DISPATCH_WORKERS)
    }

    pub(super) fn with_capacity(handler: FrameHandler, queue_depth: usize, workers: usize) -> Self {
        let depth = queue_depth.max(1);
        let (foreground, foreground_rx) = mpsc::channel::<QueuedWork>(depth);
        let (background, background_rx) = mpsc::channel::<QueuedWork>(depth);
        // Two queues, many workers: whoever is free takes the next foreground
        // frame, and only a worker holding one of the background permits may
        // take a background one.
        let queues = Arc::new(Queues {
            foreground: tokio::sync::Mutex::new(foreground_rx),
            background: tokio::sync::Mutex::new(background_rx),
            background_slots: Arc::new(Semaphore::new(BACKGROUND_WORKERS)),
        });
        let folded_reads: Arc<Mutex<HashMap<ReadKey, FoldedRead>>> =
            Arc::new(Mutex::new(HashMap::new()));
        for _ in 0..workers.max(1) {
            tokio::spawn(work_loop(
                handler.clone(),
                Arc::clone(&queues),
                Arc::clone(&folded_reads),
            ));
        }
        Dispatcher {
            handler,
            jobs: [foreground, background],
            lanes: Mutex::new(HashMap::new()),
            folded_reads,
        }
    }

    /// The handler these queues feed, for the frames the intake answers
    /// without queueing at all (`FrameIntake::accept`).
    pub(super) fn handler(&self) -> FrameHandler {
        self.handler.clone()
    }

    /// The queue a frame of this priority joins.
    fn queue(&self, priority: Priority) -> &mpsc::Sender<QueuedWork> {
        match priority {
            Priority::Foreground => &self.jobs[0],
            Priority::Background => &self.jobs[1],
        }
    }

    /// Hand one decrypted request frame to a worker. Waits only when every
    /// worker is busy and the queue is full.
    pub(super) async fn dispatch(&self, sender: SessionSender, frame: Frame) {
        match ordered_lane(&sender, &frame) {
            // A terminal's frames are scheduled by its lane and by nothing else:
            // its stream is the client's own typing, never a background pull, and
            // a `priority` on one of them means nothing here.
            Some(key) => {
                let queued = self.handler.clock.queued();
                self.dispatch_in_order(key, sender, frame, queued).await;
            }
            None => {
                let priority = frame_priority(&frame);
                let queued = self.handler.clock.queued_at(priority);
                self.dispatch_to_pool(priority, sender, frame, queued).await;
            }
        }
    }

    /// Queue a terminal's frame on its serial lane. A terminal id is minted
    /// once and never reused, so its close is the last frame its lane can
    /// carry: letting the lane's sender go ends the lane once that close has
    /// run.
    async fn dispatch_in_order(
        &self,
        key: (String, String),
        sender: SessionSender,
        frame: Frame,
        queued: QueuedFrame,
    ) {
        let closing =
            frame.payload.get("method").and_then(Value::as_str) == Some(TERMINAL_CLOSE_METHOD);
        let _ = self
            .lane(key.clone())
            .send(LaneMessage::Run(Box::new(Job {
                sender,
                frame,
                queued,
            })))
            .await;
        if closing {
            self.lanes.lock().unwrap().remove(&key);
        }
    }

    /// Queue an independent frame for whichever worker is free, joining an
    /// identical read already waiting if there is one. A fold that heads the
    /// queue and finds no worker left to take it is unparked again, so no frame
    /// waits in a map nothing will ever drain.
    async fn dispatch_to_pool(
        &self,
        priority: Priority,
        sender: SessionSender,
        frame: Frame,
        queued: QueuedFrame,
    ) {
        let job = |sender, frame, queued| {
            QueuedWork::Frame(Job {
                sender,
                frame,
                queued,
            })
        };
        let (queue, work) = match read_key(&sender, &frame) {
            None => (priority, job(sender, frame, queued)),
            Some(key) => {
                match self.fold_into_queued_read(key.clone(), priority, sender, frame, queued) {
                    Folded::Joined => return,
                    Folded::Heads => (priority, QueuedWork::FoldedRead(key)),
                    Folded::Promoted => (Priority::Foreground, QueuedWork::FoldedRead(key)),
                    Folded::Overflowed(job) => (priority, QueuedWork::Frame(*job)),
                }
            }
        };
        if let Err(mpsc::error::SendError(QueuedWork::FoldedRead(key))) =
            self.queue(queue).send(work).await
        {
            self.folded_reads.lock().unwrap().remove(&key);
        }
    }

    /// Join a read to an identical one already waiting for a worker, if there is
    /// one and it has room.
    fn fold_into_queued_read(
        &self,
        key: ReadKey,
        priority: Priority,
        sender: SessionSender,
        frame: Frame,
        queued: QueuedFrame,
    ) -> Folded {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let mut folded_reads = self.folded_reads.lock().unwrap();
        match folded_reads.get_mut(&key) {
            Some(waiting) if waiting.ids.len() < MAX_FOLDED_READS => {
                waiting.join(id, sender, frame, priority)
            }
            // A full fold: this read takes a queue slot of its own, which is what
            // puts the caller back under the queue's bound.
            Some(_) => Folded::Overflowed(Box::new(Job {
                sender,
                frame,
                queued,
            })),
            None => {
                folded_reads.insert(
                    key,
                    FoldedRead {
                        sender,
                        frame,
                        priority,
                        ids: vec![id],
                        queued,
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
            .or_insert_with(|| spawn_lane(self.handler.clone()))
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
    ///
    /// The frame goes out only if `still_ended` says so once the lanes have
    /// drained: the same id may have been opened again in the meantime, and
    /// what that session holds is not this end's to release.
    pub(super) fn close_session(
        &self,
        session_id: &str,
        still_ended: impl FnOnce() -> bool + Send + 'static,
    ) {
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
            if !still_ended() {
                return;
            }
            let closed = Frame {
                session_id: session_id.clone(),
                message_id: String::new(),
                frame_type: CLOSE_FRAME_TYPE.into(),
                sender: SENDER_DEVICE.into(),
                created_at: String::new(),
                payload: Value::Null,
            };
            let sender = SessionSender::detached(&session_id);
            // No response: nobody is left to read one.
            let _ = tokio::task::spawn_blocking(move || handler.call(sender, closed)).await;
        });
    }
}

/// The two pool queues and the permits that bound the background one.
struct Queues {
    foreground: tokio::sync::Mutex<mpsc::Receiver<QueuedWork>>,
    background: tokio::sync::Mutex<mpsc::Receiver<QueuedWork>>,
    /// One permit per background frame that may be running. A worker holds its
    /// permit for as long as it runs the frame it took, so the pool never has
    /// more than [`BACKGROUND_WORKERS`] of them in flight.
    background_slots: Arc<Semaphore>,
}

/// One worker: take work, run it, repeat, until both queues are closed.
async fn work_loop(
    handler: FrameHandler,
    queues: Arc<Queues>,
    folded_reads: Arc<Mutex<HashMap<ReadKey, FoldedRead>>>,
) {
    // The permit, when there is one, lives exactly as long as the background
    // frame it admitted.
    while let Some((work, _permit)) = next_work(&queues).await {
        run_work(&handler, &folded_reads, work).await;
    }
}

/// The next frame this worker should run, and the background permit it needed
/// to take it.
///
/// Foreground first, always: the branches are biased, so a worker offered both
/// takes the frame somebody is waiting on. The background branch takes a permit
/// before it looks at its queue, which is what keeps six workers free — a worker
/// that cannot get a permit simply waits on foreground alone. Both branches are
/// cancel-safe (`recv` and `acquire_owned` both are), so the branch that loses
/// leaves neither a frame nor a permit behind.
async fn next_work(queues: &Arc<Queues>) -> Option<(QueuedWork, Option<OwnedSemaphorePermit>)> {
    tokio::select! {
        biased;
        work = next_of(&queues.foreground) => work.map(|work| (work, None)),
        taken = next_background(queues) => taken,
    }
}

async fn next_of(queue: &tokio::sync::Mutex<mpsc::Receiver<QueuedWork>>) -> Option<QueuedWork> {
    queue.lock().await.recv().await
}

async fn next_background(
    queues: &Arc<Queues>,
) -> Option<(QueuedWork, Option<OwnedSemaphorePermit>)> {
    let permit = Arc::clone(&queues.background_slots)
        .acquire_owned()
        .await
        .ok()?;
    Some((next_of(&queues.background).await?, Some(permit)))
}

/// Run one thing off a queue: a frame, or the marker standing for a fold.
async fn run_work(
    handler: &FrameHandler,
    folded_reads: &Mutex<HashMap<ReadKey, FoldedRead>>,
    work: QueuedWork,
) {
    match work {
        QueuedWork::Frame(job) => run_job(handler, job).await,
        QueuedWork::FoldedRead(key) => {
            // Taking the entry out is what closes the fold: from here on, the
            // same read queues afresh behind us. A marker whose fold is already
            // gone — the second marker of a promoted fold — has nothing to run.
            let folded = folded_reads.lock().unwrap().remove(&key);
            if let Some(folded) = folded {
                run_folded_read(handler, folded).await;
            }
        }
    }
}

/// One serial lane: runs what it is sent, one at a time, in arrival order,
/// until its last sender is dropped.
fn spawn_lane(handler: FrameHandler) -> mpsc::Sender<LaneMessage> {
    let (tx, mut rx) = mpsc::channel::<LaneMessage>(LANE_QUEUE_DEPTH);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            match message {
                LaneMessage::Run(job) => run_job(&handler, *job).await,
                LaneMessage::Fence(reply) => {
                    let _ = reply.send(());
                }
            }
        }
    });
    tx
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

/// The queue this frame asked for. See [`PRIORITY_FIELD`]: only the exact word
/// [`BACKGROUND_PRIORITY`] moves a frame out of the foreground queue.
fn frame_priority(frame: &Frame) -> Priority {
    match frame.payload.get(PRIORITY_FIELD).and_then(Value::as_str) {
        Some(BACKGROUND_PRIORITY) => Priority::Background,
        _ => Priority::Foreground,
    }
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
    let FoldedRead {
        sender,
        frame,
        ids,
        queued,
        priority: _,
    } = folded;
    let answer = match run_handler(handler, &sender, frame, queued).await {
        None => return,
        Some(Ok(payload)) => payload,
        Some(Err(_)) => json!({ "ok": false, "error": "handler failed" }),
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

/// Run one frame's handler and push its answer. A handler that panicked (or a
/// runtime shutting down) is answered too: a client that never hears back
/// waits forever.
async fn run_job(handler: &FrameHandler, job: Job) {
    let Job {
        sender,
        frame,
        queued,
    } = job;
    let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
    match run_handler(handler, &sender, frame, queued).await {
        None => {}
        Some(Ok(payload)) => {
            sender.push(payload);
        }
        Some(Err(_)) => {
            sender.push(json!({ "id": id, "ok": false, "error": "handler failed" }));
        }
    }
}

/// The one place a handler is invoked, and so the one place this rule is
/// written: **nothing runs for a session after its close**. A frame admitted
/// while its session was open and taken off a lane, the pool or a fold after
/// that session ended is dropped here, whatever it asked — the close was the
/// session's last frame, and a `session.hello` or a `term.attach` run behind
/// it would register the session into a bus or a terminal it has left.
///
/// `None` is that refusal. Otherwise the handler's answer, or the join error of
/// a handler that panicked.
///
/// Blocking, not async: a handler takes the app mutex and may sit in libgit2
/// for seconds. On a runtime worker that would block the read loop and the
/// writer with it — the very thing the queue exists to prevent.
async fn run_handler(
    handler: &FrameHandler,
    sender: &SessionSender,
    frame: Frame,
    queued: QueuedFrame,
) -> Option<Result<Value, tokio::task::JoinError>> {
    if !sender.session_is_open() {
        return None;
    }
    let handler = handler.clone();
    let answering = sender.clone();
    Some(tokio::task::spawn_blocking(move || handler.run(queued, answering, frame)).await)
}

#[cfg(test)]
mod dispatcher_tests {
    use super::*;
    use crate::carrier::OutboundEnvelope;
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
    /// reconnect that follows is what would never happen. The close itself still
    /// arrives, once the lane has drained.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_close_does_not_wait_behind_a_saturated_lane() {
        let (gate, gated) = HandlerGate::new();
        let closed = Arc::new(AtomicBool::new(false));
        let handler: FrameHandler = {
            let closed = closed.clone();
            FrameHandler::new(
                crate::timing::FrameClock::new(),
                move |_sender, frame, _timer| {
                    if frame.frame_type == CLOSE_FRAME_TYPE {
                        closed.store(true, Ordering::SeqCst);
                    } else if frame.payload["id"] == 0 {
                        gated.hold();
                    }
                    json!({ "ok": true })
                },
            )
        };
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

        let closing = dispatcher.clone();
        tokio::time::timeout(
            PATIENTLY,
            tokio::task::spawn_blocking(move || closing.close_session("s-wedged", || true)),
        )
        .await
        .expect("the close does not wait on the lane it is tearing down")
        .expect("the closing thread finished");

        gate.release();
        tokio::time::timeout(PATIENTLY, async {
            while !closed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the session's close reached the handler once the lane drained");
        feeding.abort();
    }

    /// The incident in one test: a handler that takes seconds must not keep the
    /// next frame from being handled. Frames are dispatched from one task (as the
    /// read loop does), so a stall here is a stall of the socket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_handler_does_not_hold_up_the_next_frame() {
        let (gate, gated) = HandlerGate::new();
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                if frame.payload["method"] == "slow" {
                    gated.hold();
                }
                json!({ "id": frame.payload["id"], "ok": true })
            },
        );
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                let id = frame.payload["id"].as_u64().unwrap_or_default();
                // The first frame is the held one: without a serial lane the ones
                // behind it would finish first and record out of order.
                if id == 0 {
                    gated.hold();
                }
                recorder.lock().unwrap().push(id);
                json!({ "id": id, "ok": true })
            },
        );
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                let id = frame.payload["id"].as_u64().unwrap_or_default();
                let method = frame.payload["method"].as_str().unwrap_or_default();
                // The input at the head is slow: were the acks behind it dispatched
                // to the shared pool, the idle workers would record them first.
                if id == 0 {
                    std::thread::sleep(Duration::from_millis(300));
                }
                recorder.lock().unwrap().push((id, method.to_string()));
                json!({ "id": id, "ok": true })
            },
        );
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                while gate.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                json!({ "id": frame.payload["id"], "ok": true })
            },
        );
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                let _ = ran.send(frame.payload["id"].as_u64().unwrap_or_default());
                json!({ "ok": true })
            },
        );
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, _frame, _timer| {
                let reachable = tokio::runtime::Handle::try_current().is_ok();
                if reachable {
                    let spawned = spawned.clone();
                    tokio::spawn(async move {
                        let _ = spawned.send("pump");
                    });
                }
                json!({ "ok": reachable })
            },
        );
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
        FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
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
            },
        )
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
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                if frame.frame_type == CLOSE_FRAME_TYPE {
                    recorder.lock().unwrap().push(CLOSE_FRAME_TYPE.into());
                    return json!({ "ok": true });
                }
                std::thread::sleep(Duration::from_millis(200));
                recorder
                    .lock()
                    .unwrap()
                    .push(frame.payload["id"].to_string());
                json!({ "ok": true })
            },
        );
        let dispatcher = Dispatcher::with_capacity(handler, 16, 4);

        let (sender, _rx, _key) = SessionSender::observable("s-closing");
        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.attach", json!({ "term_id": "term-1" })),
            )
            .await;
        dispatcher.close_session("s-closing", || true);

        for _ in 0..100 {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["1".to_string(), CLOSE_FRAME_TYPE.to_string()],
            "the close ran last"
        );
    }

    /// The race a second carrier makes real: a session's end has taken its
    /// lanes, and a frame for that session — admitted while it was still open
    /// — is dispatched behind it. It never runs: the close was the session's
    /// last frame, and an attach run after it would register a sender into a
    /// session that is gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_lane_frame_admitted_before_the_end_never_runs_after_it() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                let ran = if frame.frame_type == CLOSE_FRAME_TYPE {
                    CLOSE_FRAME_TYPE.to_string()
                } else {
                    frame.payload["id"].to_string()
                };
                recorder.lock().unwrap().push(ran);
                json!({ "ok": true })
            },
        );
        let dispatcher = Dispatcher::with_capacity(handler, 16, 4);
        let (sender, _rx, _key) = SessionSender::observable("s-ended");
        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.attach", json!({ "term_id": "term-1" })),
            )
            .await;
        for _ in 0..100 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        sender.opening_ended();
        dispatcher.close_session("s-ended", || true);
        dispatcher
            .dispatch(
                sender.clone(),
                request(2, "term.attach", json!({ "term_id": "term-1" })),
            )
            .await;

        for _ in 0..100 {
            if seen.lock().unwrap().contains(&CLOSE_FRAME_TYPE.to_string()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["1".to_string(), CLOSE_FRAME_TYPE.to_string()],
            "the attach dispatched behind the close was never run"
        );
    }

    /// The same race for a frame that names no terminal, so it waits in the
    /// pool rather than on a lane: taken by a worker after the session ended,
    /// it never runs. `session.hello` is the one that matters — run behind the
    /// close it would subscribe a session that is gone to every change event
    /// for the life of the carrier.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_pooled_frame_admitted_before_the_end_never_runs_after_it() {
        a_queued_frame_never_runs_after_its_session_ended("session.hello").await;
    }

    /// And for a read that waits in a fold rather than as a frame of its own.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_folded_read_admitted_before_the_end_never_runs_after_it() {
        a_queued_frame_never_runs_after_its_session_ended("board.list").await;
    }

    /// Queue `method` behind a held worker — the only one — end the session,
    /// close it, let the worker go, and check the frame's handler never ran.
    async fn a_queued_frame_never_runs_after_its_session_ended(method: &str) {
        let (gate, gated) = HandlerGate::new();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                if frame.payload["method"] == "hold" {
                    gated.hold();
                }
                let ran = if frame.frame_type == CLOSE_FRAME_TYPE {
                    CLOSE_FRAME_TYPE.to_string()
                } else {
                    frame.payload["method"].as_str().unwrap_or("").to_string()
                };
                recorder.lock().unwrap().push(ran);
                json!({ "ok": true })
            },
        );
        let dispatcher = Dispatcher::with_capacity(handler, 16, 1);
        let (sender, _rx, _key) = SessionSender::observable("s-ended");
        dispatcher
            .dispatch(sender.clone(), request(0, "hold", json!({})))
            .await;
        gate.wait_until_held();
        dispatcher
            .dispatch(sender.clone(), request(1, method, json!({})))
            .await;

        sender.opening_ended();
        dispatcher.close_session("s-ended", || true);
        gate.release();

        for _ in 0..100 {
            if seen.lock().unwrap().contains(&CLOSE_FRAME_TYPE.to_string()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut ran = seen.lock().unwrap().clone();
        ran.sort();
        assert_eq!(
            ran,
            vec![CLOSE_FRAME_TYPE.to_string(), "hold".to_string()],
            "the {method} queued behind the close never ran"
        );
    }

    /// A background frame says so in its envelope; everything else is
    /// foreground by omission.
    fn background_request(id: u64, method: &str, params: Value) -> Frame {
        let mut frame = request(id, method, params);
        frame.payload["priority"] = json!("background");
        frame
    }

    /// A handler that parks every `hold` frame on an [`OffLockGate`], so a test
    /// can keep an exact number of workers occupied and count who got in.
    fn gated_by(gate: crate::test_support::off_lock::OffLockGate) -> FrameHandler {
        FrameHandler::new(
            crate::timing::FrameClock::new(),
            move |_sender, frame, _timer| {
                if frame.payload["method"] == "hold" {
                    gate.arrive();
                }
                json!({ "id": frame.payload["id"], "ok": true })
            },
        )
    }

    /// The wedge this step exists for: the background tier pulls a whole feed at
    /// once, and the surface the human is looking at must not queue behind it.
    /// Eight background frames arrive; two occupy workers, six wait, and the
    /// foreground `board.list` is answered while every one of them is still held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_foreground_read_answers_while_eight_background_frames_wait() {
        let (gate, held) = crate::test_support::off_lock::OffLockGate::new();
        let handler = gated_by(gate);
        let clock = Arc::clone(&handler.clock);
        let dispatcher = Dispatcher::with_capacity(handler, 256, DISPATCH_WORKERS);
        let (sender, mut rx, key) = SessionSender::observable("s-priority");

        for id in 1..=8u64 {
            dispatcher
                .dispatch(sender.clone(), background_request(id, "hold", json!({})))
                .await;
        }
        for _ in 0..BACKGROUND_WORKERS {
            held.wait_for_arrival();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !held.has_pending_arrival(),
            "a background frame never occupies a third worker"
        );
        let stats = clock.stats();
        assert_eq!(
            stats["queues"]["background"]["depth"], 6,
            "six background frames wait for one of the two slots: {stats}"
        );

        dispatcher
            .dispatch(sender.clone(), request(9, "board.list", json!({})))
            .await;
        let answer = next_push(&mut rx, &key, SLOW_FRAME).await;
        assert_eq!(
            answer["id"], 9,
            "the foreground read overtook the whole background queue"
        );

        for _ in 0..8 {
            held.release();
        }
        for _ in 0..8 {
            next_push(&mut rx, &key, PATIENTLY).await;
        }
    }

    /// Terminal traffic is a stream per terminal, scheduled by its lane and by
    /// nothing else: a `priority` on a terminal frame changes nothing, even with
    /// every background slot taken.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_terminal_lane_is_unaffected_by_priority() {
        let (gate, held) = crate::test_support::off_lock::OffLockGate::new();
        let dispatcher = Dispatcher::with_capacity(gated_by(gate), 256, BACKGROUND_WORKERS);
        let (sender, mut rx, key) = SessionSender::observable("s-lane");

        for id in 1..=BACKGROUND_WORKERS as u64 {
            dispatcher
                .dispatch(sender.clone(), background_request(id, "hold", json!({})))
                .await;
        }
        for _ in 0..BACKGROUND_WORKERS {
            held.wait_for_arrival();
        }

        dispatcher
            .dispatch(
                sender.clone(),
                background_request(9, "term.input", json!({ "term_id": "t1" })),
            )
            .await;
        assert_eq!(
            next_push(&mut rx, &key, PATIENTLY).await["id"],
            9,
            "the terminal's lane ran it whatever its envelope claimed"
        );

        for _ in 0..BACKGROUND_WORKERS {
            held.release();
        }
    }

    /// The fold is about the question, not about who is in a hurry: a background
    /// pull that matches a read already queued is answered by it, for one compute.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_background_read_folds_into_a_queued_foreground_one() {
        let gate = Arc::new(AtomicBool::new(true));
        let computed: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let (started, mut started_rx) = mpsc::unbounded_channel();
        let handler = counting_handler(gate.clone(), computed.clone(), started);
        let dispatcher = Dispatcher::with_capacity(handler, 256, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-cross-fold");

        dispatcher
            .dispatch(sender.clone(), request(0, "hold", json!({})))
            .await;
        tokio::time::timeout(Duration::from_secs(5), started_rx.recv())
            .await
            .expect("the only worker is busy");

        dispatcher
            .dispatch(sender.clone(), request(1, "git.status", json!({})))
            .await;
        dispatcher
            .dispatch(
                sender.clone(),
                background_request(2, "git.status", json!({})),
            )
            .await;
        gate.store(false, Ordering::SeqCst);

        let mut ids = answered_ids(&mut rx, &key, 3).await;
        ids.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(ids, vec![json!(0), json!(1), json!(2)]);
        assert_eq!(
            *computed.lock().unwrap(),
            vec![("git.status".to_string(), json!({}))],
            "the background pull cost no second compute"
        );
    }

    /// The other direction of the same fold: a foreground read that joins a
    /// background one must not inherit its wait. The fold moves to the
    /// foreground queue, where a free worker takes it at once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_foreground_read_joining_a_background_fold_is_answered_at_once() {
        let (gate, held) = crate::test_support::off_lock::OffLockGate::new();
        let dispatcher = Dispatcher::with_capacity(gated_by(gate), 256, BACKGROUND_WORKERS + 1);
        let (sender, mut rx, key) = SessionSender::observable("s-promote");

        for id in 1..=BACKGROUND_WORKERS as u64 {
            dispatcher
                .dispatch(sender.clone(), background_request(id, "hold", json!({})))
                .await;
        }
        for _ in 0..BACKGROUND_WORKERS {
            held.wait_for_arrival();
        }

        // No background slot is free, so this one waits in the background queue.
        dispatcher
            .dispatch(
                sender.clone(),
                background_request(8, "board.list", json!({})),
            )
            .await;
        dispatcher
            .dispatch(sender.clone(), request(9, "board.list", json!({})))
            .await;

        let mut ids = answered_ids(&mut rx, &key, 2).await;
        ids.sort_by_key(|id| id.as_u64().unwrap_or_default());
        assert_eq!(
            ids,
            vec![json!(8), json!(9)],
            "the promoted fold answered both callers with the workers still held"
        );

        for _ in 0..BACKGROUND_WORKERS {
            held.release();
        }
    }

    use crate::timing::{recording_clock, SLOW_FRAME};
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stats_count_the_frames_waiting_for_a_worker() {
        let (gate, gated) = HandlerGate::new();
        let handler = FrameHandler::new(FrameClock::new(), move |_sender, frame, _timer| {
            if frame.payload["method"] == "held" {
                gated.hold();
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        let clock = Arc::clone(&handler.clock);
        // One worker: everything behind the held frame waits.
        let dispatcher = Dispatcher::with_capacity(handler, 8, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-depth");

        dispatcher
            .dispatch(sender.clone(), request(1, "held", json!({})))
            .await;
        gate.wait_until_held();
        assert_eq!(clock.stats()["queue_depth"], 0, "the held frame is running");

        for id in 2..5u64 {
            dispatcher
                .dispatch(sender.clone(), request(id, "waiting", json!({ "n": id })))
                .await;
        }
        assert_eq!(clock.stats()["queue_depth"], 3);

        gate.release();
        for _ in 0..4 {
            next_push(&mut rx, &key, PATIENTLY).await;
        }
        assert_eq!(clock.stats()["queue_depth"], 0, "the queue drained");
        assert_eq!(clock.stats()["frames_served"], 4);
    }

    /// The browser's clock starts when it sends, so a frame that sat in the queue
    /// is slow however fast its handler was. Its record — and its one line — must
    /// say so, or every stall reads as "the handler was fine".
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_frame_that_waited_for_a_worker_counts_the_wait_as_its_own() {
        let (clock, lines) = recording_clock();
        let (gate, gated) = HandlerGate::new();
        let handler = FrameHandler::new(clock.clone(), move |_sender, frame, _timer| {
            if frame.payload["method"] == "held" {
                gated.hold();
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        let dispatcher = Dispatcher::with_capacity(handler, 8, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-queued");

        dispatcher
            .dispatch(sender.clone(), request(1, "held", json!({})))
            .await;
        gate.wait_until_held();
        dispatcher
            .dispatch(sender.clone(), request(2, "quick", json!({})))
            .await;
        // Longer than SLOW_FRAME, and spent entirely in the queue: the `quick`
        // handler itself does nothing but answer.
        tokio::time::sleep(SLOW_FRAME + Duration::from_millis(20)).await;
        gate.release();
        for _ in 0..2 {
            next_push(&mut rx, &key, PATIENTLY).await;
        }

        let stats = clock.stats();
        assert!(
            stats["methods"]["quick"]["max_ms"].as_f64().unwrap() >= 200.0,
            "the queue wait belongs to the frame that waited: {stats}"
        );
        let lines = lines.lock().unwrap();
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("slow frame quick ")),
            "the frame that waited logged its own line: {lines:?}"
        );
    }

    /// A read that folds into one already waiting costs the queue nothing — the
    /// depth must report the one slot they share, not one per caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn folded_reads_cost_the_queue_one_slot() {
        let (gate, gated) = HandlerGate::new();
        let handler = FrameHandler::new(FrameClock::new(), move |_sender, frame, _timer| {
            if frame.payload["method"] == "held" {
                gated.hold();
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        let clock = Arc::clone(&handler.clock);
        let dispatcher = Dispatcher::with_capacity(handler, 8, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-fold");

        dispatcher
            .dispatch(sender.clone(), request(1, "held", json!({})))
            .await;
        gate.wait_until_held();
        for id in 2..6u64 {
            dispatcher
                .dispatch(sender.clone(), request(id, "board.list", json!({})))
                .await;
        }
        assert_eq!(
            clock.stats()["queue_depth"],
            1,
            "four identical reads wait on one queue slot"
        );

        gate.release();
        for _ in 0..5 {
            next_push(&mut rx, &key, PATIENTLY).await;
        }
        assert_eq!(clock.stats()["queue_depth"], 0);
        assert_eq!(
            clock.stats()["methods"]["board.list"]["served"],
            1,
            "one compute answered all four"
        );
    }
}
