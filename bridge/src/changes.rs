//! Push invalidation: the bridge telling every connected browser that
//! something changed, instead of waiting to be polled.
//!
//! Two events go out on the browser's own session, in the frame shape it
//! already parses for terminal pushes (a payload with a `type`):
//!
//! ```text
//! {"type":"board.changed"}                    feed-level state moved
//! {"type":"entity.changed","id":"run-7"}      one entity's detail moved
//! ```
//!
//! `board.changed` says the feed is stale — a task lifecycle transition, the
//! inbox/attention map, a capture, an agent coming or going. `entity.changed`
//! says one issue/branch/run's thread, stages, git state or diff is stale. A
//! client that holds both refetches what it is showing; nothing about WHAT
//! changed rides the wire, so the events stay content-free like every other
//! signal Build sends about work it cannot read.
//!
//! **Terminal output is not a change.** It has its own push path
//! (`term.output` / `term.reset`), and routing a byte storm through here would
//! turn a repainting TUI into an invalidation storm.
//!
//! # The two halves, and why they are separate
//!
//! Noting a change happens deep inside mutations that run holding the app
//! mutex. Sending one encrypts a frame per subscriber. So [`ChangeBus::note`]
//! only inserts a key into a set behind a leaf mutex — no I/O, no encryption,
//! nothing that can block on anything but itself — and [`ChangeBus::flush`],
//! driven by a task that holds no app lock, does the sending.
//!
//! That split is also the coalescer. A flush collapses everything noted since
//! the last one into ONE event per key, and the driver
//! ([`ChangeBus::spawn_flusher`]) flushes at most once per
//! [`ChangeBus::window`]: the first change on an idle bus goes out at once,
//! and a burst behind it costs one event per key per window rather than one
//! per mutation.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Map, Value};
use tokio::time::Instant;

use crate::carrier::SessionSender;

/// How long changes collapse together before the next flush. Short enough that
/// a browser reacts as if it were watching, long enough that a mutation storm
/// (a stage sweep, an agent writing a file a second) costs a handful of frames.
pub const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_millis(250);

/// How long one entity's own event waits before it may be repeated.
///
/// The bus's window collapses a burst of notes into one event; this bounds how
/// often the SAME entity's event goes out at all, for the origin that fires on
/// every file an agent writes. A browser repaints a git surface off it, and a
/// second is as often as a human reads one. It is also the floor under a
/// realtime subscription's `git` and `files` items (wire spec, step 1.3).
pub const ENTITY_SETTLE_WINDOW: Duration = Duration::from_secs(1);

/// The most keys one un-flushed window holds before it gives up on precision.
///
/// Applies to the legacy key set and, separately, to each subscription's
/// pending map. Past the cap the whole batch collapses to
/// [`ChangeKey::Board`] / a bare board item, which already tells a client to
/// refetch everything.
const PENDING_KEY_CAP: usize = 512;

/// The most paths one `files` item names before it stops naming them. Past
/// this the item is `truncated`, which means "refetch the tree", not "these
/// paths".
pub const FILES_PER_FLUSH: usize = 200;

/// The most working-tree diff one `git` item carries.
///
/// A quarter of a megabyte is a large review diff and a small cache write.
/// Past it the item says how big the diff is and carries none of it, and the
/// client reads it when a reviewer opens the changes.
pub const WORKING_TREE_DIFF_MAX_BYTES: usize = 262_144;

/// The most conversation items one `thread` item carries.
///
/// A push is a cache write, and past a hundred rows the write is bigger than
/// the read that would replace it: the tip alone goes out, and the client
/// pages forward from the sequence it holds. An agent that says a hundred
/// things between two flushes is a harness in a storm, not a conversation.
pub const THREAD_PUSH_MAX_ITEMS: usize = 100;

/// The clamp on a `{"batch_ms": N}` mode, as the greeting advertises it.
pub const MIN_BATCH_MS: u64 = 1_000;
/// The upper end of that clamp: ten minutes.
pub const MAX_BATCH_MS: u64 = 600_000;

/// The entity id the feed itself takes in a `changes` frame's items. Not an
/// entity: the board's own row, carrying [`ChangeBus::board_revision`].
pub const BOARD_ITEM_ID: &str = "board";

/// The event type a subscription's frames carry.
pub const CHANGES_EVENT: &str = "changes";

/// The change events a browser session can be told about, announced in the
/// `session.hello` greeting so a client knows what it may hear. The first two
/// are the legacy pair; `changes` is what a subscription delivers.
pub const ANNOUNCED_EVENTS: [&str; 3] = ["board.changed", "entity.changed", CHANGES_EVENT];

// ------------------------------------------------------------ the wire ---

/// What moved about an entity, at the grain a subscription filters on.
///
/// Declaration order is the wire order of a `kinds` list, and `Ord` is what
/// makes [`KindSet`] deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Lifecycle, agent liveness, attention — what a row shows.
    State,
    /// Conversation items.
    Thread,
    /// Status, index, HEAD, refs.
    Git,
    /// Working-tree paths.
    Files,
    /// The tabs open in a checkout — the human's shells, coming and going.
    Terminals,
}

impl Kind {
    /// Every kind, in wire order.
    pub const ALL: [Kind; 5] = [
        Kind::State,
        Kind::Thread,
        Kind::Git,
        Kind::Files,
        Kind::Terminals,
    ];

    /// How the wire spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::State => "state",
            Kind::Thread => "thread",
            Kind::Git => "git",
            Kind::Files => "files",
            Kind::Terminals => "terminals",
        }
    }

    /// Whether this kind comes from the worktree — the two the settle window
    /// paces, and the two that need a filesystem watcher.
    pub fn is_worktree(self) -> bool {
        matches!(self, Kind::Git | Kind::Files)
    }
}

/// The kinds one subscription asked for. A set, so a client that names `git`
/// twice asked for it once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct KindSet(BTreeSet<Kind>);

impl KindSet {
    /// Every kind — what a focus-tier subscription asks for.
    pub fn all() -> KindSet {
        Kind::ALL.into_iter().collect()
    }

    pub fn contains(&self, kind: Kind) -> bool {
        self.0.contains(&kind)
    }

    pub fn insert(&mut self, kind: Kind) -> bool {
        self.0.insert(kind)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Kind> + '_ {
        self.0.iter().copied()
    }

    /// Whether any of these come from the worktree — whether this
    /// subscription is a reason to watch a checkout.
    pub fn needs_worktree(&self) -> bool {
        self.iter().any(Kind::is_worktree)
    }
}

impl FromIterator<Kind> for KindSet {
    fn from_iter<I: IntoIterator<Item = Kind>>(kinds: I) -> KindSet {
        KindSet(kinds.into_iter().collect())
    }
}

/// What a subscription is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The feed itself: `state` only, carrying the board revision.
    Board,
    /// One issue, run, or worktree.
    Entity(String),
    /// Every entity the board currently lists, tracked as the board changes.
    All,
}

/// `{"kind": "...", "id": "..."}` — the wire form of a [`Scope`]. A tagged
/// enum with a newtype variant is not something serde can derive, so the two
/// halves are written out.
#[derive(Debug, Deserialize, Serialize)]
struct ScopeWire {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

impl Scope {
    /// Whether an item about `entity_id` belongs in this scope. `All` covers
    /// anything noted rather than consulting the board on the note path: a
    /// note takes a leaf lock and returns, and an entity nobody notes is not
    /// one the board is waiting on. The board list is read where it has to be
    /// exact — [`ChangeBus::covered_worktrees`].
    fn covers(&self, entity_id: &str) -> bool {
        match self {
            Scope::Board => entity_id == BOARD_ITEM_ID,
            Scope::Entity(id) => id == entity_id,
            Scope::All => true,
        }
    }
}

impl Serialize for Scope {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Scope::Board => ScopeWire {
                kind: "board".into(),
                id: None,
            },
            Scope::Entity(id) => ScopeWire {
                kind: "entity".into(),
                id: Some(id.clone()),
            },
            Scope::All => ScopeWire {
                kind: "all".into(),
                id: None,
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Scope, D::Error> {
        let wire = ScopeWire::deserialize(deserializer)?;
        match (wire.kind.as_str(), wire.id) {
            ("board", _) => Ok(Scope::Board),
            ("all", _) => Ok(Scope::All),
            ("entity", Some(id)) => Ok(Scope::Entity(id)),
            ("entity", None) => Err(serde::de::Error::custom("missing required param: scope.id")),
            (other, _) => Err(serde::de::Error::custom(format!(
                "unknown scope kind {other:?} — board, entity, or all"
            ))),
        }
    }
}

/// How often a subscription hears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Flushed on the bus's own window; the first change on an idle
    /// subscription goes at once.
    #[default]
    Realtime,
    /// Once per interval, clamped to [`MIN_BATCH_MS`]..=[`MAX_BATCH_MS`].
    Batch(Duration),
    /// Kept, delivers nothing. Still accumulates, so the upsert that turns it
    /// back on has no gap in what it will report.
    Off,
}

/// `"realtime"` / `"off"` / `{"batch_ms": N}`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum ModeWire {
    Named(String),
    Batch { batch_ms: u64 },
}

impl Serialize for Mode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Mode::Realtime => ModeWire::Named("realtime".into()),
            Mode::Off => ModeWire::Named("off".into()),
            Mode::Batch(every) => ModeWire::Batch {
                batch_ms: every.as_millis() as u64,
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Mode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Mode, D::Error> {
        match ModeWire::deserialize(deserializer)? {
            ModeWire::Named(name) if name == "realtime" => Ok(Mode::Realtime),
            ModeWire::Named(name) if name == "off" => Ok(Mode::Off),
            ModeWire::Named(other) => Err(serde::de::Error::custom(format!(
                "unknown mode {other:?} — realtime, off, or {{\"batch_ms\": N}}"
            ))),
            // Clamped here rather than refused: a client asking for a faster
            // batch than the bridge serves gets the fastest it serves.
            ModeWire::Batch { batch_ms } => Ok(Mode::Batch(Duration::from_millis(
                batch_ms.clamp(MIN_BATCH_MS, MAX_BATCH_MS),
            ))),
        }
    }
}

/// Which flush a subscription rides in — the same type the dispatch queues are
/// keyed by, so a subscription's priority and the priority of the pulls a
/// client makes in response are one word. `Ord` is the flush order: foreground
/// never waits behind background.
pub use crate::timing::Priority;

/// Whether a subscription's worktree kinds come from a filesystem watcher or
/// from the TTL refresh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchState {
    /// A watcher is on every worktree in scope.
    #[default]
    Live,
    /// At least one worktree could not get one; its `git`/`files` items come
    /// from the TTL refresh instead.
    Polled,
}

/// One subscription, exactly as `changes.subscribe` states it and
/// `changes.list` answers it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SubscriptionSpec {
    #[serde(rename = "subscription_id")]
    pub id: String,
    pub scope: Scope,
    pub kinds: KindSet,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub priority: Priority,
}

/// What [`ChangeBus::subscribe`] answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscribeOutcome {
    pub watch: WatchState,
}

// ------------------------------------------------------- injected reads ---

/// Which of the lists a client caches whole a board change moved.
///
/// A row moving moves neither: the row rides its own `state` item, and the
/// board item says only that the feed's revision advanced. A project added
/// or a workspace created moves one of them, and the board item then carries
/// that list in full — the same answer `project.list` and `workspace.list`
/// give, because a list small enough to re-send whole is not worth a delta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardLists {
    pub projects: bool,
    pub workspaces: bool,
}

impl BoardLists {
    /// The project list moved: one was added, hidden, deleted, or renamed.
    pub const PROJECTS: BoardLists = BoardLists {
        projects: true,
        workspaces: false,
    };
    /// The workspace list moved: one was created, renamed, or deleted.
    pub const WORKSPACES: BoardLists = BoardLists {
        projects: false,
        workspaces: true,
    };

    pub fn is_empty(self) -> bool {
        !self.projects && !self.workspaces
    }

    fn merge(&mut self, other: BoardLists) {
        self.projects |= other.projects;
        self.workspaces |= other.workspaces;
    }
}

/// The board's current entity list, injected at construction so this module
/// never reaches into `AppState`. Only [`ChangeBus::covered_worktrees`] calls
/// it — the note path must stay a leaf-lock insert.
pub type BoardEntities = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// What one flush needs looked up for one entity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactsRequest {
    pub entity_id: String,
    /// `status_key` and `head`.
    pub git: bool,
    /// Each conversation's `last_sequence`.
    pub thread: bool,
    /// The row's lifecycle, agents and attention.
    pub state: bool,
    /// The checkout's open tabs.
    pub terminals: bool,
    /// The worktree's root listing.
    pub files: bool,
    /// Which whole lists the board item is to carry. Only ever set on a
    /// request for [`BOARD_ITEM_ID`].
    pub lists: BoardLists,
    /// The last sequence already sent for each agent, so a thread item
    /// carries what was said after it. An agent named here is one some
    /// subscription has heard about; one that is not gets its tip alone.
    pub thread_after: Vec<(String, u64)>,
}

/// One conversation's tail, as a `thread` item carries it: where the
/// conversation now stands, and what was said to get there.
///
/// `items` is empty and `since_sequence` null on the first flush a
/// subscription makes for an agent — the client's own sync has just read that
/// conversation — and again when a burst since the last flush is wider than
/// [`THREAD_PUSH_MAX_ITEMS`], which the client answers by paging forward from
/// the sequence it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ThreadTip {
    pub agent_id: String,
    pub last_sequence: u64,
    /// The conversation items after [`since_sequence`](Self::since_sequence),
    /// in the order they happened.
    #[serde(default)]
    pub items: Vec<Value>,
    /// The sequence `items` runs from, exclusive. `None` says the item
    /// carries no items to run from anything.
    #[serde(default)]
    pub since_sequence: Option<u64>,
}

/// What a flush learned about one entity, off the app lock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityFacts {
    pub entity_id: String,
    pub status_key: Option<String>,
    pub head: Option<String>,
    pub threads: Vec<ThreadTip>,
    /// The row's own state, as the board reads it — the same lifecycle,
    /// agent count and attention its board row carries. `None` (an entity the
    /// board has no row for) leaves the item's `state` an empty object, which
    /// still says "this moved, refetch".
    pub state: Option<Value>,
    /// The checkout's tab list, as `term.list` reads it. `None` leaves the
    /// item's `terminals` an empty object, which says the same.
    pub terminals: Option<Value>,
    /// The whole `git.status` shape the key was taken from.
    pub status: Option<Value>,
    /// The latest commits, as `git.log` answers them.
    pub log: Option<Value>,
    /// What this checkout holds that its publication base does not, as a
    /// commit list and a `diff_key` — never a patch.
    pub unpushed: Option<Value>,
    /// The working tree's own diff, the body `run.diff` / `worktree.diff`
    /// answers with. `None` beside a [`diff_bytes`](Self::diff_bytes) is a
    /// diff past [`WORKING_TREE_DIFF_MAX_BYTES`] — too big to push.
    pub diff: Option<Value>,
    /// How big that diff's patch is. `None` when the diff could not be read
    /// at all, which leaves both fields off the item.
    pub diff_bytes: Option<u64>,
    /// The worktree's top level, as `fs.tree` lists it for `path: ""`.
    /// Deeper directories are re-listed by the client: the bridge cannot know
    /// which ones a reader has walked into.
    pub root_listing: Option<Value>,
}

/// How a flush answers a batch of [`FactsRequest`]s. Runs on the flusher's
/// own `spawn_blocking`: a status walk per moved worktree and a short app-lock
/// lookup for the thread tails, never on the flusher's async worker and never
/// under a lock this module holds.
pub type FactsSource = Arc<dyn Fn(&[FactsRequest]) -> Vec<EntityFacts> + Send + Sync>;

// --------------------------------------------------------- legacy events ---

/// What changed, at the grain a browser refetches in.
///
/// `Ord` (and the variant order) is the wire order of one flush: the feed
/// first, then entities by id — so a client that reloads the feed and the
/// entity it is showing does it in that order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKey {
    /// Feed-level state: task lifecycle, inbox/attention, capture, agent
    /// liveness — anything the board reads.
    Board,
    /// One issue/branch/run: its thread, stages, git state or diff.
    Entity(String),
}

impl ChangeKey {
    /// The payload a browser receives for this key.
    pub fn payload(&self) -> Value {
        match self {
            ChangeKey::Board => json!({ "type": "board.changed" }),
            ChangeKey::Entity(id) => json!({ "type": "entity.changed", "id": id }),
        }
    }
}

/// What one un-flushed window holds, for the legacy events.
#[derive(Default)]
struct Pending {
    /// Noted since the last flush. A set, so a thousand notes of one key are
    /// one event.
    keys: BTreeSet<ChangeKey>,
    /// The subset of `keys` noted through a worktree kind, which may not go
    /// out again inside [`ENTITY_SETTLE_WINDOW`].
    settled: BTreeSet<ChangeKey>,
    /// Past [`PENDING_KEY_CAP`] the window gave up naming entities and stands
    /// as a bare [`ChangeKey::Board`] until it is flushed. Latched, so the
    /// notes that keep arriving cannot start refilling the set behind it.
    collapsed: bool,
}

// ------------------------------------------------------- subscriptions ---

/// One entity's un-flushed item, for one subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingItem {
    kinds: KindSet,
    paths: BTreeSet<String>,
    truncated: bool,
}

impl PendingItem {
    fn add_paths(&mut self, paths: &[String]) {
        for path in paths {
            if self.paths.len() >= FILES_PER_FLUSH && !self.paths.contains(path) {
                self.truncated = true;
                break;
            }
            self.paths.insert(path.clone());
        }
    }

    fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// Split into (what the settle window holds back, what may go now): the
    /// worktree kinds wait, `state` and `thread` never do.
    fn split_worktree(self) -> (PendingItem, PendingItem) {
        let mut held = PendingItem {
            paths: self.paths,
            truncated: self.truncated,
            ..PendingItem::default()
        };
        let mut due = PendingItem::default();
        for kind in self.kinds.iter() {
            if kind.is_worktree() {
                held.kinds.insert(kind);
            } else {
                due.kinds.insert(kind);
            }
        }
        (held, due)
    }
}

/// One live subscription: what it asked for, and what is waiting for it.
struct Subscription {
    spec: SubscriptionSpec,
    session: SessionSender,
    pending: BTreeMap<String, PendingItem>,
    collapsed: bool,
    created: Instant,
    last_flush: Option<Instant>,
    /// When each entity's worktree kinds last went out on THIS subscription —
    /// the settle floor, pruned on every flush so it never grows with the
    /// entities a bridge has seen.
    emitted_at: HashMap<String, Instant>,
    /// The last sequence this subscription has sent for each agent, so its
    /// next thread item carries what was said after it. Per subscription,
    /// because two subscriptions over one conversation have heard different
    /// amounts of it; bounded by the agents on the device.
    emitted_tips: HashMap<String, u64>,
    /// Which entities the board listed the last time this subscription was
    /// sent a board item — what the next one's `removed` is measured against.
    /// `None` until the first board item goes out: a subscription with no
    /// before has nothing to say about what left.
    covered_last: Option<BTreeSet<String>>,
}

impl Subscription {
    fn new(spec: SubscriptionSpec, session: SessionSender, now: Instant) -> Subscription {
        Subscription {
            spec,
            session,
            pending: BTreeMap::new(),
            collapsed: false,
            created: now,
            last_flush: None,
            emitted_at: HashMap::new(),
            emitted_tips: HashMap::new(),
            covered_last: None,
        }
    }

    fn wants(&self, entity_id: &str, kind: Kind) -> bool {
        self.spec.scope.covers(entity_id) && self.spec.kinds.contains(kind)
    }

    /// Insert into this subscription's own pending map. Coalescing is per
    /// subscription: a thousand notes of one entity are one item, and an
    /// hour spent `off` holds one item per entity, not an hour of history.
    fn note(&mut self, entity_id: &str, kind: Kind, paths: &[String]) {
        if self.collapsed {
            return;
        }
        if self.pending.len() >= PENDING_KEY_CAP && !self.pending.contains_key(entity_id) {
            self.collapse();
            return;
        }
        let item = self.pending.entry(entity_id.to_string()).or_default();
        item.kinds.insert(kind);
        if kind == Kind::Files {
            item.add_paths(paths);
        }
    }

    /// Past the cap: one bare board item, which says refetch everything.
    fn collapse(&mut self) {
        self.collapsed = true;
        self.pending.clear();
        let mut board = PendingItem::default();
        board.kinds.insert(Kind::State);
        self.pending.insert(BOARD_ITEM_ID.to_string(), board);
    }

    /// The earliest instant this subscription may send what it holds; `None`
    /// when it holds nothing or is `off`.
    fn due_at(&self, window: Duration, now: Instant) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        match self.spec.mode {
            Mode::Off => None,
            Mode::Batch(every) => Some(self.last_flush.unwrap_or(self.created) + every),
            Mode::Realtime => {
                let cadence = self.last_flush.map_or(now, |at| at + window);
                Some(cadence.max(self.earliest_release(now)))
            }
        }
    }

    /// When the settle floor stops holding every pending item back.
    fn earliest_release(&self, now: Instant) -> Instant {
        self.pending
            .iter()
            .map(|(id, item)| self.release_at(id, item, now))
            .min()
            .unwrap_or(now)
    }

    fn release_at(&self, entity_id: &str, item: &PendingItem, now: Instant) -> Instant {
        if item.kinds.iter().any(|kind| !kind.is_worktree()) {
            return now;
        }
        match self.emitted_at.get(entity_id) {
            Some(at) => *at + ENTITY_SETTLE_WINDOW,
            None => now,
        }
    }

    /// Take what may go out now, leaving what the settle floor holds back.
    /// `None` means nothing goes out on this turn.
    fn take_due(
        &mut self,
        window: Duration,
        now: Instant,
    ) -> Option<BTreeMap<String, PendingItem>> {
        if self.due_at(window, now)? > now {
            return None;
        }
        let paced = matches!(self.spec.mode, Mode::Realtime);
        let mut due = BTreeMap::new();
        for (id, item) in std::mem::take(&mut self.pending) {
            let (held, sending) = if paced && self.held_by_settle(&id, now) {
                item.split_worktree()
            } else {
                (PendingItem::default(), item)
            };
            if !held.is_empty() {
                self.pending.insert(id.clone(), held);
            }
            if !sending.is_empty() {
                due.insert(id, sending);
            }
        }
        if due.is_empty() {
            return None;
        }
        self.stamp(&due, now);
        Some(due)
    }

    /// Which entities have left the board since this subscription's last
    /// board item — and the note of what it covers now.
    ///
    /// `all` scope only: it is the only scope that hears every row, so it is
    /// the only one whose client holds rows that can go stale without a word.
    /// Empty until a first board item has gone out, because a subscription
    /// with no before cannot say what left.
    fn board_departures(
        &mut self,
        due: &BTreeMap<String, PendingItem>,
        covered: &BTreeSet<String>,
    ) -> Vec<String> {
        if self.spec.scope != Scope::All || !due.contains_key(BOARD_ITEM_ID) {
            return Vec::new();
        }
        let departed = match self.covered_last.replace(covered.clone()) {
            Some(before) => before.difference(covered).cloned().collect(),
            None => Vec::new(),
        };
        departed
    }

    fn held_by_settle(&self, entity_id: &str, now: Instant) -> bool {
        self.emitted_at
            .get(entity_id)
            .is_some_and(|at| now.duration_since(*at) < ENTITY_SETTLE_WINDOW)
    }

    /// Record the flush: the cadence clock, the settle floor, and the
    /// collapse latch, which the drained map releases.
    fn stamp(&mut self, due: &BTreeMap<String, PendingItem>, now: Instant) {
        self.last_flush = Some(now);
        for (id, item) in due {
            if item.kinds.needs_worktree() {
                self.emitted_at.insert(id.clone(), now);
            }
        }
        self.emitted_at
            .retain(|_, at| now.duration_since(*at) < ENTITY_SETTLE_WINDOW);
        if self.pending.is_empty() {
            self.collapsed = false;
        }
    }
}

/// One subscription's frame, ready to encrypt.
struct DueFrame {
    session: SessionSender,
    subscription_id: String,
    priority: Priority,
    items: BTreeMap<String, PendingItem>,
    /// What this subscription has already sent for each agent, empty when no
    /// item in the frame carries the `thread` kind.
    thread_after: Vec<(String, u64)>,
    /// The entities that have left the board since this subscription's last
    /// board item. Empty when nothing left, and when the frame carries no
    /// board item at all.
    removed: Vec<String>,
}

impl DueFrame {
    fn payload(&self, facts: &BTreeMap<&str, &EntityFacts>, board_revision: u64) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|(id, item)| {
                item_payload(
                    id,
                    item,
                    facts.get(id.as_str()).copied(),
                    board_revision,
                    self,
                )
            })
            .collect();
        json!({
            "type": CHANGES_EVENT,
            "subscription_id": self.subscription_id,
            "items": items,
        })
    }
}

/// One item of a `changes` frame: only the kinds that moved.
fn item_payload(
    entity_id: &str,
    item: &PendingItem,
    facts: Option<&EntityFacts>,
    board_revision: u64,
    frame: &DueFrame,
) -> Value {
    let mut out = Map::new();
    out.insert("entity_id".into(), json!(entity_id));
    if item.kinds.contains(Kind::State) {
        out.insert(
            "state".into(),
            state_payload(entity_id, facts, board_revision, frame),
        );
    }
    if item.kinds.contains(Kind::Thread) {
        out.insert("thread".into(), thread_payload(facts, &frame.thread_after));
    }
    if item.kinds.contains(Kind::Git) {
        out.insert("git".into(), git_payload(facts));
    }
    if item.kinds.contains(Kind::Files) {
        out.insert("files".into(), files_payload(item, facts));
    }
    if item.kinds.contains(Kind::Terminals) {
        out.insert("terminals".into(), terminals_payload(facts));
    }
    Value::Object(out)
}

/// One subscription's view of each conversation's tail.
///
/// The lookup read one window per entity, from the oldest cursor any due
/// subscription holds; this cuts that window down to what THIS subscription
/// has not been sent. A subscription with no cursor for an agent — its first
/// flush for that conversation — gets the tip alone, because the client has
/// just read the conversation for itself and the bridge has no idea how much
/// of it that read carried.
fn thread_payload(facts: Option<&EntityFacts>, thread_after: &[(String, u64)]) -> Value {
    let tips = facts.map(|f| f.threads.clone()).unwrap_or_default();
    let cut: Vec<ThreadTip> = tips
        .into_iter()
        .map(|tip| {
            let held = thread_after
                .iter()
                .find(|(agent_id, _)| *agent_id == tip.agent_id)
                .map(|(_, sequence)| *sequence);
            match (held, tip.since_sequence) {
                (Some(since), Some(_)) => ThreadTip {
                    items: tip.items.into_iter().filter(|i| !sent(i, since)).collect(),
                    since_sequence: Some(since),
                    ..tip
                },
                _ => ThreadTip {
                    items: Vec::new(),
                    since_sequence: None,
                    ..tip
                },
            }
        })
        .collect();
    json!(cut)
}

/// Whether this conversation item is one the subscription already has.
///
/// An item carries its sequence under `data`, the shape `thread.page`
/// answers in. One whose sequence cannot be read is kept: a duplicate the
/// client writes twice costs a write, and a hole costs it the conversation.
fn sent(item: &Value, since: u64) -> bool {
    item["data"]["sequence"]
        .as_u64()
        .is_some_and(|sequence| sequence <= since)
}

/// The board item carries the revision a client compares against, which
/// entities have left the board since this subscription's last one, and the
/// whole lists when the change that noted it moved one. An entity carries
/// whatever the facts source knew, and an empty object where it knew nothing
/// — which still says "this moved".
fn state_payload(
    entity_id: &str,
    facts: Option<&EntityFacts>,
    board_revision: u64,
    frame: &DueFrame,
) -> Value {
    if entity_id != BOARD_ITEM_ID {
        return facts
            .and_then(|f| f.state.clone())
            .unwrap_or_else(|| json!({}));
    }
    let mut state = Map::new();
    state.insert("revision".into(), json!(board_revision));
    if !frame.removed.is_empty() {
        state.insert("removed".into(), json!(frame.removed));
    }
    let lists = facts
        .and_then(|f| f.state.as_ref())
        .and_then(Value::as_object);
    for (list, value) in lists.into_iter().flatten() {
        state.insert(list.clone(), value.clone());
    }
    Value::Object(state)
}

/// The paths that moved, and the worktree's top level as the lookup listed
/// it. A root the lookup could not read is left off, which a client answers
/// the way it always has — by listing the tree itself.
fn files_payload(item: &PendingItem, facts: Option<&EntityFacts>) -> Value {
    let mut files = Map::new();
    files.insert("paths".into(), json!(item.paths));
    files.insert("truncated".into(), json!(item.truncated));
    if let Some(root) = facts.and_then(|f| f.root_listing.clone()) {
        files.insert("root".into(), root);
    }
    Value::Object(files)
}

/// The tab list the facts source read, or an empty object where it read
/// nothing — which still says "this moved".
fn terminals_payload(facts: Option<&EntityFacts>) -> Value {
    facts
        .and_then(|f| f.terminals.clone())
        .unwrap_or_else(|| json!({}))
}

/// Everything the lookup read about one checkout, each field left off when
/// it read nothing. A `git` item with none of them still says "this moved".
fn git_payload(facts: Option<&EntityFacts>) -> Value {
    let mut git = Map::new();
    let Some(facts) = facts else {
        return Value::Object(git);
    };
    for (field, value) in [
        ("status_key", facts.status_key.clone().map(Value::String)),
        ("head", facts.head.clone().map(Value::String)),
        ("status", facts.status.clone()),
        ("log", facts.log.clone()),
        ("unpushed", facts.unpushed.clone()),
    ] {
        if let Some(value) = value {
            git.insert(field.into(), value);
        }
    }
    // The pair is written together or not at all: `diff: null` means "too big
    // to push, here is how big", and a diff that could not be read at all
    // says nothing rather than saying zero.
    if let Some(bytes) = facts.diff_bytes {
        git.insert("diff".into(), facts.diff.clone().unwrap_or(Value::Null));
        git.insert("diff_bytes".into(), json!(bytes));
    }
    Value::Object(git)
}

/// Everything one flush turn will send, taken under the locks and sent with
/// them released.
#[derive(Default)]
struct Due {
    legacy: Vec<ChangeKey>,
    frames: Vec<DueFrame>,
    requests: Vec<FactsRequest>,
}

impl Due {
    fn is_empty(&self) -> bool {
        self.legacy.is_empty() && self.frames.is_empty()
    }
}

/// Every browser session that asked for push invalidation, and the changes
/// waiting to reach them.
pub struct ChangeBus {
    /// One entry per legacy subscriber, keyed by its session id. Dropped when
    /// a push fails: a sender that cannot send has no connection left.
    subscribers: Mutex<Vec<SessionSender>>,
    /// Every live subscription, in the order it was made — the flush order
    /// within one priority.
    subscriptions: Mutex<Vec<Subscription>>,
    pending: Mutex<Pending>,
    /// Set whenever anything gains its first key, so the flusher wakes on the
    /// change rather than on a tick.
    wake: tokio::sync::Notify,
    window: Duration,
    /// Monotonic, bumped by every [`ChangeBus::note_board`]: a client holding
    /// the same revision skips `board.list`.
    board_revision: AtomicU64,
    /// Worktrees whose watcher could not start; a subscription covering one
    /// answers `watch: "polled"`.
    polled: Mutex<BTreeSet<String>>,
    /// Which whole lists the board notes since the last flush moved. Latched
    /// rather than passed through, because the note that moves a list takes
    /// the same leaf lock every other note takes and the flush that carries
    /// it may be several notes later.
    pending_lists: Mutex<BoardLists>,
    board_entities: BoardEntities,
    /// `None` on a bus built without one — the unit tests, and any caller
    /// that wants the kinds without the keys. A flush then costs no lookup
    /// and no blocking thread at all.
    facts: Option<FactsSource>,
    /// When each key last reached a browser, pruned to
    /// [`ENTITY_SETTLE_WINDOW`] on every flush: an older entry can hold
    /// nothing back, so this never grows with the entities a bridge has seen.
    emitted_at: Mutex<HashMap<ChangeKey, tokio::time::Instant>>,
}

impl ChangeBus {
    /// A bus that coalesces over `window`, with no board list and no facts
    /// source: `{"kind":"all"}` covers what is noted, and items carry the
    /// kinds that moved without keys. What the daemon builds is
    /// [`ChangeBus::with_sources`].
    pub fn new(window: Duration) -> Arc<Self> {
        ChangeBus::build(window, Arc::new(Vec::new), None)
    }

    /// The daemon's bus: the board's entity list and the per-flush lookups,
    /// injected once here so this module never depends on `AppState`.
    pub fn with_sources(
        window: Duration,
        board_entities: BoardEntities,
        facts: FactsSource,
    ) -> Arc<Self> {
        ChangeBus::build(window, board_entities, Some(facts))
    }

    fn build(
        window: Duration,
        board_entities: BoardEntities,
        facts: Option<FactsSource>,
    ) -> Arc<Self> {
        Arc::new(ChangeBus {
            subscribers: Mutex::new(Vec::new()),
            subscriptions: Mutex::new(Vec::new()),
            pending: Mutex::new(Pending::default()),
            wake: tokio::sync::Notify::new(),
            window,
            board_revision: AtomicU64::new(0),
            polled: Mutex::new(BTreeSet::new()),
            pending_lists: Mutex::new(BoardLists::default()),
            board_entities,
            facts,
            emitted_at: Mutex::new(HashMap::new()),
        })
    }

    /// How long changes collapse together before the next flush.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// The board's current revision — what a board item reports.
    pub fn board_revision(&self) -> u64 {
        self.board_revision.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------- legacy path ---

    /// Start hearing the legacy events on this session (`session.hello` with
    /// no `changes` field, or `"changes": "legacy"`). Re-subscribing a session
    /// id replaces its sender — a reconnected browser keeps one subscription,
    /// not two.
    pub fn subscribe_legacy(&self, sender: &SessionSender) {
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|existing| existing.session_id() != sender.session_id());
        subscribers.push(sender.clone());
    }

    /// Stop hearing the legacy events, keeping any subscriptions: what
    /// `"changes": "subscriptions"` and the first `changes.subscribe` do.
    pub fn unsubscribe_legacy(&self, session_id: &str) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|existing| existing.session_id() != session_id);
    }

    /// Stop hearing anything — the session closed.
    pub fn unsubscribe(&self, session_id: &str) {
        self.unsubscribe_legacy(session_id);
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|sub| sub.session.session_id() != session_id);
    }

    /// How many sessions hear the legacy events.
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    // -------------------------------------------------------- the verbs ---

    /// Upsert one subscription for this session (`changes.subscribe`).
    ///
    /// Re-sending an id with a new mode is how a client changes cadence: the
    /// spec is replaced and the pending items are kept, so an `off`
    /// subscription turned back on has no gap in what it will report. The
    /// call also drops the session's legacy subscription — a session that
    /// subscribes has left legacy mode (step 1.5).
    pub fn subscribe(&self, session: &SessionSender, sub: SubscriptionSpec) -> SubscribeOutcome {
        self.unsubscribe_legacy(session.session_id());
        let watch = self.watch_state(&sub);
        let now = Instant::now();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        match subscriptions
            .iter_mut()
            .find(|existing| existing.is(session.session_id(), &sub.id))
        {
            Some(existing) => {
                existing.spec = sub;
                existing.session = session.clone();
            }
            None => subscriptions.push(Subscription::new(sub, session.clone(), now)),
        }
        // Only when it already holds something: an upsert off `off` has a
        // deadline the parked flusher has not seen, while a fresh empty
        // subscription would only wake it to find nothing.
        let holding = subscriptions
            .iter()
            .any(|existing| !existing.pending.is_empty());
        drop(subscriptions);
        if holding {
            self.wake.notify_one();
        }
        SubscribeOutcome { watch }
    }

    /// Drop one subscription (`changes.unsubscribe`).
    pub fn unsubscribe_one(&self, session_id: &str, sub_id: &str) {
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|sub| !sub.is(session_id, sub_id));
    }

    /// This session's subscriptions, in the order they were made
    /// (`changes.list`).
    pub fn list(&self, session_id: &str) -> Vec<SubscriptionSpec> {
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .filter(|sub| sub.session.session_id() == session_id)
            .map(|sub| sub.spec.clone())
            .collect()
    }

    /// Every entity id some live subscription covers with `git` or `files` —
    /// the worktrees that want a watcher. `{"kind":"all"}` is resolved here,
    /// against the board's current list.
    pub fn covered_worktrees(&self) -> BTreeSet<String> {
        let mut covered = BTreeSet::new();
        let subscriptions = self.subscriptions.lock().unwrap();
        let wants_all = subscriptions
            .iter()
            .any(|sub| sub.spec.kinds.needs_worktree() && sub.spec.scope == Scope::All);
        for sub in subscriptions.iter() {
            if let (true, Scope::Entity(id)) = (sub.spec.kinds.needs_worktree(), &sub.spec.scope) {
                covered.insert(id.clone());
            }
        }
        drop(subscriptions);
        if wants_all {
            covered.extend((self.board_entities)());
        }
        covered
    }

    /// A worktree whose watcher could not start: every subscription covering
    /// it answers `watch: "polled"` from now on.
    pub fn mark_polled(&self, entity_id: &str) {
        self.polled.lock().unwrap().insert(entity_id.to_string());
    }

    /// Its watcher started after all.
    pub fn clear_polled(&self, entity_id: &str) {
        self.polled.lock().unwrap().remove(entity_id);
    }

    /// What a subscription's `watch` answer is right now: `polled` when a
    /// worktree in its scope is marked so, `live` otherwise.
    pub fn watch_state(&self, sub: &SubscriptionSpec) -> WatchState {
        if !sub.kinds.needs_worktree() {
            return WatchState::Live;
        }
        let polled = self.polled.lock().unwrap();
        let covered = match &sub.scope {
            Scope::Entity(id) => polled.contains(id),
            Scope::All => !polled.is_empty(),
            Scope::Board => false,
        };
        if covered {
            WatchState::Polled
        } else {
            WatchState::Live
        }
    }

    // --------------------------------------------------------- the notes ---

    /// Record that something changed.
    ///
    /// SAFE UNDER THE APP MUTEX, and the reason this type exists: it takes one
    /// leaf mutex, inserts into a set, and returns. No encryption, no channel a
    /// slow reader can fill, no I/O of any kind — nothing a caller holding the
    /// app lock could block the whole daemon on.
    pub fn note(&self, key: ChangeKey) {
        let mut pending = self.pending.lock().unwrap();
        if pending.collapsed {
            return;
        }
        if pending.keys.len() >= PENDING_KEY_CAP {
            pending.collapsed = true;
            pending.keys.clear();
            pending.settled.clear();
            pending.keys.insert(ChangeKey::Board);
        } else {
            pending.keys.insert(key);
        }
        drop(pending);
        self.wake.notify_one();
    }

    /// One kind of one entity moved: the subscription path, and the legacy
    /// `entity.changed` beside it.
    pub fn note_kind(&self, entity_id: &str, kind: Kind) {
        self.note_subscriptions(entity_id, kind, &[]);
        self.note_legacy_entity(entity_id, kind);
    }

    /// This checkout's tab list moved.
    ///
    /// The subscription path only. The legacy events say "the feed is stale"
    /// and "this entity's detail is stale", and a shell opening in a checkout
    /// is neither — a legacy client reads its tabs off `term.list` when the
    /// human opens the console, never off the board.
    pub fn note_terminals(&self, entity_id: &str) {
        self.note_subscriptions(entity_id, Kind::Terminals, &[]);
    }

    /// These working-tree paths moved, relative to the worktree root.
    pub fn note_files(&self, entity_id: &str, paths: &[String]) {
        self.note_subscriptions(entity_id, Kind::Files, paths);
        self.note_legacy_entity(entity_id, Kind::Files);
    }

    /// The feed is stale: bump the revision a client compares against, note
    /// the board item for every subscription that watches the feed, and note
    /// the legacy `board.changed`.
    pub fn note_board(&self) {
        self.board_revision.fetch_add(1, Ordering::SeqCst);
        self.note_subscriptions(BOARD_ITEM_ID, Kind::State, &[]);
        self.note(ChangeKey::Board);
    }

    /// The feed moved, and so did one of the lists a client caches whole:
    /// the next board item carries that list in full.
    pub fn note_board_lists(&self, lists: BoardLists) {
        self.pending_lists.lock().unwrap().merge(lists);
        self.note_board();
    }

    /// This entity is stale — and so is the feed, which shows a row for it.
    ///
    /// Both the kinds an entity's own detail is made of: its row, and its
    /// conversation. That is what the legacy `entity.changed` beside it has
    /// always meant — "its thread, stages, git state or diff moved" — and it
    /// is the only origin a conversation has, since an item lands through
    /// the same mutation tail every other change does. A conversation that
    /// did not move costs its subscription an unchanged tip, which is what
    /// the tip is for.
    pub fn note_entity(&self, id: &str) {
        self.note_kind(id, Kind::State);
        self.note_kind(id, Kind::Thread);
        self.note_board();
    }

    /// This entity is stale, at the pace a browser can paint — its own event
    /// goes out at most once per [`ENTITY_SETTLE_WINDOW`], the feed at the
    /// bus's own window.
    ///
    /// For an origin that fires as fast as an agent writes files: the TTL
    /// refresh, which is the `git` kind arriving without a watcher. Every
    /// other caller wants [`note_entity`](Self::note_entity).
    pub fn note_entity_settled(&self, id: &str) {
        self.note_kind(id, Kind::Git);
    }

    /// The legacy half of a note: the entity's own event, paced by the settle
    /// window for the worktree kinds, plus the feed.
    fn note_legacy_entity(&self, entity_id: &str, kind: Kind) {
        let entity = ChangeKey::Entity(entity_id.to_string());
        if kind.is_worktree() {
            let mut pending = self.pending.lock().unwrap();
            if !pending.collapsed {
                pending.settled.insert(entity.clone());
            }
        }
        self.note(entity);
        self.note(ChangeKey::Board);
    }

    /// Resolve one note to every subscription that asked for it.
    fn note_subscriptions(&self, entity_id: &str, kind: Kind, paths: &[String]) {
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let mut noted = false;
        for sub in subscriptions
            .iter_mut()
            .filter(|s| s.wants(entity_id, kind))
        {
            sub.note(entity_id, kind, paths);
            noted = true;
        }
        drop(subscriptions);
        if noted {
            self.wake.notify_one();
        }
    }

    /// Whether anything is waiting to go out.
    pub fn has_pending(&self) -> bool {
        !self.pending.lock().unwrap().keys.is_empty()
            || self
                .subscriptions
                .lock()
                .unwrap()
                .iter()
                .any(|sub| !sub.pending.is_empty())
    }

    // -------------------------------------------------------- the flush ---

    /// Send everything due: one legacy event per distinct key noted since the
    /// last flush — less the settled keys still inside their window, which
    /// stay pending — and one `changes` frame per due subscription, foreground
    /// first. Drops every subscriber whose connection is gone. Returns the
    /// number of frames one subscriber was sent.
    ///
    /// MUST NOT run holding the app mutex: it encrypts a frame per subscriber.
    /// The facts source runs inline here; [`ChangeBus::run`] is the caller
    /// that runs it on `spawn_blocking`.
    pub fn flush(&self) -> usize {
        let due = self.take_due();
        if due.is_empty() {
            return 0;
        }
        let facts = self.look_up(&due.requests);
        self.deliver(due, facts)
    }

    /// Ask the injected source what the due items carry. Empty without one.
    fn look_up(&self, requests: &[FactsRequest]) -> Vec<EntityFacts> {
        match (&self.facts, requests.is_empty()) {
            (Some(source), false) => source(requests),
            _ => Vec::new(),
        }
    }

    /// The earliest instant any subscription wants to be flushed at.
    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .filter_map(|sub| sub.due_at(self.window, now))
            .min()
    }

    /// What this turn sends, taken under the locks. Foreground frames come
    /// first, so a background flush due in the same turn waits behind them.
    fn take_due(&self) -> Due {
        let now = Instant::now();
        let mut due = Due {
            legacy: self.take_due_keys(),
            ..Due::default()
        };
        // Read before the subscriptions lock: the board's list is somebody
        // else's leaf lock, and this module nests none.
        let covered: BTreeSet<String> = (self.board_entities)().into_iter().collect();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        for sub in subscriptions.iter_mut() {
            let Some(items) = sub.take_due(self.window, now) else {
                continue;
            };
            let removed = sub.board_departures(&items, &covered);
            let thread_after = match items.values().any(|item| item.kinds.contains(Kind::Thread)) {
                true => sub
                    .emitted_tips
                    .iter()
                    .map(|(a, s)| (a.clone(), *s))
                    .collect(),
                false => Vec::new(),
            };
            due.frames.push(DueFrame {
                session: sub.session.clone(),
                subscription_id: sub.spec.id.clone(),
                priority: sub.spec.priority,
                items,
                thread_after,
                removed,
            });
        }
        drop(subscriptions);
        due.frames.sort_by_key(|frame| frame.priority);
        due.requests = fact_requests(&due.frames);
        if let Some(request) = self.take_board_lists(&due.frames) {
            due.requests.push(request);
        }
        due
    }

    /// The lookup a board item's whole lists need, taken off the latch — and
    /// only when a board item is actually going out, so a list noted while
    /// every subscription was `off` still rides the flush that wakes.
    fn take_board_lists(&self, frames: &[DueFrame]) -> Option<FactsRequest> {
        if !frames
            .iter()
            .any(|frame| frame.items.contains_key(BOARD_ITEM_ID))
        {
            return None;
        }
        let lists = std::mem::take(&mut *self.pending_lists.lock().unwrap());
        if lists.is_empty() {
            return None;
        }
        Some(FactsRequest {
            entity_id: BOARD_ITEM_ID.to_string(),
            lists,
            ..FactsRequest::default()
        })
    }

    /// Encrypt and send. A subscription whose push fails takes every
    /// subscription of that session with it, as one failing push drops a
    /// legacy subscriber.
    fn deliver(&self, due: Due, facts: Vec<EntityFacts>) -> usize {
        let sent = self.deliver_legacy(&due.legacy);
        let by_entity: BTreeMap<&str, &EntityFacts> = facts
            .iter()
            .map(|fact| (fact.entity_id.as_str(), fact))
            .collect();
        let revision = self.board_revision();
        let mut dead: Vec<String> = Vec::new();
        let mut frames = 0;
        for frame in &due.frames {
            if frame.session.push(frame.payload(&by_entity, revision)) {
                frames += 1;
                self.stamp_thread_tips(frame, &by_entity);
            } else {
                dead.push(frame.session.session_id().to_string());
            }
        }
        for session_id in dead {
            self.unsubscribe(&session_id);
        }
        sent + frames
    }

    /// Record the conversations this frame just carried, so the next one
    /// carries what was said after them.
    ///
    /// HERE and not in [`Subscription::stamp`]: the tips are what the facts
    /// source answered, which is looked up after the items are taken and
    /// known only once the frame has gone out.
    fn stamp_thread_tips(&self, frame: &DueFrame, facts: &BTreeMap<&str, &EntityFacts>) {
        let sent: Vec<&ThreadTip> = frame
            .items
            .iter()
            .filter(|(_, item)| item.kinds.contains(Kind::Thread))
            .filter_map(|(id, _)| facts.get(id.as_str()))
            .flat_map(|fact| fact.threads.iter())
            .collect();
        if sent.is_empty() {
            return;
        }
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let Some(sub) = subscriptions
            .iter_mut()
            .find(|sub| sub.is(frame.session.session_id(), &frame.subscription_id))
        else {
            return;
        };
        for tip in sent {
            sub.emitted_tips
                .insert(tip.agent_id.clone(), tip.last_sequence);
        }
    }

    fn deliver_legacy(&self, keys: &[ChangeKey]) -> usize {
        if keys.is_empty() {
            return 0;
        }
        let payloads: Vec<Value> = keys.iter().map(ChangeKey::payload).collect();
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|subscriber| {
            for payload in &payloads {
                if !subscriber.push(payload.clone()) {
                    return false;
                }
            }
            true
        });
        payloads.len()
    }

    /// What this flush may send on the legacy path. A settled key a browser
    /// heard about inside [`ENTITY_SETTLE_WINDOW`] stays pending instead, and
    /// holding one back wakes the flusher so the next turn sends it.
    fn take_due_keys(&self) -> Vec<ChangeKey> {
        let mut pending = self.pending.lock().unwrap();
        if pending.keys.is_empty() {
            return Vec::new();
        }
        let (held, due) = self.split_off_unsettled(std::mem::take(&mut *pending));
        let holding_back = !held.is_empty();
        pending.settled.clone_from(&held);
        pending.keys = held;
        drop(pending);
        if holding_back {
            self.wake.notify_one();
        }
        due
    }

    /// The noted keys, split into the ones held back by the settle window and
    /// the ones due now — which are stamped as emitted on the way out.
    ///
    /// Pruning first is what makes `contains_key` mean "emitted inside the
    /// window", and what keeps the map bounded.
    fn split_off_unsettled(&self, noted: Pending) -> (BTreeSet<ChangeKey>, Vec<ChangeKey>) {
        let now = tokio::time::Instant::now();
        let mut emitted_at = self.emitted_at.lock().unwrap();
        emitted_at.retain(|_, at| now.duration_since(*at) < ENTITY_SETTLE_WINDOW);
        let (held, due): (BTreeSet<ChangeKey>, BTreeSet<ChangeKey>) = noted
            .keys
            .into_iter()
            .partition(|key| noted.settled.contains(key) && emitted_at.contains_key(key));
        for key in &due {
            emitted_at.insert(key.clone(), now);
        }
        (held, due.into_iter().collect())
    }

    /// Drive [`flush`](Self::flush) forever: wake on the first note or on the
    /// earliest subscription deadline, send, then hold the window open before
    /// the next send. Notes taken while the window is open leave the wake
    /// latched, so the flush after it goes out the instant the window closes.
    ///
    /// Spawned once per daemon, on a task that holds no app lock.
    pub async fn run(bus: Arc<Self>) {
        loop {
            bus.wait_for_work().await;
            ChangeBus::flush_off_thread(&bus).await;
            tokio::time::sleep(bus.window).await;
        }
    }

    /// Wake on a note, or on the earliest deadline any subscription set.
    async fn wait_for_work(&self) {
        match self.next_deadline() {
            Some(at) => {
                tokio::select! {
                    _ = self.wake.notified() => {}
                    _ = tokio::time::sleep_until(at) => {}
                }
            }
            None => self.wake.notified().await,
        }
    }

    /// [`flush`](Self::flush) with the per-entity lookups on their own
    /// blocking thread: a status walk per moved worktree and a short app-lock
    /// read for the thread tails must never run on the flusher's async
    /// worker.
    async fn flush_off_thread(bus: &Arc<Self>) -> usize {
        let due = bus.take_due();
        if due.is_empty() {
            return 0;
        }
        let facts = bus.gather(due.requests.clone()).await;
        bus.deliver(due, facts)
    }

    async fn gather(self: &Arc<Self>, requests: Vec<FactsRequest>) -> Vec<EntityFacts> {
        let Some(source) = self.facts.clone().filter(|_| !requests.is_empty()) else {
            return Vec::new();
        };
        tokio::task::spawn_blocking(move || source(&requests))
            .await
            .unwrap_or_default()
    }

    /// Start [`run`](Self::run) if there is a runtime to start it on. A bridge
    /// built outside one (the synchronous unit tests) simply never flushes; see
    /// [`PENDING_KEY_CAP`].
    pub fn spawn_flusher(bus: Arc<Self>) {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(ChangeBus::run(bus));
        }
    }
}

impl Subscription {
    fn is(&self, session_id: &str, sub_id: &str) -> bool {
        self.session.session_id() == session_id && self.spec.id == sub_id
    }
}

/// The cursors two subscriptions over one entity share for one lookup: the
/// OLDEST each agent has been read to, so the subscription furthest behind
/// gets everything it is missing. A subscription further ahead is handed
/// items it already holds, which a cache keyed by sequence writes twice and
/// reads once.
fn merge_thread_after(into: &mut Vec<(String, u64)>, tips: &[(String, u64)]) {
    for (agent_id, sequence) in tips {
        match into.iter_mut().find(|(held, _)| held == agent_id) {
            Some((_, held)) => *held = (*held).min(*sequence),
            None => into.push((agent_id.clone(), *sequence)),
        }
    }
}

/// What the frames due this turn need looked up, one request per entity.
fn fact_requests(frames: &[DueFrame]) -> Vec<FactsRequest> {
    let mut wanted: BTreeMap<&str, FactsRequest> = BTreeMap::new();
    for frame in frames {
        for (id, item) in &frame.items {
            if id == BOARD_ITEM_ID {
                continue;
            }
            let entry = wanted.entry(id.as_str()).or_insert_with(|| FactsRequest {
                entity_id: id.clone(),
                ..FactsRequest::default()
            });
            entry.git |= item.kinds.contains(Kind::Git);
            entry.thread |= item.kinds.contains(Kind::Thread);
            entry.state |= item.kinds.contains(Kind::State);
            entry.terminals |= item.kinds.contains(Kind::Terminals);
            entry.files |= item.kinds.contains(Kind::Files);
            if item.kinds.contains(Kind::Thread) {
                merge_thread_after(&mut entry.thread_after, &frame.thread_after);
            }
        }
    }
    wanted
        .into_values()
        .filter(|request| {
            request.git || request.thread || request.state || request.terminals || request.files
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Move a paused clock forward by `step` and let the flusher act on it.
    ///
    /// `advance` wakes the timers it passes; the yield is what gives the task
    /// they woke a turn to run before the assertion looks. Without it the test
    /// would race the scheduler instead of the clock — the same race, moved.
    pub(super) async fn settle(step: Duration) {
        tokio::time::advance(step).await;
        tokio::task::yield_now().await;
    }

    /// Drain everything a subscriber was pushed, decrypted.
    pub(super) fn drained(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
        key: &str,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(message) = rx.try_recv() {
            seen.push(SessionSender::decrypt_push(key, &message));
        }
        seen
    }

    #[test]
    fn a_noted_change_reaches_every_subscriber_on_flush() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (one, mut one_rx, one_key) = SessionSender::observable("s-one");
        let (two, mut two_rx, two_key) = SessionSender::observable("s-two");
        bus.subscribe_legacy(&one);
        bus.subscribe_legacy(&two);

        bus.note_board();
        assert_eq!(bus.flush(), 1);

        assert_eq!(
            drained(&mut one_rx, &one_key),
            vec![json!({ "type": "board.changed" })]
        );
        assert_eq!(
            drained(&mut two_rx, &two_key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    #[test]
    fn an_entity_change_names_the_entity_and_stales_the_feed() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);

        bus.note_entity("run-7");
        bus.flush();

        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ]
        );
    }

    #[test]
    fn a_burst_of_notes_collapses_to_one_event_per_key() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);

        for _ in 0..200 {
            bus.note_board();
            bus.note_entity("run-7");
            bus.note_entity("run-8");
        }
        assert_eq!(bus.flush(), 3, "one event per distinct key");

        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
                json!({ "type": "entity.changed", "id": "run-8" }),
            ]
        );
    }

    #[test]
    fn a_flush_with_nothing_noted_sends_nothing() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);

        assert_eq!(bus.flush(), 0);
        assert!(drained(&mut rx, &key).is_empty());
    }

    #[test]
    fn an_unsubscribed_session_hears_nothing_more() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);
        bus.unsubscribe("s-1");

        bus.note_board();
        bus.flush();

        assert!(drained(&mut rx, &key).is_empty());
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn resubscribing_a_session_id_keeps_one_subscription() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (first, _first_rx, _first_key) = SessionSender::observable("s-1");
        let (second, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&first);
        bus.subscribe_legacy(&second);

        bus.note_board();
        bus.flush();

        assert_eq!(bus.subscriber_count(), 1);
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    #[test]
    fn a_dead_subscriber_is_dropped() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, rx, _key) = SessionSender::observable("s-gone");
        bus.subscribe_legacy(&sender);
        drop(rx); // the connection went away

        bus.note_board();
        bus.flush();

        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn an_unflushed_window_past_the_cap_collapses_to_the_board() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);

        for n in 0..(PENDING_KEY_CAP + 50) {
            bus.note(ChangeKey::Entity(format!("run-{n}")));
        }
        assert_eq!(bus.flush(), 1);

        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })]
        );
    }

    /// An ordinary entity note keeps the bus's own window: two flushes, two
    /// events. Only the settled origin is paced.
    #[test]
    fn an_ordinary_entity_note_is_never_held_back() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);

        bus.note_entity("run-7");
        assert_eq!(bus.flush(), 2);
        bus.note_entity("run-7");
        assert_eq!(bus.flush(), 2);
    }

    /// The origin that fires on every file an agent writes: the entity's own
    /// event is repeated no more than once per settle window, while the feed
    /// keeps staling at the bus's window.
    #[tokio::test(start_paused = true)]
    async fn a_settled_entity_reaches_a_browser_once_per_settle_window() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);

        bus.note_entity_settled("run-7");
        bus.flush();
        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ]
        );

        bus.note_entity_settled("run-7");
        bus.flush();
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })],
            "the entity was heard about a moment ago"
        );
        assert!(bus.has_pending(), "and is still queued, not dropped");

        tokio::time::advance(ENTITY_SETTLE_WINDOW).await;
        bus.flush();
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "entity.changed", "id": "run-7" })]
        );
    }

    /// Nothing more is noted after the storm, so the held-back event only goes
    /// out if holding it back woke the flusher again.
    #[tokio::test(start_paused = true)]
    async fn a_held_back_entity_goes_out_when_its_window_closes() {
        let bus = ChangeBus::new(Duration::from_millis(150));
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);
        ChangeBus::spawn_flusher(Arc::clone(&bus));

        bus.note_entity_settled("run-7");
        settle(Duration::from_millis(30)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ]
        );

        for _ in 0..50 {
            bus.note_entity_settled("run-7");
        }
        settle(Duration::from_millis(200)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })],
            "the storm's entity event waits out the settle window"
        );

        settle(ENTITY_SETTLE_WINDOW).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "entity.changed", "id": "run-7" })]
        );
    }

    /// The driver's contract: the first change on an idle bus goes out at once,
    /// and everything noted behind it inside the window is one more flush, not
    /// one per mutation.
    ///
    /// On a paused clock, not a real one: the assertions are about which side
    /// of the window a flush falls on, and read against the wall clock they
    /// were a race — a 30 ms sleep that overran the 150 ms window under a
    /// loaded machine turned "the storm waits" into a failure about nothing.
    /// `advance` moves the clock by exactly what the contract talks about.
    #[tokio::test(start_paused = true)]
    async fn the_flusher_sends_at_most_one_batch_per_window() {
        let bus = ChangeBus::new(Duration::from_millis(150));
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);
        ChangeBus::spawn_flusher(Arc::clone(&bus));

        bus.note_board();
        settle(Duration::from_millis(30)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![json!({ "type": "board.changed" })],
            "an idle bus sends the first change straight away"
        );

        for _ in 0..500 {
            bus.note_board();
            bus.note_entity("run-7");
        }
        settle(Duration::from_millis(30)).await;
        assert!(
            drained(&mut rx, &key).is_empty(),
            "the window is still open — the storm waits"
        );

        settle(Duration::from_millis(200)).await;
        assert_eq!(
            drained(&mut rx, &key),
            vec![
                json!({ "type": "board.changed" }),
                json!({ "type": "entity.changed", "id": "run-7" }),
            ],
            "a thousand notes are two events"
        );
    }
}

#[cfg(test)]
mod subscriptions {
    use super::tests::{drained, settle};
    use super::*;

    /// A subscription over one entity, all kinds, at a cadence.
    fn spec(id: &str, scope: Scope, mode: Mode, priority: Priority) -> SubscriptionSpec {
        SubscriptionSpec {
            id: id.to_string(),
            scope,
            kinds: KindSet::all(),
            mode,
            priority,
        }
    }

    /// Only the `changes` frames out of a push history.
    fn frames(pushes: Vec<Value>) -> Vec<Value> {
        pushes
            .into_iter()
            .filter(|push| push["type"] == CHANGES_EVENT)
            .collect()
    }

    /// A bus with a flusher, and one session subscribed to nothing yet.
    fn bench() -> (
        Arc<ChangeBus>,
        SessionSender,
        tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
        String,
    ) {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, rx, key) = SessionSender::observable("s-1");
        ChangeBus::spawn_flusher(Arc::clone(&bus));
        (bus, sender, rx, key)
    }

    /// The realtime half of the spec's cadence check: a hundred notes are one
    /// item, delivered on the bus's own window.
    #[tokio::test(start_paused = true)]
    async fn a_realtime_subscription_hears_a_burst_as_one_item() {
        let (bus, sender, mut rx, key) = bench();
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        for _ in 0..100 {
            bus.note_kind("run-7", Kind::State);
        }
        settle(Duration::from_millis(30)).await;

        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-focus",
                "items": [{ "entity_id": "run-7", "state": {} }],
            })]
        );
    }

    /// The batch half: the same burst, on a subscription that asked for
    /// thirty seconds, is one item thirty seconds later — and nothing before.
    #[tokio::test(start_paused = true)]
    async fn a_batch_subscription_hears_the_same_burst_once_an_interval() {
        let (bus, sender, mut rx, key) = bench();
        bus.subscribe(
            &sender,
            spec(
                "s-bg",
                Scope::All,
                Mode::Batch(Duration::from_secs(30)),
                Priority::Background,
            ),
        );

        for _ in 0..100 {
            bus.note_kind("run-7", Kind::Git);
        }
        settle(Duration::from_millis(400)).await;
        assert!(
            frames(drained(&mut rx, &key)).is_empty(),
            "the interval has not passed"
        );

        settle(Duration::from_secs(30)).await;
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-bg",
                "items": [{ "entity_id": "run-7", "git": {} }],
            })]
        );
    }

    /// `off` keeps the subscription and keeps accumulating; the upsert that
    /// turns it back on has no gap in what it reports.
    #[tokio::test(start_paused = true)]
    async fn an_off_subscription_delivers_nothing_until_the_upsert() {
        let (bus, sender, mut rx, key) = bench();
        let off = spec(
            "s-idle",
            Scope::Entity("run-7".into()),
            Mode::Off,
            Priority::Background,
        );
        bus.subscribe(&sender, off.clone());

        bus.note_kind("run-7", Kind::Thread);
        settle(Duration::from_secs(5)).await;
        assert!(frames(drained(&mut rx, &key)).is_empty());

        bus.subscribe(
            &sender,
            SubscriptionSpec {
                mode: Mode::Batch(Duration::from_secs(1)),
                ..off
            },
        );
        settle(Duration::from_secs(2)).await;
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-idle",
                "items": [{ "entity_id": "run-7", "thread": [] }],
            })],
            "the item it held through the off window"
        );
        assert_eq!(bus.list("s-1").len(), 1, "off never dropped it");
    }

    /// Two subscriptions due in the same turn: the foreground one is pushed
    /// first, whatever order they were made in.
    #[tokio::test(start_paused = true)]
    async fn a_foreground_flush_precedes_a_background_one_due_in_the_same_turn() {
        let (bus, sender, mut rx, key) = bench();
        bus.subscribe(
            &sender,
            spec("s-bg", Scope::All, Mode::Realtime, Priority::Background),
        );
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_kind("run-7", Kind::State);
        settle(Duration::from_millis(30)).await;

        let sent = frames(drained(&mut rx, &key));
        let order: Vec<&str> = sent
            .iter()
            .map(|frame| frame["subscription_id"].as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["s-focus", "s-bg"]);
    }

    /// The TTL refresh is the `git` kind arriving without a watcher: its
    /// `note_entity_settled` lands as a `git` item — and only that — on a
    /// subscription that asked for git, so a polled worktree feeds the same
    /// subscriptions a watched one does.
    #[test]
    fn the_ttl_refresh_feeds_a_git_subscription() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Git].into_iter().collect(),
                ..spec(
                    "s-git",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );
        bus.note_entity_settled("run-7");
        bus.flush();
        let frames = frames(drained(&mut rx, &key));
        assert_eq!(frames.len(), 1, "{frames:?}");
        let item = &frames[0]["items"][0];
        assert_eq!(item["entity_id"], "run-7");
        assert!(item.get("git").is_some(), "{item:?}");
        assert!(item.get("state").is_none(), "{item:?}");
    }

    /// The settle floor under a realtime worktree item: an agent writing a
    /// file a second costs one item a second.
    #[tokio::test(start_paused = true)]
    async fn the_settle_window_floors_a_realtime_git_item() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_kind("run-7", Kind::Git);
        assert_eq!(bus.flush(), 3, "two legacy events and one frame");
        assert_eq!(frames(drained(&mut rx, &key)).len(), 1);

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-7", Kind::Git);
        bus.flush();
        assert!(
            frames(drained(&mut rx, &key)).is_empty(),
            "inside the settle window"
        );

        settle(ENTITY_SETTLE_WINDOW).await;
        bus.flush();
        assert_eq!(frames(drained(&mut rx, &key)).len(), 1);
    }

    /// `state` and `thread` are never paced by the settle floor — only the
    /// worktree kinds are, and the rest of the item goes without them.
    #[tokio::test(start_paused = true)]
    async fn a_held_git_item_does_not_hold_its_own_state_back() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_kind("run-7", Kind::Git);
        bus.flush();
        drained(&mut rx, &key);

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-7", Kind::Git);
        bus.note_kind("run-7", Kind::State);
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-focus",
                "items": [{ "entity_id": "run-7", "state": {} }],
            })],
            "the state half goes now, the git half waits"
        );
    }

    /// A `files` item names deduped paths, and stops naming them past the cap.
    #[test]
    fn a_files_item_dedupes_and_truncates() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_files("run-7", &["src/lib.rs".into(), "src/lib.rs".into()]);
        bus.flush();
        let sent = frames(drained(&mut rx, &key));
        assert_eq!(
            sent[0]["items"][0]["files"],
            json!({ "paths": ["src/lib.rs"], "truncated": false })
        );

        let many: Vec<String> = (0..FILES_PER_FLUSH + 5).map(|n| format!("f{n}")).collect();
        bus.note_files("run-7", &many);
        let item = &bus.subscriptions.lock().unwrap()[0].pending["run-7"];
        assert_eq!(item.paths.len(), FILES_PER_FLUSH);
        assert!(item.truncated, "past the cap the list means refetch");
    }

    /// The board item carries the revision a client compares against, and
    /// every `note_board` bumps it.
    #[test]
    fn the_board_item_carries_the_revision() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec(
                    "s-board",
                    Scope::Board,
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_board();
        bus.note_board();
        bus.flush();

        assert_eq!(bus.board_revision(), 2);
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-board",
                "items": [{ "entity_id": "board", "state": { "revision": 2 } }],
            })]
        );
    }

    /// A subscription hears its own scope and its own kinds, and nothing else.
    #[test]
    fn a_subscription_hears_only_what_it_asked_for() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Git].into_iter().collect(),
                ..spec(
                    "s-git",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_kind("run-7", Kind::Thread);
        bus.note_kind("run-8", Kind::Git);
        bus.note_board();
        bus.flush();

        assert!(
            frames(drained(&mut rx, &key)).is_empty(),
            "another entity, another kind, and the feed are all out of scope"
        );
    }

    /// Per-subscription coalescing has its own cap, and past it the
    /// subscription stands as one bare board item until it is flushed.
    #[test]
    fn a_subscription_past_the_cap_collapses_to_a_board_item() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            spec("s-all", Scope::All, Mode::Realtime, Priority::Background),
        );

        for n in 0..(PENDING_KEY_CAP + 50) {
            bus.note_kind(&format!("run-{n}"), Kind::State);
        }
        bus.flush();

        let sent = frames(drained(&mut rx, &key));
        assert_eq!(sent[0]["items"].as_array().unwrap().len(), 1);
        assert_eq!(sent[0]["items"][0]["entity_id"], "board");
    }

    /// What the watcher asks for: every worktree a live subscription covers
    /// with a worktree kind, with `all` resolved against the board's list.
    #[test]
    fn covered_worktrees_resolves_all_against_the_board() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(|| vec!["run-1".to_string(), "run-2".to_string()]),
            Arc::new(|_| Vec::new()),
        );
        let (sender, _rx, _key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec(
                    "s-state",
                    Scope::Entity("run-9".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );
        assert!(
            bus.covered_worktrees().is_empty(),
            "a state-only subscription is no reason to watch a checkout"
        );

        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );
        bus.subscribe(
            &sender,
            spec(
                "s-bg",
                Scope::All,
                Mode::Batch(Duration::from_secs(30)),
                Priority::Background,
            ),
        );
        assert_eq!(
            bus.covered_worktrees(),
            ["run-1", "run-2", "run-7"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    /// A worktree with no watcher answers `polled`, and its subscription is
    /// still served — from the TTL refresh's own notes.
    #[test]
    fn a_worktree_without_a_watcher_answers_polled() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, _rx, _key) = SessionSender::observable("s-1");
        let sub = spec(
            "s-focus",
            Scope::Entity("run-7".into()),
            Mode::Realtime,
            Priority::Foreground,
        );

        bus.mark_polled("run-7");
        assert_eq!(
            bus.subscribe(&sender, sub.clone()),
            SubscribeOutcome {
                watch: WatchState::Polled
            }
        );
        bus.clear_polled("run-7");
        assert_eq!(
            bus.subscribe(&sender, sub),
            SubscribeOutcome {
                watch: WatchState::Live
            }
        );
    }

    /// Subscribing is how a session leaves legacy mode; the two contracts are
    /// never served to one session at once.
    #[test]
    fn subscribing_drops_the_sessions_legacy_events() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe_legacy(&sender);
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_entity("run-7");
        bus.flush();

        let seen = drained(&mut rx, &key);
        assert_eq!(bus.subscriber_count(), 0);
        assert!(
            seen.iter().all(|push| push["type"] == CHANGES_EVENT),
            "{seen:?}"
        );
    }

    /// `changes.list` and `changes.unsubscribe`, per session.
    #[test]
    fn a_session_lists_and_drops_its_own_subscriptions() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (mine, _rx, _key) = SessionSender::observable("s-1");
        let (theirs, _rx2, _key2) = SessionSender::observable("s-2");
        let focus = spec(
            "s-focus",
            Scope::Entity("run-7".into()),
            Mode::Realtime,
            Priority::Foreground,
        );
        bus.subscribe(&mine, focus.clone());
        bus.subscribe(
            &mine,
            spec("s-bg", Scope::All, Mode::Off, Priority::Background),
        );
        bus.subscribe(&theirs, focus);

        assert_eq!(
            bus.list("s-1")
                .iter()
                .map(|sub| sub.id.clone())
                .collect::<Vec<_>>(),
            vec!["s-focus", "s-bg"]
        );

        bus.unsubscribe_one("s-1", "s-bg");
        assert_eq!(bus.list("s-1").len(), 1);
        bus.unsubscribe("s-1");
        assert!(bus.list("s-1").is_empty());
        assert_eq!(bus.list("s-2").len(), 1, "another session is untouched");
    }

    /// An upsert replaces the spec and keeps one subscription, which is how a
    /// client changes cadence.
    #[test]
    fn an_upsert_replaces_the_spec_in_place() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, _rx, _key) = SessionSender::observable("s-1");
        let focus = spec(
            "s-focus",
            Scope::Entity("run-7".into()),
            Mode::Realtime,
            Priority::Foreground,
        );
        bus.subscribe(&sender, focus.clone());
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                mode: Mode::Batch(Duration::from_secs(30)),
                ..focus
            },
        );

        let listed = bus.list("s-1");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].mode, Mode::Batch(Duration::from_secs(30)));
    }

    /// A push that fails takes every subscription of that session with it.
    #[test]
    fn a_dead_session_loses_every_subscription() {
        let bus = ChangeBus::new(DEFAULT_COALESCE_WINDOW);
        let (sender, rx, _key) = SessionSender::observable("s-gone");
        bus.subscribe(
            &sender,
            spec("s-all", Scope::All, Mode::Realtime, Priority::Foreground),
        );
        drop(rx);

        bus.note_kind("run-7", Kind::State);
        bus.flush();

        assert!(bus.list("s-gone").is_empty());
    }

    /// What the flush looks up off the app lock ends up on the item: the
    /// status key a client compares against, HEAD, and each conversation's
    /// tail.
    #[test]
    fn the_facts_source_fills_the_keys_a_client_compares() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                requests
                    .iter()
                    .map(|request| EntityFacts {
                        entity_id: request.entity_id.clone(),
                        status_key: request.git.then(|| "9f3c1a0b7e2d4c55".to_string()),
                        head: request.git.then(|| "a1b2c3d".to_string()),
                        threads: vec![ThreadTip {
                            agent_id: "agent-3".into(),
                            last_sequence: 412,
                            ..ThreadTip::default()
                        }],
                        ..EntityFacts::default()
                    })
                    .collect()
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            spec(
                "s-focus",
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            ),
        );

        bus.note_kind("run-7", Kind::Git);
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();

        let sent = frames(drained(&mut rx, &key));
        assert_eq!(
            sent[0]["items"][0],
            json!({
                "entity_id": "run-7",
                "thread": [{
                    "agent_id": "agent-3",
                    "last_sequence": 412,
                    "items": [],
                    "since_sequence": null,
                }],
                "git": { "status_key": "9f3c1a0b7e2d4c55", "head": "a1b2c3d" },
            })
        );
    }

    /// The spec as the wire spells it, both ways.
    #[test]
    fn a_subscription_spec_round_trips_through_the_wire_form() {
        let wire = json!({
            "subscription_id": "s-focus",
            "scope": { "kind": "entity", "id": "run-7" },
            "kinds": ["state", "thread", "git", "files", "terminals"],
            "mode": "realtime",
            "priority": "foreground",
        });
        let parsed: SubscriptionSpec = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(parsed.scope, Scope::Entity("run-7".into()));
        assert_eq!(parsed.kinds, KindSet::all());
        assert_eq!(serde_json::to_value(&parsed).unwrap(), wire);
    }

    /// The defaults and the clamp: a mode is realtime and a priority is
    /// foreground unless the client says otherwise, and a batch interval
    /// outside the advertised range is clamped, never refused.
    #[test]
    fn a_batch_interval_is_clamped_to_the_advertised_range() {
        let parse = |mode: Value| -> SubscriptionSpec {
            serde_json::from_value(json!({
                "subscription_id": "s",
                "scope": { "kind": "all" },
                "kinds": ["git"],
                "mode": mode,
            }))
            .unwrap()
        };
        assert_eq!(
            parse(json!({ "batch_ms": 5 })).mode,
            Mode::Batch(Duration::from_millis(MIN_BATCH_MS))
        );
        assert_eq!(
            parse(json!({ "batch_ms": 9_999_999 })).mode,
            Mode::Batch(Duration::from_millis(MAX_BATCH_MS))
        );
        assert_eq!(parse(json!("off")).mode, Mode::Off);
        assert_eq!(
            parse(json!({ "batch_ms": 30_000 })).priority,
            Priority::Foreground
        );
    }

    /// An unknown scope or mode is refused by name rather than silently
    /// meaning something else.
    #[test]
    fn an_unknown_scope_or_mode_is_named_in_the_refusal() {
        let scope: Result<Scope, _> = serde_json::from_value(json!({ "kind": "galaxy" }));
        assert!(scope
            .unwrap_err()
            .to_string()
            .contains("unknown scope kind"));
        let missing: Result<Scope, _> = serde_json::from_value(json!({ "kind": "entity" }));
        assert!(missing
            .unwrap_err()
            .to_string()
            .contains("missing required param: scope.id"));
        let mode: Result<Mode, _> = serde_json::from_value(json!("hourly"));
        assert!(mode.unwrap_err().to_string().contains("unknown mode"));
    }

    /// A `terminals` item carries the tab list the facts source read, so a
    /// client repaints its tab row off the push rather than calling
    /// `term.list` behind it.
    #[test]
    fn a_terminals_item_carries_the_tab_list() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                requests
                    .iter()
                    .map(|request| EntityFacts {
                        entity_id: request.entity_id.clone(),
                        terminals: request
                            .terminals
                            .then(|| json!({ "tabs": [{ "term_id": "term-1", "kind": "shell" }] })),
                        ..EntityFacts::default()
                    })
                    .collect()
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Terminals].into_iter().collect(),
                ..spec(
                    "s-tabs",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_kind("run-7", Kind::Terminals);
        bus.flush();

        let sent = frames(drained(&mut rx, &key));
        assert_eq!(
            sent[0]["items"][0],
            json!({
                "entity_id": "run-7",
                "terminals": { "tabs": [{ "term_id": "term-1", "kind": "shell" }] },
            })
        );
    }

    /// The new kind is one of the five the wire names, and it is not a
    /// worktree kind: a tab list is not something a filesystem watcher sees,
    /// and the settle floor under the git surfaces has nothing to say about
    /// it.
    #[test]
    fn terminals_is_a_kind_the_wire_names_and_no_watcher_serves() {
        assert!(Kind::ALL.contains(&Kind::Terminals));
        assert_eq!(Kind::Terminals.as_str(), "terminals");
        assert!(!Kind::Terminals.is_worktree());
        let parsed: KindSet = serde_json::from_value(json!(["terminals"])).unwrap();
        assert!(parsed.contains(Kind::Terminals));
        assert!(!parsed.needs_worktree());
    }

    /// A conversation's items ride the push. The first flush an agent appears
    /// in carries the tip alone — the client's own sync has just read that
    /// conversation — and every flush after it carries what was said since
    /// the last one this subscription sent.
    #[tokio::test(start_paused = true)]
    async fn a_thread_item_carries_what_was_said_since_the_last_flush() {
        /// The cursors each lookup was asked with, flush by flush.
        type Asked = Arc<Mutex<Vec<Vec<(String, u64)>>>>;
        let seen: Asked = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(move |requests: &[FactsRequest]| {
                recorder
                    .lock()
                    .unwrap()
                    .push(requests[0].thread_after.clone());
                let since = requests[0]
                    .thread_after
                    .iter()
                    .find(|(agent, _)| agent == "a1")
                    .map(|(_, sequence)| *sequence);
                let items = match since {
                    Some(since) => ((since + 1)..=812)
                        .map(|n| json!({ "type": "message", "data": { "sequence": n } }))
                        .collect(),
                    None => Vec::new(),
                };
                vec![EntityFacts {
                    entity_id: requests[0].entity_id.clone(),
                    threads: vec![ThreadTip {
                        agent_id: "a1".into(),
                        last_sequence: 812,
                        items,
                        since_sequence: since,
                    }],
                    ..EntityFacts::default()
                }]
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Thread].into_iter().collect(),
                ..spec(
                    "s-thread",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["thread"],
            json!([{
                "agent_id": "a1",
                "last_sequence": 812,
                "items": [],
                "since_sequence": null,
            }]),
            "the first flush says where the conversation stands, nothing more"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["thread"][0]["since_sequence"],
            json!(812),
            "the second asks for what was said after the tip the first sent"
        );
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![Vec::new(), vec![("a1".to_string(), 812)]]
        );
    }

    /// Two subscriptions over the same conversation hold their own cursors:
    /// one that has heard nothing is not caught up by the other's flush.
    #[tokio::test(start_paused = true)]
    async fn each_subscription_carries_its_own_thread_cursor() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                vec![EntityFacts {
                    entity_id: requests[0].entity_id.clone(),
                    threads: vec![ThreadTip {
                        agent_id: "a1".into(),
                        last_sequence: 5,
                        items: Vec::new(),
                        since_sequence: requests[0]
                            .thread_after
                            .iter()
                            .find(|(agent, _)| agent == "a1")
                            .map(|(_, sequence)| *sequence),
                    }],
                    ..EntityFacts::default()
                }]
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        let thread_spec = |id: &str| SubscriptionSpec {
            kinds: [Kind::Thread].into_iter().collect(),
            ..spec(
                id,
                Scope::Entity("run-7".into()),
                Mode::Realtime,
                Priority::Foreground,
            )
        };
        bus.subscribe(&sender, thread_spec("s-first"));
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        drained(&mut rx, &key);

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.subscribe(&sender, thread_spec("s-second"));
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();

        let sent = frames(drained(&mut rx, &key));
        let cursor = |id: &str| {
            sent.iter()
                .find(|frame| frame["subscription_id"] == id)
                .map(|frame| frame["items"][0]["thread"][0]["since_sequence"].clone())
                .unwrap_or_else(|| panic!("no frame for {id}: {sent:?}"))
        };
        assert_eq!(cursor("s-first"), json!(5), "it heard the tip already");
        assert_eq!(cursor("s-second"), json!(null), "this one never has");
    }

    /// A `git` item carries the surfaces themselves — the status shape, the
    /// latest commits, what is unpublished, and the working tree's diff —
    /// so a client writes them into its cache and calls nothing back.
    #[test]
    fn a_git_item_carries_the_shapes_a_client_would_have_asked_for() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                vec![EntityFacts {
                    entity_id: requests[0].entity_id.clone(),
                    status_key: Some("9f3c1a0b7e2d4c55".into()),
                    head: Some("a1b2c3d".into()),
                    status: Some(json!({ "branch": "build/x", "files": [] })),
                    log: Some(json!({ "commits": [{ "hash": "a1b2c3d" }], "newest": "a1b2c3d" })),
                    unpushed: Some(json!({
                        "base": { "kind": "push_target", "label": "origin/build/x" },
                        "commits": [{ "hash": "a1b2c3d", "subject": "do it" }],
                        "diff_key": "77aa11bb",
                    })),
                    diff: Some(json!({ "stat": {}, "files": [], "patch": "" })),
                    diff_bytes: Some(41),
                    ..EntityFacts::default()
                }]
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Git].into_iter().collect(),
                ..spec(
                    "s-git",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_kind("run-7", Kind::Git);
        bus.flush();

        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0],
            json!({
                "entity_id": "run-7",
                "git": {
                    "status_key": "9f3c1a0b7e2d4c55",
                    "head": "a1b2c3d",
                    "status": { "branch": "build/x", "files": [] },
                    "log": { "commits": [{ "hash": "a1b2c3d" }], "newest": "a1b2c3d" },
                    "unpushed": {
                        "base": { "kind": "push_target", "label": "origin/build/x" },
                        "commits": [{ "hash": "a1b2c3d", "subject": "do it" }],
                        "diff_key": "77aa11bb",
                    },
                    "diff": { "stat": {}, "files": [], "patch": "" },
                    "diff_bytes": 41,
                },
            })
        );
    }

    /// A working tree too big to push says so: the item names the size and
    /// carries no diff, and the client asks for it when a reviewer opens it.
    #[test]
    fn a_git_item_past_the_diff_cap_carries_the_size_and_no_diff() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                vec![EntityFacts {
                    entity_id: requests[0].entity_id.clone(),
                    diff: None,
                    diff_bytes: Some(WORKING_TREE_DIFF_MAX_BYTES as u64 + 1),
                    ..EntityFacts::default()
                }]
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Git].into_iter().collect(),
                ..spec(
                    "s-git",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_kind("run-7", Kind::Git);
        bus.flush();

        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["git"],
            json!({
                "diff": null,
                "diff_bytes": WORKING_TREE_DIFF_MAX_BYTES + 1,
            })
        );
    }

    /// A `files` item carries the root listing beside the paths that moved:
    /// the client repaints the file tree's top level off the push, and
    /// re-lists only the deeper directories it is holding open.
    #[test]
    fn a_files_item_carries_the_root_listing() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                vec![EntityFacts {
                    entity_id: requests[0].entity_id.clone(),
                    root_listing: requests[0].files.then(
                        || json!({ "path": "", "entries": [{ "name": "src", "kind": "dir" }] }),
                    ),
                    ..EntityFacts::default()
                }]
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Files].into_iter().collect(),
                ..spec(
                    "s-files",
                    Scope::Entity("run-7".into()),
                    Mode::Realtime,
                    Priority::Foreground,
                )
            },
        );

        bus.note_files("run-7", &["src/lib.rs".to_string()]);
        bus.flush();

        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["files"],
            json!({
                "paths": ["src/lib.rs"],
                "truncated": false,
                "root": { "path": "", "entries": [{ "name": "src", "kind": "dir" }] },
            })
        );
    }

    /// The board item says which entities left the board, so a client that
    /// paints from its cache knows which rows to drop without diffing a list
    /// it did not fetch.
    #[tokio::test(start_paused = true)]
    async fn a_board_item_names_the_entities_that_left() {
        let listed: Arc<Mutex<Vec<String>>> =
            Arc::new(Mutex::new(vec!["run-7".into(), "run-8".into()]));
        let board = Arc::clone(&listed);
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(move || board.lock().unwrap().clone()),
            Arc::new(|_: &[FactsRequest]| Vec::new()),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec("s-all", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );

        bus.note_board();
        bus.flush();
        let first = frames(drained(&mut rx, &key));
        assert_eq!(
            first[0]["items"][0]["state"]["removed"],
            Value::Null,
            "the first board item has no before to compare with: {first:?}"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        listed.lock().unwrap().retain(|id| id != "run-8");
        bus.note_board();
        bus.flush();

        let sent = frames(drained(&mut rx, &key));
        assert_eq!(sent[0]["items"][0]["state"]["removed"], json!(["run-8"]));

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_board();
        bus.flush();
        let again = frames(drained(&mut rx, &key));
        assert_eq!(
            again[0]["items"][0]["state"]["removed"],
            Value::Null,
            "it left once: {again:?}"
        );
    }

    /// The project and workspace lists ride the board item only when the
    /// change that noted it moved one of them — a row moving re-sends no
    /// list at all.
    #[tokio::test(start_paused = true)]
    async fn a_board_item_carries_the_lists_only_when_they_moved() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(|requests: &[FactsRequest]| {
                requests
                    .iter()
                    .map(|request| EntityFacts {
                        entity_id: request.entity_id.clone(),
                        state: request
                            .lists
                            .projects
                            .then(|| json!({ "projects": [{ "project_id": "proj-1" }] })),
                        ..EntityFacts::default()
                    })
                    .collect()
            }),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec("s-all", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );

        bus.note_board();
        bus.flush();
        let rows_only = frames(drained(&mut rx, &key));
        assert_eq!(
            rows_only[0]["items"][0]["state"],
            json!({ "revision": 1 }),
            "a row moved, not a list"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_board_lists(BoardLists::PROJECTS);
        bus.flush();

        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["state"],
            json!({ "revision": 2, "projects": [{ "project_id": "proj-1" }] })
        );
    }
}
