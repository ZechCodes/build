//! One tab's screen: the vt100 model a terminal paints into, the clients
//! watching it, and the flow control between them.
//!
//! Every screen lives behind its own [`ScreenHandle`] — a lock of its own, held
//! by the pump that feeds it and by whichever frame is reading it. The app
//! mutex resolves a handle and releases; nothing here can see `AppState`, which
//! is what makes "a flooding PTY never blocks a board read" a property of the
//! module rather than a rule the callers remember.
//!
//! The screen is authoritative: a client reconnects on a snapshot plus a
//! monotonic byte cursor, never on a byte replay, so one model serves a shell
//! and an agent alike.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::PtySize;
use serde_json::{json, Value};

use crate::app::b64encode;
use crate::harness::{AgentSession, TerminalView};
use crate::relay::SessionSender;

/// Authoritative server-side screen: vt100 model + attach list + coalescing
/// buffer + the monotonic byte cursor. Snapshot resync, not byte replay. One
/// model for every tab — a shell and an agent reconnect the same way.
struct TermScreen {
    parser: vt100::Parser,
    attached: Vec<AttachedClient>,
    /// Output coalescing buffer: PTY bytes accumulate here and flush on a timer,
    /// so a repaint becomes one frame instead of ten.
    pending: Vec<u8>,
    /// Total output bytes processed — the live-tail cursor.
    total: u64,
    /// When the last flood-collapse snapshot went out, or `None` if this screen
    /// has never collapsed one. Rate-limits the collapse; a per-client attach
    /// snapshot is a different thing and does not touch it.
    last_flood_snapshot_at: Option<std::time::Instant>,
    /// How long one flood collapse holds off the next. Always
    /// [`TERM_SNAPSHOT_MIN_INTERVAL_MS`] in production; a test widens it so that
    /// real time cannot slip past the window while the test is doing the work
    /// the window is supposed to suppress.
    snapshot_min_interval: Duration,
    /// Set when a flush dropped its backlog without sending anything. Until the
    /// rate-limit window reopens and the snapshot ships, this screen owes the
    /// client a resync and must not send raw output — the bytes it would carry
    /// are no longer contiguous.
    snapshot_due: bool,
    cols: u16,
    rows: u16,
}

/// One client attached to a screen, and how far behind it is running.
///
/// The bridge cannot see the browser's receive queue, so the client tells it:
/// every applied frame is acknowledged with the cursor it reached
/// (`term.ack`), and the gap between that and the live cursor is the only
/// measure of a client that is not draining.
struct AttachedClient {
    sender: SessionSender,
    /// The highest cursor this client has reported applying. Seeded at attach
    /// with the cursor the attach snapshot carries.
    acked_cursor: u64,
    /// The highest cursor actually pushed to this client. While it is paused
    /// the live cursor runs ahead of this, and this — not the live cursor — is
    /// the most its acks can ever reach, so the resume is measured against it.
    sent_cursor: u64,
    /// Whether this client has ever acknowledged anything. A client that has
    /// not is exempt from flow control — an older SPA sends no acks, and
    /// measuring it by a cursor it never reports would stall it forever.
    sent_ack: bool,
    /// Set once the client fell past [`TERM_UNACKED_BUDGET_BYTES`]. It receives
    /// nothing until it has acked everything it was sent (`sent_cursor`), and
    /// comes back on a snapshot because the frames it missed left a hole in
    /// its byte stream.
    paused: bool,
}

impl AttachedClient {
    /// Output bytes this client has been sent but not acknowledged.
    fn lag(&self, total: u64) -> u64 {
        total.saturating_sub(self.acked_cursor)
    }

    /// Whether this client is too far behind to keep feeding. Only a client
    /// that acks at all can be judged this way.
    fn falling_behind(&self, total: u64) -> bool {
        self.sent_ack && self.lag(total) > TERM_UNACKED_BUDGET_BYTES
    }
}
/// Flush coalesced terminal output at ~100 fps.
pub(crate) const TERM_FLUSH_MS: u64 = 10;
/// How many bytes one client may leave unacknowledged before the bridge stops
/// feeding it.
///
/// Every frame for a browser tab rides ONE FIFO (bridge channel → relay queue →
/// browser demux), so a client that cannot drain as fast as a PTY floods does
/// not just fall behind: it becomes an unbounded queue that everything else —
/// the liveness ping, every keystroke — waits behind. A megabyte is far more
/// than any screen and far less than a stall, and past it chasing the client
/// with bytes it will never catch up on is worse than resyncing it with one
/// snapshot the moment it drains.
const TERM_UNACKED_BUDGET_BYTES: u64 = 1024 * 1024;
/// If a single flush exceeds this, send the current screen snapshot instead of
/// the raw byte backlog — collapses a massive burst (scroll/flood) to one frame
/// and bounds per-frame size. The vt100 model makes this lossless for the screen.
const TERM_SNAPSHOT_THRESHOLD: usize = 128 * 1024;
/// Minimum gap between two flood-collapse snapshots on one screen. Bounds a
/// sustained flood to ~10 screens/sec, which is all a human can perceive —
/// without it a 10 ms flush cadence would push up to 100 full screens/sec
/// through the relay and starve every other frame behind them.
const TERM_SNAPSHOT_MIN_INTERVAL_MS: u64 = 100;

impl TermScreen {
    fn new(cols: u16, rows: u16) -> TermScreen {
        TermScreen {
            parser: vt100::Parser::new(rows, cols, 2000),
            attached: Vec::new(),
            pending: Vec::new(),
            total: 0,
            last_flood_snapshot_at: None,
            snapshot_min_interval: Duration::from_millis(TERM_SNAPSHOT_MIN_INTERVAL_MS),
            snapshot_due: false,
            cols,
            rows,
        }
    }

    /// Test-only: age the last flood-collapse stamp by `ago`, so a screen that
    /// just collapsed reports the rate-limit window as reopened. The real gap is
    /// 100 ms; sleeping it in every flood test would be paid on every run for no
    /// added coverage — only the clock moves, the screen model is untouched.
    #[cfg(test)]
    fn backdate_last_flood_snapshot(&mut self, ago: Duration) {
        self.last_flood_snapshot_at = self.last_flood_snapshot_at.map(|at| {
            at.checked_sub(ago)
                .expect("a stamp old enough to age by the rate-limit window")
        });
    }

    /// The current screen serialized as escape sequences — write it to a fresh
    /// terminal and the screen is reproduced.
    fn snapshot(&self) -> String {
        b64encode(&self.parser.screen().contents_formatted())
    }

    fn set_size(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
        self.cols = cols;
        self.rows = rows;
    }

    /// Feed PTY bytes: advance the screen model, the cursor, and the pending
    /// coalescing buffer.
    fn process(&mut self, chunk: &[u8]) {
        self.parser.process(chunk);
        self.total += chunk.len() as u64;
        self.pending.extend_from_slice(chunk);
    }

    /// Register a client for live output, dropping any prior sender with the
    /// same session id first (a reconnect on the same id).
    ///
    /// The new client starts acknowledged up to the live cursor: the attach
    /// response carries that same cursor with the screen snapshot, so it owes
    /// nothing for anything that came before. It starts unpaused and, until its
    /// first ack, exempt from flow control.
    fn register(&mut self, sender: &SessionSender) {
        self.attached
            .retain(|client| client.sender.session_id() != sender.session_id());
        self.attached.push(AttachedClient {
            sender: sender.clone(),
            acked_cursor: self.total,
            sent_cursor: self.total,
            sent_ack: false,
            paused: false,
        });
    }

    /// Record what a client has applied, and resync it once a paused client
    /// has drained everything it was actually sent.
    ///
    /// A paused client missed frames, so the raw stream it left is no longer
    /// contiguous with what it holds (the INVARIANT raw output rides on). It
    /// comes back on one snapshot at the live cursor — a full screen, so it
    /// replaces whatever the client was left holding — and resumes from there.
    /// A client whose connection died while paused is dropped here, since a
    /// paused client is not fed and a failed push is the only proof left.
    fn ack(&mut self, term_id: &str, session_id: &str, cursor: u64) {
        let total = self.total;
        let Some(index) = self.index_of_session(session_id) else {
            return;
        };
        let client = &mut self.attached[index];
        client.sent_ack = true;
        // A cursor past what the bridge has produced acknowledges nothing real;
        // clamping keeps a confused client measurable rather than exempt.
        client.acked_cursor = client.acked_cursor.max(cursor.min(total));
        // A paused client is fed nothing, so the live cursor runs away from it
        // without bound — measuring the resume against the live cursor could
        // hold a client that drained everything it was ever sent paused
        // forever, with no frame left that could unpause it. Its own last sent
        // frame is the most it can ack, and acking that means its queue is
        // empty: resync it now.
        if !client.paused || client.acked_cursor < client.sent_cursor {
            return;
        }
        // PTY bytes arrive on their own channel, so an ack can land between a
        // `process` and the flush that would have shipped it. Those bytes are
        // already on the screen this resync serializes, so the next flush must
        // not hand them to the resumed client a second time as raw output.
        // Flushing first empties `pending` — the clients that are keeping up
        // get those bytes now, the paused one is skipped as always — and
        // leaves the snapshot standing exactly at the live cursor.
        self.flush(term_id);
        // The flush drops clients whose connection is gone, so the index has to
        // be taken again; this client is paused, so it cannot be one of them.
        let Some(index) = self.index_of_session(session_id) else {
            return;
        };
        let payload = json!({ "type": "term.reset", "term_id": term_id, "data": self.snapshot(), "cursor": self.total });
        let client = &mut self.attached[index];
        client.paused = false;
        client.sent_cursor = self.total;
        // The snapshot is a fresh baseline, exactly like the attach snapshot
        // in `register`: the client owes nothing before it. Leaving the old
        // acked cursor standing would count the whole paused gap as unacked
        // debt and re-pause the client on the very next flush.
        client.acked_cursor = self.total;
        if !client.sender.push(payload) {
            self.attached.remove(index);
        }
    }

    fn index_of_session(&self, session_id: &str) -> Option<usize> {
        self.attached
            .iter()
            .position(|client| client.sender.session_id() == session_id)
    }

    /// Push one frame to every client that is keeping up: a client past its
    /// unacked budget is paused and skipped (its own resync will catch it up),
    /// a client already paused stays skipped, and a client whose connection is
    /// gone is dropped.
    fn push_to_keeping_up(&mut self, payload: Value) {
        let total = self.total;
        self.attached.retain_mut(|client| {
            if client.falling_behind(total) {
                client.paused = true;
            }
            if client.paused {
                return true;
            }
            if !client.sender.push(payload.clone()) {
                return false;
            }
            // Every frame this method carries stands at the live cursor.
            client.sent_cursor = total;
            true
        });
    }

    /// Flush pending bytes as one keyed push to every attached client — raw
    /// output, or a screen snapshot when the backlog crosses the collapse
    /// threshold. Senders whose connection is gone are dropped.
    ///
    /// Collapsing is rate-limited to one snapshot per
    /// [`TERM_SNAPSHOT_MIN_INTERVAL_MS`]. A flush that crosses the threshold
    /// inside that window drops its backlog silently and records the debt in
    /// `snapshot_due`: the vt100 model and the cursor already advanced in
    /// [`Self::process`], so the screen the next snapshot carries is still
    /// exactly right. While the debt stands nothing raw may go out — those bytes
    /// would land on a client whose stream now has a hole in it.
    fn flush(&mut self, term_id: &str) {
        let collapsing = self.snapshot_due || self.pending.len() > TERM_SNAPSHOT_THRESHOLD;
        if !collapsing {
            if self.pending.is_empty() {
                return;
            }
            let payload = json!({ "type": "term.output", "term_id": term_id, "data": b64encode(&self.pending), "cursor": self.total });
            self.pending.clear();
            self.push_to_keeping_up(payload);
            return;
        }

        // The backlog is skipped either way — the screen model already holds it.
        self.pending.clear();
        let window_reopened = self
            .last_flood_snapshot_at
            .is_none_or(|at| at.elapsed() >= self.snapshot_min_interval);
        if !window_reopened {
            self.snapshot_due = true;
            return;
        }
        self.snapshot_due = false;
        self.last_flood_snapshot_at = Some(std::time::Instant::now());
        let payload = json!({ "type": "term.reset", "term_id": term_id, "data": self.snapshot(), "cursor": self.total });
        self.push_to_keeping_up(payload);
    }

    /// Tell every attached client this terminal ended, and why. A paused client
    /// hears it too: flow control withholds output, never the fact that there
    /// is no more of it coming.
    fn push_closed(&self, term_id: &str, reason: &str) {
        let payload = json!({ "type": "term.closed", "term_id": term_id, "reason": reason });
        for client in &self.attached {
            client.sender.push(payload.clone());
        }
    }
}

/// One tab's screen, shared by everything that reads or writes it.
///
/// Clone it to hold it: the app mutex is taken only long enough to clone a
/// handle out of the registry, and every read and write below happens with
/// that mutex released. The term id travels with the handle because every push
/// the screen makes is keyed by it, and a screen keyed by one tab is never
/// pushed under another's name.
#[derive(Clone)]
pub struct ScreenHandle {
    term_id: Arc<str>,
    screen: Arc<Mutex<TermScreen>>,
}

/// What a client needs to render a screen it has just attached to.
pub struct AttachSnapshot {
    pub snapshot: String,
    pub cursor: u64,
    pub cols: u16,
    pub rows: u16,
}

impl ScreenHandle {
    pub fn new(term_id: &str, cols: u16, rows: u16) -> ScreenHandle {
        ScreenHandle {
            term_id: Arc::from(term_id),
            screen: Arc::new(Mutex::new(TermScreen::new(cols, rows))),
        }
    }

    /// Register `sender` for live output and hand back what it should render.
    ///
    /// Registration and snapshot happen under one acquisition of the screen's
    /// own lock, which is what makes them atomic: the pump feeds through the
    /// same lock, so no byte can land between the snapshot this client renders
    /// and the cursor it is told to resume from.
    ///
    /// `viewport` is the grid the client is looking at, or `None` for a dead
    /// tab — a retained screen is the last thing its agent painted and is never
    /// reflowed to fit a browser window that arrived after it died.
    pub fn attach(&self, sender: &SessionSender, viewport: Option<(u16, u16)>) -> AttachSnapshot {
        let mut screen = self.screen.lock().unwrap();
        if let Some((cols, rows)) = viewport {
            if (screen.cols, screen.rows) != (cols, rows) {
                screen.set_size(cols, rows);
            }
        }
        screen.register(sender);
        Self::reading(&screen)
    }

    /// The screen as it stands, for a client that is not attaching to it.
    pub fn snapshot(&self) -> AttachSnapshot {
        Self::reading(&self.screen.lock().unwrap())
    }

    fn reading(screen: &TermScreen) -> AttachSnapshot {
        AttachSnapshot {
            snapshot: screen.snapshot(),
            cursor: screen.total,
            cols: screen.cols,
            rows: screen.rows,
        }
    }

    /// The grid this screen is currently painting at.
    pub fn size(&self) -> (u16, u16) {
        let screen = self.screen.lock().unwrap();
        (screen.cols, screen.rows)
    }

    /// Record how far a client has applied, resyncing it if it has drained.
    pub fn ack(&self, session_id: &str, cursor: u64) {
        self.screen
            .lock()
            .unwrap()
            .ack(&self.term_id, session_id, cursor);
    }

    /// Drop a client from this screen. Answers whether any client is left —
    /// a screen nobody is watching for an agent that does not exist yet is a
    /// screen with nothing to hold.
    pub fn detach(&self, session_id: &str) -> bool {
        let mut screen = self.screen.lock().unwrap();
        screen
            .attached
            .retain(|client| client.sender.session_id() != session_id);
        !screen.attached.is_empty()
    }

    /// The pump's write: PTY bytes into the model and the coalescing buffer.
    pub fn feed(&self, chunk: &[u8]) {
        self.screen.lock().unwrap().process(chunk);
    }

    /// The pump's tick: whatever has accumulated, as one frame per client.
    pub fn flush(&self) {
        self.screen.lock().unwrap().flush(&self.term_id);
    }

    /// A new session is painting here: blank the parser and resync every
    /// attached client. The cursor is NEVER reset — client dedupe rides it, so
    /// a replacement process continues the numbering its predecessor left.
    pub fn restart(&self) {
        let mut screen = self.screen.lock().unwrap();
        screen.parser = vt100::Parser::new(screen.rows, screen.cols, 2000);
        screen.pending.clear();
        // This reset resyncs every attached client, so a snapshot a previous
        // session's flood left owing is already paid.
        screen.snapshot_due = false;
        let payload = json!({
            "type": "term.reset",
            "term_id": &*self.term_id,
            "data": screen.snapshot(),
            "cursor": screen.total,
        });
        screen.push_to_keeping_up(payload);
    }

    /// Take the clients waiting on `waiting` — and the viewport they are
    /// rendering at — onto this screen.
    ///
    /// Two screens are never locked at once: the waiting screen is drained
    /// under its own lock, which is released before this one is taken.
    pub fn carry_clients_from(&self, waiting: &ScreenHandle) {
        let (carried, cols, rows) = {
            let mut waiting = waiting.screen.lock().unwrap();
            let carried = std::mem::take(&mut waiting.attached);
            (carried, waiting.cols, waiting.rows)
        };
        let mut screen = self.screen.lock().unwrap();
        screen.set_size(cols, rows);
        for client in &carried {
            screen.register(&client.sender);
        }
    }

    /// Tell every attached client this terminal ended, and why.
    ///
    /// Bounded by construction — one encrypt and one unbounded channel send per
    /// client — which is why this is the one screen call a caller may make with
    /// the app mutex still in hand.
    pub fn close(&self, reason: &str) {
        self.screen
            .lock()
            .unwrap()
            .push_closed(&self.term_id, reason);
    }

    /// The last words on this screen: its final non-empty lines, trimmed and
    /// bounded. `None` for a harness that painted nothing worth repeating.
    pub fn epitaph(&self) -> Option<String> {
        const MAX_LINES: usize = 3;
        const MAX_CHARS: usize = 240;
        let contents = self.screen.lock().unwrap().parser.screen().contents();
        let mut lines: Vec<&str> = contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.is_empty() {
            return None;
        }
        let tail = lines.split_off(lines.len().saturating_sub(MAX_LINES));
        let mut said = tail.join(" · ");
        if said.chars().count() > MAX_CHARS {
            said = said.chars().take(MAX_CHARS).collect::<String>() + "…";
        }
        Some(said)
    }

    /// How many clients are watching. Tests only: production asks the questions
    /// above, which each answer for themselves.
    #[cfg(test)]
    pub fn attached(&self) -> usize {
        self.screen.lock().unwrap().attached.len()
    }

    /// Hold this screen's lock until the returned guard is dropped. Tests
    /// only: it is how a test parks a screen where a flooding pump would park
    /// it and proves the rest of the daemon answers anyway.
    #[cfg(test)]
    pub fn hold(&self) -> HeldScreen<'_> {
        HeldScreen(self.screen.lock().unwrap())
    }

    /// Which sessions are watching, in attach order. Tests only.
    #[cfg(test)]
    pub fn attached_sessions(&self) -> Vec<String> {
        self.screen
            .lock()
            .unwrap()
            .attached
            .iter()
            .map(|client| client.sender.session_id().to_string())
            .collect()
    }

    /// The live cursor. Tests only, for the same reason.
    #[cfg(test)]
    pub fn cursor(&self) -> u64 {
        self.screen.lock().unwrap().total
    }
}

/// A tab's terminal and the grid it paints into, taken out of the registry
/// together so both can be written to with the app mutex released.
///
/// A PTY write blocks when the child stops draining, so it is the other thing
/// that must never happen under the app mutex — and the resize below is two
/// writes that have to agree, the ioctl the child reads and the grid the
/// clients render.
#[derive(Clone)]
pub struct TerminalHandle {
    session: Arc<dyn AgentSession>,
    screen: ScreenHandle,
}

impl TerminalHandle {
    /// The handle for a session that offers a terminal, or `None` for one that
    /// does not — the terminal is a capability, and a grid without one is a
    /// screen nothing can ever paint.
    pub fn of(
        session: &Arc<dyn AgentSession>,
        screen: &Option<ScreenHandle>,
    ) -> Option<TerminalHandle> {
        let screen = screen.as_ref()?;
        session.terminal()?;
        Some(TerminalHandle {
            session: Arc::clone(session),
            screen: screen.clone(),
        })
    }

    pub fn screen(&self) -> &ScreenHandle {
        &self.screen
    }

    fn terminal(&self) -> &dyn TerminalView {
        self.session
            .terminal()
            .expect("a handle is only built for a session that offers a terminal")
    }

    /// Tell the child its window changed. The grid is the caller's half.
    fn tell_child(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.terminal()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }

    /// Forward a human's keystrokes to the child, untouched.
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), String> {
        self.terminal()
            .write_input(bytes)
            .map_err(|e| e.to_string())
    }

    /// Resize the child and the grid together. A TUI draws to the size it was
    /// told, so the two must never disagree.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.tell_child(cols, rows)?;
        self.screen.screen.lock().unwrap().set_size(cols, rows);
        Ok(())
    }

    /// Show this terminal to a client: match the child to the viewport it is
    /// looking at, register it for live output, and hand back what it should
    /// render. `viewport` is `None` for a dead tab, whose retained screen is
    /// the last thing its agent painted.
    ///
    /// A child that refuses the ioctl is not the attach's business: it is
    /// dying, and the client still gets the screen it died on.
    pub fn attach(&self, sender: &SessionSender, viewport: Option<(u16, u16)>) -> AttachSnapshot {
        if let Some((cols, rows)) = viewport {
            if self.screen.size() != (cols, rows) {
                let _ = self.tell_child(cols, rows);
            }
        }
        self.screen.attach(sender, viewport)
    }

    /// Take the clients waiting on `waiting` onto this terminal's screen, and
    /// size the child to the viewport they are rendering at.
    pub fn adopt_clients_of(&self, waiting: &ScreenHandle) {
        let (cols, rows) = waiting.size();
        let _ = self.tell_child(cols, rows);
        self.screen.carry_clients_from(waiting);
    }
}

/// A screen's lock, held open by a test. The guard is the whole point: it is
/// never read, it is held.
#[cfg(test)]
pub struct HeldScreen<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, TermScreen>);

#[cfg(test)]
mod tests {
    use super::*;

    /// Two screens are never locked at once. A spawn carries the clients that
    /// were waiting for it onto the screen it just made, and if it held the
    /// waiting screen while it waited for the new one, two spawns racing
    /// through the same pair would deadlock the daemon. The waiting screen is
    /// drained and released first.
    #[test]
    fn carrying_clients_between_two_screens_holds_one_lock_at_a_time() {
        let (sender, _pushes, _key) = SessionSender::observable("waiting-client");
        let waiting = ScreenHandle::new("agent:carried", 90, 25);
        waiting.attach(&sender, None);
        let born = ScreenHandle::new("agent:carried", 80, 24);

        // The destination is busy — a flood, a snapshot, a slow client.
        let held = born.hold();
        let carrying = {
            let born = born.clone();
            let waiting = waiting.clone();
            std::thread::spawn(move || born.carry_clients_from(&waiting))
        };

        let (answered, answers) = std::sync::mpsc::channel();
        let source = waiting.clone();
        std::thread::spawn(move || {
            let _ = answered.send(source.attached());
        });
        assert_eq!(
            answers
                .recv_timeout(Duration::from_secs(5))
                .expect("the source screen is free while the destination is waited on"),
            0,
            "the waiting screen is drained before the new one is locked"
        );

        drop(held);
        carrying.join().expect("the carry completes");
        assert_eq!(
            born.attached_sessions(),
            vec!["waiting-client".to_string()],
            "the client it was holding is on the new screen"
        );
        assert_eq!(
            born.size(),
            (90, 25),
            "at the viewport it was already rendering at"
        );
    }

    /// The bytes one `term.output` push carries.
    fn b64decode(data: &str) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .expect("the bridge encodes what it pushes")
    }

    /// The concatenated bytes of every `term.output` push for `term_id`.
    fn output_text(pushes: &[Value], term_id: &str) -> String {
        let mut bytes = Vec::new();
        for push in pushes {
            if push["type"] == "term.output" && push["term_id"] == term_id {
                bytes.extend_from_slice(&b64decode(push["data"].as_str().unwrap()));
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Drain every decrypted push a test sender has captured so far.
    fn drain_pushes(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        session_key: &str,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(message) = rx.try_recv() {
            seen.push(SessionSender::decrypt_push(session_key, &message));
        }
        seen
    }

    /// A screen with one observable client attached, ready to flush.
    ///
    /// The rate-limit window is widened far past the production 100 ms: a debug
    /// build parsing 128 KB through vt100 on a machine running the whole suite
    /// in parallel can itself outlast the real window, which would let a test
    /// about suppression watch a legitimate snapshot go out. Every test that
    /// needs the window to reopen says so with `backdate_last_flood_snapshot`.
    const HELD_FLOOD_WINDOW: Duration = Duration::from_secs(60);

    fn flooded_screen() -> (
        TermScreen,
        tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        String,
    ) {
        let (sender, pushes, session_key) = SessionSender::observable("flood-client");
        let mut screen = TermScreen::new(80, 24);
        screen.snapshot_min_interval = HELD_FLOOD_WINDOW;
        screen.register(&sender);
        (screen, pushes, session_key)
    }

    /// More than one flush's worth of backlog: enough to cross the collapse
    /// threshold on its own.
    fn flood_chunk() -> Vec<u8> {
        vec![b'x'; TERM_SNAPSHOT_THRESHOLD + 1]
    }

    /// A flood produces an over-threshold backlog every 10 ms tick. Collapsing
    /// each one to a full-screen snapshot is ~100 screens/sec through the relay,
    /// which head-of-line-blocks everything behind it. The first collapse goes
    /// out; the next one inside the rate-limit window drops its backlog and
    /// sends nothing at all.
    #[test]
    fn a_second_flood_collapse_inside_the_window_sends_nothing() {
        let (mut screen, mut pushes, session_key) = flooded_screen();

        screen.process(&flood_chunk());
        screen.flush("term-1");
        let first = drain_pushes(&mut pushes, &session_key);
        assert_eq!(first.len(), 1, "the first collapse goes out: {first:?}");
        assert_eq!(first[0]["type"], "term.reset", "{first:?}");

        screen.process(&flood_chunk());
        screen.flush("term-1");
        let second = drain_pushes(&mut pushes, &session_key);
        assert!(
            second.is_empty(),
            "a collapse inside the rate-limit window sends nothing: {second:?}"
        );
        assert!(
            screen.pending.is_empty(),
            "the dropped backlog is cleared, not carried into the next flush"
        );
        assert!(
            screen.snapshot_due,
            "dropping bytes owes the client a resync snapshot"
        );
    }

    /// The tail of a flood is the part a human actually reads. Once the window
    /// reopens, the owed snapshot goes out on the next flush even if barely any
    /// bytes arrived in that tick — otherwise the last screen of a flood is the
    /// one that never ships.
    #[test]
    fn the_owed_snapshot_ships_once_the_window_reopens() {
        let (mut screen, mut pushes, session_key) = flooded_screen();

        screen.process(&flood_chunk());
        screen.flush("term-1");
        screen.process(&flood_chunk());
        screen.flush("term-1");
        drain_pushes(&mut pushes, &session_key);
        assert!(screen.snapshot_due);

        screen.backdate_last_flood_snapshot(HELD_FLOOD_WINDOW + Duration::from_millis(10));
        let cursor_before_tail = screen.total;
        screen.process(b"tail");
        screen.flush("term-1");

        let tail = drain_pushes(&mut pushes, &session_key);
        assert_eq!(tail.len(), 1, "exactly one resync frame: {tail:?}");
        assert_eq!(tail[0]["type"], "term.reset", "{tail:?}");
        assert_eq!(
            tail[0]["cursor"].as_u64().unwrap(),
            cursor_before_tail + 4,
            "the snapshot carries the live cursor: {tail:?}"
        );
        assert!(!screen.snapshot_due, "the debt is settled");

        // An empty tick after the debt is settled sends nothing.
        screen.flush("term-1");
        assert!(drain_pushes(&mut pushes, &session_key).is_empty());
    }

    /// Raw `term.output` bytes must be contiguous — the client applies them by
    /// cursor. Once a flood-collapse has dropped bytes, raw output would paint
    /// a garbled screen, so nothing but the resync snapshot may go out until the
    /// debt is settled.
    #[test]
    fn no_raw_output_ships_between_a_dropped_backlog_and_its_resync() {
        let (mut screen, mut pushes, session_key) = flooded_screen();

        screen.process(&flood_chunk());
        screen.flush("term-1");
        screen.process(&flood_chunk());
        screen.flush("term-1");
        assert!(screen.snapshot_due);
        drain_pushes(&mut pushes, &session_key);

        // Small ticks while the debt stands: each one is dropped silently.
        for _ in 0..5 {
            screen.process(b"garble");
            screen.flush("term-1");
        }
        screen.backdate_last_flood_snapshot(HELD_FLOOD_WINDOW + Duration::from_millis(10));
        screen.process(b"garble");
        screen.flush("term-1");

        let seen = drain_pushes(&mut pushes, &session_key);
        assert!(
            seen.iter().all(|push| push["type"] != "term.output"),
            "no raw output crosses a gap in the byte stream: {seen:?}"
        );
        assert_eq!(
            seen.iter()
                .filter(|push| push["type"] == "term.reset")
                .count(),
            1,
            "one resync closes the gap: {seen:?}"
        );
    }

    /// The ordinary case — a prompt, a command, some output — is untouched by
    /// the flood rate limit: raw frames with advancing cursors, no snapshots.
    #[test]
    fn small_steady_output_still_ships_raw_with_advancing_cursors() {
        let (mut screen, mut pushes, session_key) = flooded_screen();

        for line in ["one\r\n", "two\r\n", "three\r\n"] {
            screen.process(line.as_bytes());
            screen.flush("term-1");
        }

        let seen = drain_pushes(&mut pushes, &session_key);
        assert_eq!(seen.len(), 3, "one frame per flush: {seen:?}");
        assert!(
            seen.iter().all(|push| push["type"] == "term.output"),
            "small output never collapses to a snapshot: {seen:?}"
        );
        let cursors: Vec<u64> = seen
            .iter()
            .map(|push| push["cursor"].as_u64().unwrap())
            .collect();
        assert_eq!(cursors, vec![5, 10, 17], "{seen:?}");
        assert_eq!(output_text(&seen, "term-1"), "one\r\ntwo\r\nthree\r\n");
    }

    /// One chunk of output well under the flood-collapse threshold, so a flush
    /// of it ships as raw `term.output`. Sixteen of them exceed the unacked
    /// budget — the ack tests count in these.
    const ACK_TEST_CHUNK: usize = 100 * 1024;

    fn chunk_of(bytes: usize) -> Vec<u8> {
        vec![b'x'; bytes]
    }

    /// One client's capture: everything the bridge pushed to it, and the
    /// session key those pushes decrypt with.
    type ClientCapture = (
        tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        String,
    );

    /// A screen with two observable clients attached, each with its own capture.
    fn two_client_screen() -> (TermScreen, ClientCapture, ClientCapture) {
        let (first_sender, first_pushes, first_key) = SessionSender::observable("client-one");
        let (second_sender, second_pushes, second_key) = SessionSender::observable("client-two");
        let mut screen = TermScreen::new(80, 24);
        screen.snapshot_min_interval = HELD_FLOOD_WINDOW;
        screen.register(&first_sender);
        screen.register(&second_sender);
        (
            screen,
            (first_pushes, first_key),
            (second_pushes, second_key),
        )
    }

    /// Push one chunk and flush it, then have `acking` acknowledge everything
    /// the screen has produced so far.
    fn flush_chunk_acked_by(screen: &mut TermScreen, acking: &str) {
        screen.process(&chunk_of(ACK_TEST_CHUNK));
        screen.flush("term-1");
        screen.ack("term-1", acking, screen.total);
    }

    /// A client that acknowledges what it received is keeping up by definition,
    /// so nothing about flow control may interrupt its stream — however much
    /// output flows through it.
    #[test]
    fn a_client_that_keeps_acking_keeps_receiving_raw_output() {
        let (mut screen, (mut pushes, session_key), _) = two_client_screen();

        for _ in 0..16 {
            flush_chunk_acked_by(&mut screen, "client-one");
        }

        let seen = drain_pushes(&mut pushes, &session_key);
        assert_eq!(seen.len(), 16, "every flush reached the client: {seen:?}");
        assert!(
            seen.iter().all(|push| push["type"] == "term.output"),
            "an acking client is never resynced out of the raw stream: {seen:?}"
        );
        assert_eq!(
            seen.last().unwrap()["cursor"].as_u64().unwrap(),
            screen.total
        );
    }

    /// A client whose acks stop is a client that is not draining: its frames are
    /// piling up in the bridge's channel and the relay's queue, and every frame
    /// behind them — the liveness ping, the human's keystrokes — waits on the
    /// pile. Past the budget it stops being fed. The other client is a different
    /// connection and must not be slowed by its neighbour.
    #[test]
    fn a_client_that_stops_acking_stops_being_fed_and_the_other_does_not() {
        let (mut screen, (mut silent_pushes, silent_key), (mut acking_pushes, acking_key)) =
            two_client_screen();

        // Both acknowledge the first flush, so neither is exempt as never-acked.
        screen.process(&chunk_of(ACK_TEST_CHUNK));
        screen.flush("term-1");
        screen.ack("term-1", "client-one", screen.total);
        screen.ack("term-1", "client-two", screen.total);
        drain_pushes(&mut silent_pushes, &silent_key);
        drain_pushes(&mut acking_pushes, &acking_key);

        // client-one goes silent while output keeps flowing past the budget.
        for _ in 0..16 {
            flush_chunk_acked_by(&mut screen, "client-two");
        }

        // It is fed until the flush that carries it PAST the budget: ten 100 KiB
        // chunks fit inside a megabyte, the eleventh does not.
        let fits_in_budget = (TERM_UNACKED_BUDGET_BYTES / ACK_TEST_CHUNK as u64) as usize;
        let silent = drain_pushes(&mut silent_pushes, &silent_key);
        assert_eq!(
            silent.len(),
            fits_in_budget,
            "a client past its unacked budget stops being fed: {silent:?}"
        );
        let fed_bytes: u64 = silent
            .iter()
            .map(|push| b64decode(push["data"].as_str().unwrap()).len() as u64)
            .sum();
        assert!(
            fed_bytes <= TERM_UNACKED_BUDGET_BYTES,
            "nothing past the budget went out: {fed_bytes}"
        );

        let acking = drain_pushes(&mut acking_pushes, &acking_key);
        assert_eq!(
            acking.len(),
            16,
            "the client that kept acking kept receiving: {acking:?}"
        );
    }

    /// A paused client drains, acks, and comes back under budget. It missed
    /// frames while paused, so the raw stream it left is no longer contiguous
    /// with what it holds: exactly one snapshot resyncs it, and raw output
    /// resumes from there.
    #[test]
    fn an_ack_under_budget_resyncs_the_paused_client_once_then_resumes_raw_output() {
        let (mut screen, (mut pushes, session_key), _) = two_client_screen();

        screen.process(&chunk_of(ACK_TEST_CHUNK));
        screen.flush("term-1");
        screen.ack("term-1", "client-one", screen.total);
        for _ in 0..16 {
            flush_chunk_acked_by(&mut screen, "client-two");
        }
        drain_pushes(&mut pushes, &session_key);

        // It catches up on everything the bridge has produced.
        let caught_up_at = screen.total;
        screen.ack("term-1", "client-one", caught_up_at);
        let resync = drain_pushes(&mut pushes, &session_key);
        assert_eq!(resync.len(), 1, "exactly one resync frame: {resync:?}");
        assert_eq!(
            resync[0]["type"], "term.reset",
            "the resync is a snapshot, never raw bytes over a gap: {resync:?}"
        );
        assert_eq!(resync[0]["cursor"].as_u64().unwrap(), caught_up_at);

        // A second ack at the same cursor does not resync again.
        screen.ack("term-1", "client-one", caught_up_at);
        assert!(drain_pushes(&mut pushes, &session_key).is_empty());

        // And the stream is raw again.
        flush_chunk_acked_by(&mut screen, "client-one");
        let resumed = drain_pushes(&mut pushes, &session_key);
        assert_eq!(resumed.len(), 1, "{resumed:?}");
        assert_eq!(resumed[0]["type"], "term.output", "{resumed:?}");
    }

    /// Walk one client's frames as that client applies them, and return where
    /// its stream stands afterwards. A `term.reset` replaces the screen and
    /// moves the stream to its own cursor; a `term.output` must begin exactly
    /// where the stream stands, since raw bytes are applied on top of what the
    /// client already holds. A raw frame that starts behind the stream would be
    /// re-applied bytes, one that starts ahead of it a hole — both are the
    /// contiguity break the INVARIANT forbids.
    fn assert_stream_contiguous(frames: &[Value], start: u64) -> u64 {
        let mut applied = start;
        for frame in frames {
            let cursor = frame["cursor"].as_u64().unwrap();
            match frame["type"].as_str().unwrap() {
                "term.reset" => applied = cursor,
                "term.output" => {
                    let bytes = b64decode(frame["data"].as_str().unwrap()).len() as u64;
                    assert_eq!(
                        cursor - bytes,
                        applied,
                        "raw output must begin where the client's stream stands: {frame:?}"
                    );
                    applied = cursor;
                }
                other => panic!("unexpected frame while streaming: {other} in {frame:?}"),
            }
        }
        applied
    }

    /// PTY bytes arrive on their own channel, so an ack can land between a
    /// `process` and the flush that would have shipped it. The resync snapshot
    /// serializes the live screen, which already holds those bytes — so the
    /// next flush must not also hand them to the resumed client as raw output
    /// on top of the screen it just applied.
    #[test]
    fn an_ack_between_a_process_and_its_flush_does_not_replay_the_snapshotted_bytes() {
        let (mut screen, (mut resumed_pushes, resumed_key), (mut acking_pushes, acking_key)) =
            two_client_screen();

        screen.process(&chunk_of(ACK_TEST_CHUNK));
        screen.flush("term-1");
        screen.ack("term-1", "client-one", screen.total);
        screen.ack("term-1", "client-two", screen.total);
        for _ in 0..16 {
            flush_chunk_acked_by(&mut screen, "client-two");
        }
        drain_pushes(&mut resumed_pushes, &resumed_key);
        drain_pushes(&mut acking_pushes, &acking_key);
        let streams_stand_at = screen.total;

        // A chunk lands mid-cycle: processed, not yet flushed, when the paused
        // client's ack arrives and resyncs it.
        screen.process(b"mid-cycle");
        screen.ack("term-1", "client-one", screen.total);
        screen.process(b"after-resync");
        screen.flush("term-1");

        let resumed = drain_pushes(&mut resumed_pushes, &resumed_key);
        assert_eq!(
            assert_stream_contiguous(&resumed, streams_stand_at),
            screen.total,
            "the resumed client ends holding everything the bridge produced: {resumed:?}"
        );

        // The client that never paused keeps its own contiguous raw stream —
        // the mid-cycle bytes are shipped to it, not dropped on the floor.
        let acking = drain_pushes(&mut acking_pushes, &acking_key);
        assert_eq!(
            assert_stream_contiguous(&acking, streams_stand_at),
            screen.total,
            "the client that kept up misses nothing: {acking:?}"
        );
        assert!(
            output_text(&acking, "term-1").contains("mid-cycle"),
            "{acking:?}"
        );
    }

    /// A paused client receives nothing, so the highest cursor it can ever ack
    /// is the last frame it was sent before pausing. A sustained flood runs the
    /// live cursor far past that frame — measuring the resume against the live
    /// cursor would leave the client paused forever, with no frame in existence
    /// that could ever unpause it. Draining everything it was actually sent is
    /// all a paused client can do, and it must be enough: the resync snapshot
    /// covers the withheld gap by construction.
    #[test]
    fn a_paused_client_resumes_after_acking_all_it_was_sent_even_when_the_flood_ran_far_ahead() {
        let (mut screen, (mut pushes, session_key), _) = two_client_screen();

        screen.process(&chunk_of(ACK_TEST_CHUNK));
        screen.flush("term-1");
        screen.ack("term-1", "client-one", screen.total);
        screen.ack("term-1", "client-two", screen.total);

        // client-one goes silent; the flood runs 32 chunks (~3.2 MiB) — far
        // more than the unacked budget past anything client-one was sent.
        for _ in 0..32 {
            flush_chunk_acked_by(&mut screen, "client-two");
        }
        let sent = drain_pushes(&mut pushes, &session_key);
        let last_received = sent.last().unwrap()["cursor"].as_u64().unwrap();
        assert!(
            screen.total - last_received > TERM_UNACKED_BUDGET_BYTES,
            "the flood must outrun the paused client by more than the budget"
        );

        // It drains its queue and acks the last frame it was given — the
        // highest cursor it can ever report.
        screen.ack("term-1", "client-one", last_received);
        let resync = drain_pushes(&mut pushes, &session_key);
        assert_eq!(
            resync.len(),
            1,
            "draining everything sent earns the resync: {resync:?}"
        );
        assert_eq!(resync[0]["type"], "term.reset", "{resync:?}");
        assert_eq!(resync[0]["cursor"].as_u64().unwrap(), screen.total);

        // And the raw stream is back.
        flush_chunk_acked_by(&mut screen, "client-one");
        let resumed = drain_pushes(&mut pushes, &session_key);
        assert_eq!(resumed.len(), 1, "{resumed:?}");
        assert_eq!(resumed[0]["type"], "term.output", "{resumed:?}");
    }

    /// A client from before acks existed never sends one, and it must not be
    /// starved for that: with no ack to measure by, there is no evidence it is
    /// falling behind, so it keeps today's behaviour.
    #[test]
    fn a_client_that_never_acks_is_never_paused() {
        let (mut screen, (mut pushes, session_key), _) = two_client_screen();

        for _ in 0..16 {
            screen.process(&chunk_of(ACK_TEST_CHUNK));
            screen.flush("term-1");
        }

        let seen = drain_pushes(&mut pushes, &session_key);
        assert_eq!(
            seen.len(),
            16,
            "an ack-less client is fed exactly as it always was: {seen:?}"
        );
    }
}
