//! Push invalidation: the bridge telling every connected browser that
//! something changed, instead of waiting to be polled.
//!
//! A session subscribes (`changes.subscribe`) to the entities and kinds it is
//! showing, and hears `changes` frames for exactly those: one item per entity
//! that moved, carrying the body of what moved (its row, its conversation
//! since the last frame, its git shapes, the files that changed), so a client
//! paints from the push rather than refetching after it. A session that
//! subscribes to nothing hears nothing.
//!
//! **Terminal output is not a change.** It has its own push path
//! (`term.output` / `term.reset`), and routing a byte storm through here would
//! turn a repainting TUI into an invalidation storm.
//!
//! # The two halves, and why they are separate
//!
//! Noting a change happens deep inside mutations that run holding the app
//! mutex. Sending one encrypts a frame per subscription. So a note only
//! inserts into a subscription's pending map behind a leaf mutex — no I/O, no
//! encryption, nothing that can block on anything but itself — and
//! [`ChangeBus::flush`], driven by a task that holds no app lock, does the
//! sending.
//!
//! That split is also the coalescer. A flush collapses everything noted since
//! the last one into ONE item per entity, and the driver
//! ([`ChangeBus::spawn_flusher`]) flushes at most once per
//! [`ChangeBus::window`]: the first change on an idle bus goes out at once,
//! and a burst behind it costs one frame per window rather than one per
//! mutation.

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

/// How long one entity's worktree item waits before it may be repeated.
///
/// The bus's window collapses a burst of notes into one item; this bounds how
/// often the SAME entity's `git` and `files` items go out at all, for the
/// origin that fires on every file an agent writes. A browser repaints a git
/// surface off it, and a second is as often as a human reads one: the floor
/// under a realtime subscription's worktree items (wire spec, step 1.3).
pub const ENTITY_SETTLE_WINDOW: Duration = Duration::from_secs(1);

/// The most keys one un-flushed window holds before it gives up on precision.
///
/// Applies to each subscription's pending map. Past the cap the whole batch
/// collapses to a bare board item, which already tells a client to refetch
/// everything.
const PENDING_KEY_CAP: usize = 512;

/// How many entities one subscription remembers conversation cursors for.
///
/// The same ceiling [`PENDING_KEY_CAP`] puts on the entities one flush can
/// name, so the table can never outgrow what a flush could fill it with.
/// Past it the coldest entity is dropped, which costs that conversation one
/// tip-only item and the `thread.page` a client answers that with.
const THREAD_CURSOR_ENTITIES: usize = PENDING_KEY_CAP;

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

/// The most unpublished commits one `git` item names.
///
/// A checkout with no publication base — a repository with no remote, or one
/// whose history is unrelated to its push target — has its WHOLE history
/// standing above that base, so the list is capped at what a client keeps
/// (`UNPUSHED_COMMITS_MAX` in the SPA, the commits whose patches it syncs).
/// Newest first, so the cap drops the oldest. A reviewer wanting further
/// back pages `git.log`.
pub const UNPUSHED_COMMITS_MAX: usize = 20;

/// The most conversation items one `thread` item carries.
///
/// A push is a cache write, and past a hundred rows the write is bigger than
/// the read that would replace it: the tip alone goes out, and the client
/// pages forward from the sequence it holds. An agent that says a hundred
/// things between two flushes is a harness in a storm, not a conversation.
pub const THREAD_PUSH_MAX_ITEMS: usize = 100;
/// The same, for the task ids one `tasks` item names. Past it the item is
/// `truncated`, which means "refetch the list", not "these tasks".
pub const TASKS_PER_FLUSH: usize = 200;

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
/// `session.hello` greeting so a client knows what it may hear. `changes` is
/// what a subscription delivers.
pub const ANNOUNCED_EVENTS: [&str; 3] =
    [CHANGES_EVENT, "bridge.update_status", MODELS_CHANGED_EVENT];

/// A CLI on this machine changed what it runs, so `models.list` would answer
/// differently now (#203). Carries nothing else: the catalog is asked again.
pub const MODELS_CHANGED_EVENT: &str = "models.changed";

/// The whole of a [`MODELS_CHANGED_EVENT`] push.
pub fn models_changed_payload() -> serde_json::Value {
    serde_json::json!({ "type": MODELS_CHANGED_EVENT })
}

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
    /// One project's task tracker: a create, an update, a comment, a move
    /// (spec: Tasks → Push).
    ///
    /// The one kind whose entity is a PROJECT rather than a work item, because
    /// a tracker belongs to a project and not to any one thing inside it. A
    /// subscription scoped `all` receives it beside everything else; one
    /// scoped to a project entity receives only it.
    Tasks,
}

impl Kind {
    /// Every kind, in wire order.
    pub const ALL: [Kind; 6] = [
        Kind::State,
        Kind::Thread,
        Kind::Git,
        Kind::Files,
        Kind::Terminals,
        Kind::Tasks,
    ];

    /// How the wire spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::State => "state",
            Kind::Thread => "thread",
            Kind::Git => "git",
            Kind::Files => "files",
            Kind::Terminals => "terminals",
            Kind::Tasks => "tasks",
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
    /// One task, run, or worktree.
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
    /// A cooldown, not an interval clock: what moves is pushed at once, and
    /// that push holds the next one back for the interval. Anything noted
    /// inside the cooldown rides the push that ends it, so a subscription
    /// quiet for an hour hears the next change on the bus's own window
    /// rather than an interval later. Clamped to
    /// [`MIN_BATCH_MS`]..=[`MAX_BATCH_MS`].
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

/// The kinds a `changes.subscribe` names, as the words it sent. The verb
/// reads them as words before it reads them as kinds, so its refusal can name
/// every one this bridge does not know rather than the first serde met.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct KindNames(Vec<String>);

impl KindNames {
    /// The kinds these name, or every name that is not one — in the order
    /// asked, each once.
    pub fn known(&self) -> Result<KindSet, Vec<String>> {
        let mut kinds = KindSet::default();
        let mut unknown: Vec<String> = Vec::new();
        for name in &self.0 {
            match Kind::ALL.into_iter().find(|kind| kind.as_str() == name) {
                Some(kind) => {
                    kinds.insert(kind);
                }
                None if !unknown.contains(name) => unknown.push(name.clone()),
                None => {}
            }
        }
        if unknown.is_empty() {
            Ok(kinds)
        } else {
            Err(unknown)
        }
    }
}

impl<'a> FromIterator<&'a str> for KindNames {
    fn from_iter<I: IntoIterator<Item = &'a str>>(names: I) -> KindNames {
        KindNames(names.into_iter().map(str::to_string).collect())
    }
}

/// One subscription, exactly as `changes.subscribe` states it. `K` is how its kinds are held: a [`KindSet`]
/// once read, [`KindNames`] while the verb is still reading them.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SubscriptionSpec<K = KindSet> {
    #[serde(rename = "subscription_id")]
    pub id: String,
    pub scope: Scope,
    pub kinds: K,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub priority: Priority,
}

impl SubscriptionSpec<KindNames> {
    /// The same spec with its kinds read, or every name among them that is
    /// not a kind this bridge knows.
    pub fn known(self) -> Result<SubscriptionSpec, Vec<String>> {
        let kinds = self.kinds.known()?;
        Ok(SubscriptionSpec {
            id: self.id,
            scope: self.scope,
            kinds,
            mode: self.mode,
            priority: self.priority,
        })
    }
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
    /// The harnesses out of usage on this device (task #58).
    pub usage_limits: bool,
}

impl BoardLists {
    /// The project list moved: one was added, hidden, deleted, or renamed.
    pub const PROJECTS: BoardLists = BoardLists {
        projects: true,
        workspaces: false,
        usage_limits: false,
    };
    /// The workspace list moved: one was created, renamed, or deleted.
    pub const WORKSPACES: BoardLists = BoardLists {
        projects: false,
        workspaces: true,
        usage_limits: false,
    };
    /// A harness on this device ran out of usage, or a turn ran on one that
    /// had.
    pub const USAGE_LIMITS: BoardLists = BoardLists {
        projects: false,
        workspaces: false,
        usage_limits: true,
    };

    pub fn is_empty(self) -> bool {
        !self.projects && !self.workspaces && !self.usage_limits
    }

    fn merge(&mut self, other: BoardLists) {
        self.projects |= other.projects;
        self.workspaces |= other.workspaces;
        self.usage_limits |= other.usage_limits;
    }

    /// The wire keys this set names — what a board item may carry, and the
    /// filter a frame applies to a lookup answered for several frames at
    /// once.
    fn names(self) -> impl Iterator<Item = &'static str> {
        [
            ("projects", self.projects),
            ("workspaces", self.workspaces),
            ("usage_limits", self.usage_limits),
        ]
        .into_iter()
        .filter_map(|(name, wanted)| wanted.then_some(name))
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
    /// Canonical conversation identity, which can differ from `agent_id`
    /// when an implementation agent continues a task agent's conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_generation_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_revision: Option<u64>,
    pub last_sequence: u64,
    /// The inbox row this conversation belongs to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_session: Option<crate::session_summary::SessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_session: Option<crate::session_summary::SessionSummary>,
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

// ------------------------------------------------------- subscriptions ---

/// The names one kind carries on an item — a `files` item's paths, an
/// `tasks` item's task ids — with the cap that turns naming them into
/// "refetch".
///
/// One type for both, because they are one idea: a bounded list of what moved,
/// which past its cap stops being a list and becomes a flag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NameSet {
    names: BTreeSet<String>,
    truncated: bool,
}

impl NameSet {
    fn add(&mut self, names: &[String], cap: usize) {
        for name in names {
            if self.names.len() >= cap && !self.names.contains(name) {
                self.truncated = true;
                break;
            }
            self.names.insert(name.clone());
        }
    }
}

/// One entity's un-flushed item, for one subscription.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PendingItem {
    kinds: KindSet,
    files: NameSet,
    tasks: NameSet,
}

impl PendingItem {
    fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// Split into (what the settle window holds back, what may go now): the
    /// worktree kinds wait, `state`, `thread` and `tasks` never do.
    fn split_worktree(self) -> (PendingItem, PendingItem) {
        let mut held = PendingItem {
            files: self.files,
            tasks: self.tasks,
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

/// What one subscription has already sent for one entity's conversations,
/// and when it last sent any of it — the order the table is evicted in when
/// it is full.
struct ThreadCursors {
    sent: Vec<(String, u64)>,
    at: Instant,
}

/// One live subscription: what it asked for, and what is waiting for it.
struct Subscription {
    spec: SubscriptionSpec,
    session: SessionSender,
    pending: BTreeMap<String, PendingItem>,
    collapsed: bool,
    created: Instant,
    last_flush: Option<Instant>,
    /// When `pending` last went from empty to holding something — the
    /// leading edge of a batch window, and `None` while it holds nothing.
    /// A batch subscription that has been quiet longer than its interval is
    /// pushed from here, so the first change after the quiet costs the
    /// coalesce window rather than another whole interval.
    first_pending_at: Option<Instant>,
    /// When each entity's worktree kinds last went out on THIS subscription —
    /// the settle floor, pruned on every flush so it never grows with the
    /// entities a bridge has seen.
    emitted_at: HashMap<String, Instant>,
    /// What this subscription has sent of each entity's conversations, so
    /// its next thread item carries what was said after it. Per
    /// subscription, because two subscriptions over one conversation have
    /// heard different amounts of it; keyed by entity, so a flush reads the
    /// cursors of what moved and not of the whole device. An entity that
    /// leaves the board takes its cursors with it, and
    /// [`THREAD_CURSOR_ENTITIES`] caps the rest.
    emitted_tips: HashMap<String, ThreadCursors>,
    /// Which whole lists have moved since THIS subscription's last board
    /// item. Per subscription, because two subscriptions watch the board at
    /// two cadences: the one that flushes first must not swallow the news
    /// for the one still waiting out its batch. A subscription that is
    /// `off` holds its latch until it wakes.
    pending_lists: BoardLists,
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
            first_pending_at: None,
            emitted_at: HashMap::new(),
            emitted_tips: HashMap::new(),
            pending_lists: BoardLists::default(),
            covered_last: None,
        }
    }

    fn wants(&self, entity_id: &str, kind: Kind) -> bool {
        self.spec.scope.covers(entity_id) && self.spec.kinds.contains(kind)
    }

    /// Insert into this subscription's own pending map. Coalescing is per
    /// subscription: a thousand notes of one entity are one item, and an
    /// hour spent `off` holds one item per entity, not an hour of history.
    fn note(&mut self, entity_id: &str, kind: Kind, names: &[String], now: Instant) {
        if self.collapsed {
            return;
        }
        if self.pending.is_empty() {
            self.first_pending_at = Some(now);
        }
        if self.pending.len() >= PENDING_KEY_CAP && !self.pending.contains_key(entity_id) {
            self.collapse();
            return;
        }
        let item = self.pending.entry(entity_id.to_string()).or_default();
        item.kinds.insert(kind);
        match kind {
            Kind::Files => item.files.add(names, FILES_PER_FLUSH),
            Kind::Tasks => item.tasks.add(names, TASKS_PER_FLUSH),
            _ => {}
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
    ///
    /// A batch subscription is a cooldown, not an interval clock: what it
    /// holds goes out as soon as it arrives unless the last push is still
    /// inside the window, in which case it goes out when that window ends —
    /// never an interval after the note itself.
    fn due_at(&self, window: Duration, now: Instant) -> Option<Instant> {
        if self.pending.is_empty() {
            return None;
        }
        match self.spec.mode {
            Mode::Off => None,
            Mode::Batch(every) => Some(self.cooldown_ends(every).max(self.leading_edge(now))),
            Mode::Realtime => {
                let cadence = self.last_flush.map_or(now, |at| at + window);
                Some(cadence.max(self.earliest_release(now)))
            }
        }
    }

    /// When the cooldown this subscription's last push started ends. A
    /// subscription that has pushed nothing yet is in no cooldown, so its
    /// first item is bounded by the leading edge alone.
    fn cooldown_ends(&self, every: Duration) -> Instant {
        match self.last_flush {
            Some(at) => at + every,
            None => self.created,
        }
    }

    /// When what this subscription holds first arrived — `now` for a map
    /// that somehow holds something nothing noted, which is due at once.
    fn leading_edge(&self, now: Instant) -> Instant {
        self.first_pending_at.unwrap_or(now)
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
        let Some(before) = self.covered_last.replace(covered.clone()) else {
            return Vec::new();
        };
        let removed: Vec<String> = before.difference(covered).cloned().collect();
        // The client drops a departed entity's conversation, so what this
        // subscription remembered having sent for it is worth nothing.
        for entity_id in &removed {
            self.emitted_tips.remove(entity_id);
        }
        removed
    }

    /// What this subscription has already been sent for each entity whose
    /// conversation is going out — its own cursors and nobody else's, so a
    /// flush costs the entities that moved rather than every entity the
    /// subscription has ever carried.
    fn thread_cursors(
        &self,
        due: &BTreeMap<String, PendingItem>,
    ) -> BTreeMap<String, Vec<(String, u64)>> {
        due.iter()
            .filter(|(_, item)| item.kinds.contains(Kind::Thread))
            .filter_map(|(id, _)| Some((id.clone(), self.emitted_tips.get(id)?.sent.clone())))
            .collect()
    }

    /// Record what one entity's item just carried.
    fn record_thread_tips(&mut self, entity_id: &str, tips: &[ThreadTip], now: Instant) {
        let cursors = self
            .emitted_tips
            .entry(entity_id.to_string())
            .or_insert_with(|| ThreadCursors {
                sent: Vec::new(),
                at: now,
            });
        cursors.at = now;
        for tip in tips {
            match cursors.sent.iter_mut().find(|(id, _)| *id == tip.agent_id) {
                Some((_, sequence)) => *sequence = tip.last_sequence,
                None => cursors.sent.push((tip.agent_id.clone(), tip.last_sequence)),
            }
        }
        self.evict_cold_cursors();
    }

    /// Hold the cursor table to [`THREAD_CURSOR_ENTITIES`], coldest entity
    /// first. One dropped entity costs its conversation a tip-only item and
    /// the `thread.page` the client answers that with.
    fn evict_cold_cursors(&mut self) {
        while self.emitted_tips.len() > THREAD_CURSOR_ENTITIES {
            let coldest = self
                .emitted_tips
                .iter()
                .min_by_key(|(_, cursors)| cursors.at)
                .map(|(entity_id, _)| entity_id.clone());
            match coldest {
                Some(entity_id) => self.emitted_tips.remove(&entity_id),
                None => return,
            };
        }
    }

    /// The whole lists this flush's board item carries, taken off the
    /// subscription's own latch — and only when a board item is actually
    /// going out, so a list that moved while this subscription was `off` is
    /// still carried by the flush that wakes it.
    fn take_board_lists(&mut self, due: &BTreeMap<String, PendingItem>) -> BoardLists {
        match due.contains_key(BOARD_ITEM_ID) {
            true => std::mem::take(&mut self.pending_lists),
            false => BoardLists::default(),
        }
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
            self.first_pending_at = None;
        }
    }
}

/// One subscription's frame, ready to encrypt.
struct DueFrame {
    session: SessionSender,
    subscription_id: String,
    priority: Priority,
    items: BTreeMap<String, PendingItem>,
    /// What this subscription has already sent for each agent of each entity
    /// whose conversation is in the frame. Empty when no item carries the
    /// `thread` kind.
    thread_after: BTreeMap<String, Vec<(String, u64)>>,
    /// The entities that have left the board since this subscription's last
    /// board item. Empty when nothing left, and when the frame carries no
    /// board item at all.
    removed: Vec<String>,
    /// The whole lists THIS frame's board item is to carry, taken off the
    /// subscription's own latch. The lookup answers the union across every
    /// due frame; this says which of it is this one's.
    lists: BoardLists,
}

impl DueFrame {
    /// What this frame has already sent for ONE entity's conversations.
    fn cursors_for(&self, entity_id: &str) -> &[(String, u64)] {
        self.thread_after
            .get(entity_id)
            .map_or(&[][..], Vec::as_slice)
    }

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
        out.insert(
            "thread".into(),
            thread_payload(facts, frame.cursors_for(entity_id)),
        );
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
    if item.kinds.contains(Kind::Tasks) {
        out.insert(
            "tasks".into(),
            json!({ "task_ids": item.tasks.names, "truncated": item.tasks.truncated }),
        );
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
/// Whether this subscription has already been sent this item AS IT STANDS.
///
/// The item's own later of the two columns, because an item that was mutated
/// after it was sent — a delivery status moving, an edit — is news to a
/// subscription holding the copy from before, and its `sequence` says nothing
/// about that.
fn sent(item: &Value, since: u64) -> bool {
    let data = &item["data"];
    let latest = data["sequence"]
        .as_u64()
        .max(data["updated_sequence"].as_u64());
    latest.is_some_and(|sequence| sequence <= since)
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
    let looked_up = facts
        .and_then(|f| f.state.as_ref())
        .and_then(Value::as_object);
    for list in frame.lists.names() {
        if let Some(value) = answered_list(looked_up, list) {
            state.insert(list.into(), value.clone());
        }
    }
    Value::Object(state)
}

/// The list this lookup answered under `name`, when it answered one.
///
/// The board item's lists are arrays by contract. A key the lookup could not
/// fill — and a key it filled with a null, which is what a failed
/// `workspace.list` reads as — is not an answer: the client is left holding
/// the list it has rather than handed something it cannot iterate.
fn answered_list<'a>(answer: Option<&'a Map<String, Value>>, name: &str) -> Option<&'a Value> {
    answer?.get(name).filter(|value| value.is_array())
}

/// Which of the lists a frame was owed its lookup did not answer with one.
fn unanswered_lists(owed: BoardLists, answer: Option<&Map<String, Value>>) -> BoardLists {
    BoardLists {
        projects: owed.projects && answered_list(answer, "projects").is_none(),
        workspaces: owed.workspaces && answered_list(answer, "workspaces").is_none(),
        usage_limits: owed.usage_limits && answered_list(answer, "usage_limits").is_none(),
    }
}

/// The paths that moved, and the worktree's top level as the lookup listed
/// it. A root the lookup could not read is left off, which a client answers
/// the way it always has — by listing the tree itself.
fn files_payload(item: &PendingItem, facts: Option<&EntityFacts>) -> Value {
    let mut files = Map::new();
    files.insert("paths".into(), json!(item.files.names));
    files.insert("truncated".into(), json!(item.files.truncated));
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
    // The body never rides a push, whatever its size.
    //
    // A push is news about a checkout, and the hunks are the largest thing a
    // checkout has: sending them to every subscriber on every flush put
    // megabytes on the wire for surfaces nobody had opened, and over a phone's
    // relayed path those megabytes are what the reader's own connection had to
    // wait behind. So the item says the diff moved and how big it is, and the
    // surface that shows one asks for it — which is the same shape a diff too
    // big to push already had, and the client has read it that way all along.
    if let Some(bytes) = facts.diff_bytes {
        git.insert("diff".into(), Value::Null);
        git.insert("diff_bytes".into(), json!(bytes));
    }
    Value::Object(git)
}

/// Everything one flush turn will send, taken under the locks and sent with
/// them released.
#[derive(Default)]
struct Due {
    frames: Vec<DueFrame>,
    requests: Vec<FactsRequest>,
}

impl Due {
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Every browser session that asked for push invalidation, and the changes
/// waiting to reach them.
pub struct ChangeBus {
    /// Every live subscription, in the order it was made — the flush order
    /// within one priority. A session whose push fails loses all of its
    /// subscriptions: a sender that cannot send has no connection left.
    subscriptions: Mutex<Vec<Subscription>>,
    /// Set whenever a subscription gains something to send, so the flusher
    /// wakes on the change rather than on a tick.
    wake: tokio::sync::Notify,
    window: Duration,
    /// Monotonic, bumped by every [`ChangeBus::note_board`]: a client holding
    /// the same revision skips `board.list`.
    board_revision: AtomicU64,
    /// Worktrees whose watcher could not start; a subscription covering one
    /// answers `watch: "polled"`.
    polled: Mutex<BTreeSet<String>>,
    board_entities: BoardEntities,
    /// `None` on a bus built without one — the unit tests, and any caller
    /// that wants the kinds without the keys. A flush then costs no lookup
    /// and no blocking thread at all.
    facts: Option<FactsSource>,
    #[cfg(test)]
    completed_test_cycles: tokio::sync::watch::Sender<(u64, bool)>,
    /// Records the worker that completed each subscription frame's encryption.
    #[cfg(test)]
    delivered_test_threads: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Option<String>>>>,
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
        #[cfg(test)]
        let (completed_test_cycles, _) = tokio::sync::watch::channel((0, true));
        Arc::new(ChangeBus {
            subscriptions: Mutex::new(Vec::new()),
            wake: tokio::sync::Notify::new(),
            window,
            board_revision: AtomicU64::new(0),
            polled: Mutex::new(BTreeSet::new()),
            board_entities,
            facts,
            #[cfg(test)]
            completed_test_cycles,
            #[cfg(test)]
            delivered_test_threads: Mutex::new(None),
        })
    }

    /// Wait for a flusher cycle that starts after this call. Two completions
    /// cover a cycle already in flight when the test registers its barrier.
    #[cfg(test)]
    pub async fn settle_for_test(&self) {
        let mut completed = self.completed_test_cycles.subscribe();
        let target = completed.borrow().0 + 2;
        self.wake.notify_one();
        while {
            let (cycle, drained) = *completed.borrow_and_update();
            cycle < target || !drained
        } {
            completed
                .changed()
                .await
                .expect("the flusher remains alive");
        }
    }

    /// Whether every sendable change has completed its flush. A subscription
    /// in Off mode deliberately holds changes until it is enabled again.
    #[cfg(test)]
    fn has_test_pending(&self) -> bool {
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .any(|sub| sub.spec.mode != Mode::Off && !sub.pending.is_empty())
    }

    /// How long changes collapse together before the next flush.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// The board's current revision — what a board item reports.
    pub fn board_revision(&self) -> u64 {
        self.board_revision.load(Ordering::SeqCst)
    }

    // -------------------------------------------------------- the verbs ---

    /// Upsert one subscription for this session (`changes.subscribe`).
    ///
    /// Re-sending an id with a new mode is how a client changes cadence: the
    /// spec is replaced and the pending items are kept, so an `off`
    /// subscription turned back on has no gap in what it will report.
    pub fn subscribe(&self, session: &SessionSender, sub: SubscriptionSpec) -> SubscribeOutcome {
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

    /// Stop hearing anything — the session closed.
    pub fn unsubscribe(&self, session_id: &str) {
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|sub| sub.session.session_id() != session_id);
    }

    /// Drop one subscription (`changes.unsubscribe`).
    pub fn unsubscribe_one(&self, session_id: &str, sub_id: &str) {
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|sub| !sub.is(session_id, sub_id));
    }

    /// This session's subscriptions, in the order they were made — what the
    /// tests read back.
    #[cfg(test)]
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

    /// One kind of one entity moved.
    ///
    /// SAFE UNDER THE APP MUTEX, and the reason this type exists: it takes
    /// leaf mutexes, inserts into each subscription that asked, and returns.
    /// No encryption, no channel a slow reader can fill, no I/O of any kind —
    /// nothing a caller holding the app lock could block the whole daemon on.
    pub fn note_kind(&self, entity_id: &str, kind: Kind) {
        self.note_subscriptions(entity_id, kind, &[]);
    }

    /// This checkout's tab list moved.
    pub fn note_terminals(&self, entity_id: &str) {
        self.note_subscriptions(entity_id, Kind::Terminals, &[]);
    }

    /// These working-tree paths moved, relative to the worktree root.
    pub fn note_files(&self, entity_id: &str, paths: &[String]) {
        self.note_subscriptions(entity_id, Kind::Files, paths);
    }

    /// These tasks of this project moved (spec: Tasks → Push).
    ///
    /// No board bump: bumping the board on every comment would repaint the
    /// feed for something the feed does not show.
    pub fn note_tasks(&self, project_id: &str, task_ids: &[String]) {
        self.note_subscriptions(project_id, Kind::Tasks, task_ids);
    }

    /// The feed is stale: bump the revision a client compares against, and
    /// note the board item for every subscription that watches the feed.
    pub fn note_board(&self) {
        self.board_revision.fetch_add(1, Ordering::SeqCst);
        self.note_subscriptions(BOARD_ITEM_ID, Kind::State, &[]);
    }

    /// The feed moved, and so did one of the lists a client caches whole:
    /// the next board item carries that list in full.
    pub fn note_board_lists(&self, lists: BoardLists) {
        for sub in self
            .subscriptions
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|sub| sub.wants(BOARD_ITEM_ID, Kind::State))
        {
            sub.pending_lists.merge(lists);
        }
        self.note_board();
    }

    /// This entity is stale — and so is the feed, which shows a row for it.
    ///
    /// Both the kinds an entity's own detail is made of: its row, and its
    /// conversation. It is the only origin a conversation has, since an item
    /// lands through the same mutation tail every other change does. A conversation that
    /// did not move costs its subscription an unchanged tip, which is what
    /// the tip is for.
    pub fn note_entity(&self, id: &str) {
        self.note_kind(id, Kind::State);
        self.note_kind(id, Kind::Thread);
        self.note_board();
    }

    /// This entity's git is stale, at the pace a browser can paint — its item
    /// goes out at most once per [`ENTITY_SETTLE_WINDOW`].
    ///
    /// For an origin that fires as fast as an agent writes files: the TTL
    /// refresh, which is the `git` kind arriving without a watcher. Every
    /// other caller wants [`note_entity`](Self::note_entity).
    pub fn note_entity_settled(&self, id: &str) {
        self.note_kind(id, Kind::Git);
    }

    /// Resolve one note to every subscription that asked for it.
    fn note_subscriptions(&self, entity_id: &str, kind: Kind, paths: &[String]) {
        let now = Instant::now();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let mut noted = false;
        for sub in subscriptions
            .iter_mut()
            .filter(|s| s.wants(entity_id, kind))
        {
            sub.note(entity_id, kind, paths, now);
            noted = true;
        }
        drop(subscriptions);
        if noted {
            self.wake.notify_one();
        }
    }

    /// Whether anything is waiting to go out.
    pub fn has_pending(&self) -> bool {
        self.subscriptions
            .lock()
            .unwrap()
            .iter()
            .any(|sub| !sub.pending.is_empty())
    }

    // -------------------------------------------------------- the flush ---

    /// Send everything due: one `changes` frame per due subscription,
    /// foreground first. Drops every session whose connection is gone.
    /// Returns the number of frames sent.
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
        let mut due = Due::default();
        // Read before the subscriptions lock: the board's list is somebody
        // else's leaf lock, and this module nests none.
        let covered: BTreeSet<String> = (self.board_entities)().into_iter().collect();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        for sub in subscriptions.iter_mut() {
            let Some(items) = sub.take_due(self.window, now) else {
                continue;
            };
            let removed = sub.board_departures(&items, &covered);
            let lists = sub.take_board_lists(&items);
            let thread_after = sub.thread_cursors(&items);
            due.frames.push(DueFrame {
                session: sub.session.clone(),
                subscription_id: sub.spec.id.clone(),
                priority: sub.spec.priority,
                items,
                thread_after,
                removed,
                lists,
            });
        }
        drop(subscriptions);
        due.frames.sort_by_key(|frame| frame.priority);
        due.requests = fact_requests(&due.frames);
        if let Some(request) = board_lists_request(&due.frames) {
            due.requests.push(request);
        }
        due
    }

    /// Encrypt and send. A subscription whose push fails takes every
    /// subscription of that session with it.
    fn deliver(&self, due: Due, facts: Vec<EntityFacts>) -> usize {
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
                #[cfg(test)]
                if let Some(observe) = self.delivered_test_threads.lock().unwrap().as_ref() {
                    let _ = observe.send(std::thread::current().name().map(str::to_owned));
                }
                self.stamp_thread_tips(frame, &by_entity);
                self.rearm_unanswered_lists(frame, &by_entity);
            } else {
                dead.push(frame.session.session_id().to_string());
            }
        }
        for session_id in dead {
            self.unsubscribe(&session_id);
        }
        frames
    }

    /// Put back what this frame was owed and could not carry.
    ///
    /// The latch is taken when the frame is built, BEFORE the lookup runs,
    /// so a lookup that answered no list must leave the subscription still
    /// knowing its client's cached list is stale. Nothing else would re-arm
    /// it: the note that moved the list is long gone.
    ///
    /// Re-armed, not re-noted. A lookup that keeps failing would otherwise
    /// pin the flusher at the coalesce window forever, sending a board item
    /// a second that carries nothing. The list rides the next board item
    /// instead, and every entity change notes one.
    fn rearm_unanswered_lists(&self, frame: &DueFrame, facts: &BTreeMap<&str, &EntityFacts>) {
        let answer = facts
            .get(BOARD_ITEM_ID)
            .and_then(|fact| fact.state.as_ref())
            .and_then(Value::as_object);
        let missing = unanswered_lists(frame.lists, answer);
        if missing.is_empty() {
            return;
        }
        let mut subscriptions = self.subscriptions.lock().unwrap();
        if let Some(sub) = subscriptions
            .iter_mut()
            .find(|sub| sub.is(frame.session.session_id(), &frame.subscription_id))
        {
            sub.pending_lists.merge(missing);
        }
    }

    /// Record the conversations this frame just carried, so the next one
    /// carries what was said after them.
    ///
    /// HERE and not in [`Subscription::stamp`]: the tips are what the facts
    /// source answered, which is looked up after the items are taken and
    /// known only once the frame has gone out.
    fn stamp_thread_tips(&self, frame: &DueFrame, facts: &BTreeMap<&str, &EntityFacts>) {
        let sent: Vec<(&String, &[ThreadTip])> = frame
            .items
            .iter()
            .filter(|(_, item)| item.kinds.contains(Kind::Thread))
            .filter_map(|(id, _)| Some((id, facts.get(id.as_str())?.threads.as_slice())))
            .filter(|(_, tips)| !tips.is_empty())
            .collect();
        if sent.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut subscriptions = self.subscriptions.lock().unwrap();
        let Some(sub) = subscriptions
            .iter_mut()
            .find(|sub| sub.is(frame.session.session_id(), &frame.subscription_id))
        else {
            return;
        };
        for (entity_id, tips) in sent {
            sub.record_thread_tips(entity_id, tips, now);
        }
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
            #[cfg(test)]
            {
                let completed = bus.completed_test_cycles.borrow().0 + 1;
                bus.completed_test_cycles
                    .send_replace((completed, !bus.has_test_pending()));
                if bus.completed_test_cycles.receiver_count() > 0 {
                    bus.wake.notify_one();
                }
            }
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
        // A row's stat is a fact of the checkout: an entity whose git moved has
        // a row that moved with it, so the subscriptions that carry rows hear
        // `state` for it on the next window — after the facts source has set
        // the stat walk going, so the row they get carries what it found.
        let rows_moved: Vec<String> = due
            .requests
            .iter()
            .filter(|request| request.git)
            .map(|request| request.entity_id.clone())
            .collect();
        let delivered = bus.deliver(due, facts);
        for entity_id in rows_moved {
            bus.note_kind(&entity_id, Kind::State);
        }
        delivered
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
        ChangeBus::spawn_flusher_on(bus, None);
    }

    /// The same, on `runtime` — the daemon's push runtime, where serializing
    /// and encrypting a frame per subscriber competes with nothing that
    /// answers a request. `None` is the runtime this is called on.
    pub fn spawn_flusher_on(bus: Arc<Self>, runtime: Option<tokio::runtime::Handle>) {
        if let Some(handle) = runtime.or_else(|| tokio::runtime::Handle::try_current().ok()) {
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
///
/// One consequence worth naming: a merged window wider than
/// [`THREAD_PUSH_MAX_ITEMS`] answers tip-only for everyone, so a tab that is
/// far behind sends every tab that is close to `thread.page` too. Correct
/// either way — the client pages — and the alternative is a lookup per
/// subscription rather than per entity.
fn merge_thread_after(into: &mut Vec<(String, u64)>, tips: &[(String, u64)]) {
    for (agent_id, sequence) in tips {
        match into.iter_mut().find(|(held, _)| held == agent_id) {
            Some((_, held)) => *held = (*held).min(*sequence),
            None => into.push((agent_id.clone(), *sequence)),
        }
    }
}

/// The one lookup every due board item's whole lists share: the union of
/// what the frames asked for. Each frame then carries its own share of the
/// answer, so a subscription that heard about the project list does not get
/// the workspace list a different subscription was owed.
fn board_lists_request(frames: &[DueFrame]) -> Option<FactsRequest> {
    let mut lists = BoardLists::default();
    for frame in frames {
        lists.merge(frame.lists);
    }
    (!lists.is_empty()).then(|| FactsRequest {
        entity_id: BOARD_ITEM_ID.to_string(),
        lists,
        ..FactsRequest::default()
    })
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
                merge_thread_after(&mut entry.thread_after, frame.cursors_for(id));
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
mod subscriptions {
    use super::*;

    /// Move a paused clock forward by `step` and let the flusher act on it.
    ///
    /// `advance` wakes the timers it passes; the yield is what gives the task
    /// they woke a turn to run before the assertion looks. Without it the test
    /// would race the scheduler instead of the clock — the same race, moved.
    async fn settle(step: Duration) {
        tokio::time::advance(step).await;
        tokio::task::yield_now().await;
    }

    /// Drain everything a subscriber was pushed, decrypted.
    fn drained(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
        key: &str,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(message) = rx.try_recv() {
            seen.push(SessionSender::decrypt_push(key, &message));
        }
        seen
    }

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

    /// The batch half: the same burst is one item on a subscription that
    /// asked for thirty seconds — pushed at once, because nothing has been
    /// pushed to it yet — and the storm that follows inside the cooldown it
    /// started is one more item at the cooldown's end. An entity whose git
    /// moved has a row that moved with it, so the item after the first
    /// carries `state` beside `git`.
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
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-bg",
                "items": [{ "entity_id": "run-7", "git": {} }],
            })],
            "a hundred notes, one item, on the leading edge"
        );

        for _ in 0..100 {
            bus.note_kind("run-7", Kind::Git);
            settle(Duration::from_millis(100)).await;
        }
        assert!(
            frames(drained(&mut rx, &key)).is_empty(),
            "ten seconds of notes, all inside the cooldown"
        );

        settle(Duration::from_secs(21)).await;
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-bg",
                "items": [{ "entity_id": "run-7", "git": {}, "state": {} }],
            })],
            "a hundred more, one item, at the cooldown's end"
        );
    }

    /// The cooldown's leading edge: a batch subscription that has been quiet
    /// long past its interval owes nothing to a clock it already outlived, so
    /// the first note after the quiet goes out on the bus's own window rather
    /// than waiting out another interval.
    #[tokio::test(start_paused = true)]
    async fn a_batch_subscription_quiet_for_an_hour_pushes_the_next_note_at_once() {
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

        settle(Duration::from_secs(3600)).await;
        assert!(frames(drained(&mut rx, &key)).is_empty(), "nothing moved");

        bus.note_kind("run-7", Kind::Git);
        settle(DEFAULT_COALESCE_WINDOW).await;
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-bg",
                "items": [{ "entity_id": "run-7", "git": {} }],
            })],
            "the first note after the quiet, on the coalesce window"
        );
    }

    /// The cooldown's trailing edge: what arrives inside the cooldown waits
    /// for the cooldown to end — measured from the push that started it, not
    /// from the note that is waiting it out.
    #[tokio::test(start_paused = true)]
    async fn a_note_inside_the_cooldown_goes_out_when_the_cooldown_ends() {
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
        settle(Duration::from_secs(3600)).await;

        bus.note_kind("run-7", Kind::Git);
        settle(DEFAULT_COALESCE_WINDOW).await;
        assert_eq!(frames(drained(&mut rx, &key)).len(), 1, "the leading edge");

        settle(Duration::from_secs(5)).await;
        bus.note_kind("run-9", Kind::Git);
        settle(Duration::from_secs(24)).await;
        assert!(
            frames(drained(&mut rx, &key)).is_empty(),
            "the cooldown the first push started has not ended"
        );

        settle(Duration::from_secs(2)).await;
        assert_eq!(
            frames(drained(&mut rx, &key)),
            vec![json!({
                "type": "changes",
                "subscription_id": "s-bg",
                // run-7's row rides this interval: its git went out on the
                // last one, and a row moves with its checkout.
                "items": [{ "entity_id": "run-7", "state": {} }, { "entity_id": "run-9", "git": {} }],
            })],
            "thirty seconds after the push, not thirty-five after the note"
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
        assert_eq!(bus.flush(), 1, "one frame");
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
        assert_eq!(item.files.names.len(), FILES_PER_FLUSH);
        assert!(item.files.truncated, "past the cap the list means refetch");
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

    /// What a session holds, and `changes.unsubscribe`, per session.
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
    /// A kind this bridge does not know refuses the WHOLE subscription — it is
    /// not dropped and the rest served.
    ///
    /// `KindSet` is a transparent `BTreeSet<Kind>`, so an unknown variant fails
    /// the set, which fails the spec, which is `invalid_params` for the call.
    /// That is the opposite of how the v1 facade treats an unknown FIELD, which
    /// it drops silently — and the difference matters to a client: asking an
    /// older bridge for a newer kind costs it the whole subscription, including
    /// the kinds it does understand. A client should ask for what the greeting
    /// advertises rather than for what it hopes is there.
    #[test]
    fn a_kind_this_bridge_does_not_know_refuses_the_whole_subscription() {
        let refused: Result<SubscriptionSpec, _> = serde_json::from_value(json!({
            "subscription_id": "s-inbox",
            "scope": { "kind": "entity", "id": "proj-1" },
            "kinds": ["state", "thread", "sandwiches"],
        }));
        let refused = refused.expect_err("a kind that names nothing");
        assert!(
            refused.to_string().contains("sandwiches"),
            "the refusal names the kind it did not know: {refused}"
        );

        // And the type's refusal survives the facade: `api::v1::parse_params`
        // rewrites only the missing-field case and passes everything else
        // through, naming the unknown kind AND the ones this bridge serves.
        // `changes.subscribe` itself reads its kinds as `KindNames` first, so
        // its refusal names EVERY unknown kind in `details.kinds` and a client
        // drops them and re-subscribes with the rest (api/v1/changes.rs).
        let wire = crate::api::v1::parse_params::<SubscriptionSpec>(&json!({
            "subscription_id": "s-inbox",
            "scope": { "kind": "entity", "id": "proj-1" },
            "kinds": ["state", "thread", "sandwiches"],
        }))
        .expect_err("the facade refuses it too");
        assert_eq!(wire.code(), "invalid_params");
        assert!(wire.message().contains("sandwiches"), "{}", wire.message());
        for served in Kind::ALL {
            assert!(
                wire.message().contains(served.as_str()),
                "the refusal lists {}, which this bridge does serve: {}",
                served.as_str(),
                wire.message()
            );
        }

        // And the kinds it DOES know still parse beside each other, so the
        // refusal above is about the unknown one and not about the list.
        let accepted: SubscriptionSpec = serde_json::from_value(json!({
            "subscription_id": "s-inbox",
            "scope": { "kind": "entity", "id": "proj-1" },
            "kinds": ["state", "thread", "tasks"],
        }))
        .expect("every kind this bridge advertises");
        assert!(accepted.kinds.contains(Kind::Tasks));
    }

    #[test]
    fn a_subscription_spec_round_trips_through_the_wire_form() {
        let wire = json!({
            "subscription_id": "s-focus",
            "scope": { "kind": "entity", "id": "run-7" },
            "kinds": ["state", "thread", "git", "files", "terminals", "tasks"],
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
                        ..ThreadTip::default()
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

    /// Every entity a lookup was asked about, and the cursors it was asked
    /// with — what the cursor-table tests read back.
    type AskedCursors = Arc<Mutex<Vec<(String, Vec<(String, u64)>)>>>;

    /// One conversation per entity — `run-7` holds `a7` — standing at
    /// sequence 5, echoing back the cursor it was asked with and recording
    /// it.
    fn thread_cursor_source(seen: AskedCursors) -> FactsSource {
        Arc::new(move |requests: &[FactsRequest]| {
            requests
                .iter()
                .map(|request| {
                    seen.lock()
                        .unwrap()
                        .push((request.entity_id.clone(), request.thread_after.clone()));
                    let agent = request.entity_id.replace("run-", "a");
                    let since = request
                        .thread_after
                        .iter()
                        .find(|(id, _)| *id == agent)
                        .map(|(_, sequence)| *sequence);
                    EntityFacts {
                        entity_id: request.entity_id.clone(),
                        threads: vec![ThreadTip {
                            agent_id: agent,
                            last_sequence: 5,
                            items: Vec::new(),
                            since_sequence: since,
                            ..ThreadTip::default()
                        }],
                        ..EntityFacts::default()
                    }
                })
                .collect()
        })
    }

    /// One entity's `thread` item out of a frame history.
    fn thread_of(sent: &[Value], entity_id: &str) -> Value {
        sent.iter()
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .find(|item| item["entity_id"] == entity_id)
            .map(|item| item["thread"].clone())
            .unwrap_or_else(|| panic!("no item for {entity_id}: {sent:?}"))
    }

    /// A lookup is asked what ONE entity's conversations have already been
    /// sent, not every cursor the subscription holds. An inbox subscription
    /// covers every entity on the device, and handing all of their cursors
    /// to each entity's lookup makes a flush cost the whole device.
    #[tokio::test(start_paused = true)]
    async fn a_lookup_is_asked_only_for_the_conversations_of_the_entity_it_names() {
        let seen: AskedCursors = Arc::new(Mutex::new(Vec::new()));
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(|| vec!["run-7".into(), "run-8".into()]),
            thread_cursor_source(Arc::clone(&seen)),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Thread].into_iter().collect(),
                ..spec("s-inbox", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );

        bus.note_kind("run-7", Kind::Thread);
        bus.note_kind("run-8", Kind::Thread);
        bus.flush();
        drained(&mut rx, &key);
        settle(DEFAULT_COALESCE_WINDOW).await;
        seen.lock().unwrap().clear();

        bus.note_kind("run-7", Kind::Thread);
        bus.note_kind("run-8", Kind::Thread);
        bus.flush();
        drained(&mut rx, &key);

        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![
                ("run-7".to_string(), vec![("a7".to_string(), 5)]),
                ("run-8".to_string(), vec![("a8".to_string(), 5)]),
            ]
        );
    }

    /// A cursor leaves with the entity it belonged to. A run that finished
    /// or was deleted is named in the board item's `removed`, the client
    /// drops its conversation, and the bridge drops what it remembered
    /// having sent for it.
    #[tokio::test(start_paused = true)]
    async fn a_cursor_leaves_with_the_entity_that_left_the_board() {
        let listed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec!["run-7".into()]));
        let board = Arc::clone(&listed);
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(move || board.lock().unwrap().clone()),
            thread_cursor_source(Arc::new(Mutex::new(Vec::new()))),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State, Kind::Thread].into_iter().collect(),
                ..spec("s-inbox", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );

        bus.note_board();
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        drained(&mut rx, &key);

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        let held = frames(drained(&mut rx, &key));
        assert_eq!(
            thread_of(&held, "run-7")[0]["since_sequence"],
            json!(5),
            "the cursor is held while the run is on the board: {held:?}"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        listed.lock().unwrap().clear();
        bus.note_board();
        bus.flush();
        let left = frames(drained(&mut rx, &key));
        assert_eq!(
            left[0]["items"][0]["state"]["removed"],
            json!(["run-7"]),
            "{left:?}"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-7", Kind::Thread);
        bus.flush();
        let again = frames(drained(&mut rx, &key));
        assert_eq!(
            thread_of(&again, "run-7")[0]["since_sequence"],
            Value::Null,
            "the cursor left with the run: {again:?}"
        );
    }

    /// The cursor table is bounded whatever the board does. Past
    /// [`THREAD_CURSOR_ENTITIES`] the coldest entity is dropped, and the
    /// next item for it is the tip alone — the client pages forward once
    /// rather than the bridge growing a cursor per entity it has ever seen.
    #[tokio::test(start_paused = true)]
    async fn the_cursor_table_drops_the_coldest_entity_past_its_cap() {
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            thread_cursor_source(Arc::new(Mutex::new(Vec::new()))),
        );
        let (sender, mut rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::Thread].into_iter().collect(),
                ..spec("s-inbox", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );

        bus.note_kind("run-0", Kind::Thread);
        bus.flush();
        drained(&mut rx, &key);

        settle(DEFAULT_COALESCE_WINDOW).await;
        for n in 1..=THREAD_CURSOR_ENTITIES {
            bus.note_kind(&format!("run-{n}"), Kind::Thread);
        }
        bus.flush();
        drained(&mut rx, &key);

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_kind("run-0", Kind::Thread);
        bus.note_kind("run-1", Kind::Thread);
        bus.flush();
        let sent = frames(drained(&mut rx, &key));
        assert_eq!(
            thread_of(&sent, "run-1")[0]["since_sequence"],
            json!(5),
            "the warm entity kept its cursor: {sent:?}"
        );
        assert_eq!(
            thread_of(&sent, "run-0")[0]["since_sequence"],
            Value::Null,
            "the coldest one was dropped to keep the table bounded: {sent:?}"
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
                        ..ThreadTip::default()
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
                    // The body is never pushed, however small: the item says
                    // the diff moved and how big it is, and the surface that
                    // shows one asks for it.
                    "diff": Value::Null,
                    "diff_bytes": 41,
                },
            })
        );
    }

    /// A working tree past the cap reads exactly like every other one now —
    /// the size, and no body — which is the shape this client has always
    /// known how to answer.
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

    /// A bus whose lookup answers the project list for any request that asks
    /// for it — the shape every list-latch test measures against.
    fn bus_listing_one_project() -> Arc<ChangeBus> {
        ChangeBus::with_sources(
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
        )
    }

    /// The list latch is per subscription. Two tabs watching the board at
    /// different cadences both hear that the project list moved: the one
    /// that flushes first does not swallow the news for the one still
    /// waiting out its batch.
    #[tokio::test(start_paused = true)]
    async fn a_moved_list_rides_every_subscription_not_the_first_to_flush() {
        let bus = bus_listing_one_project();
        let (fast, mut fast_rx, fast_key) = SessionSender::observable("s-fast");
        let (slow, mut slow_rx, slow_key) = SessionSender::observable("s-slow");
        bus.subscribe(
            &fast,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec("s-inbox", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );
        bus.subscribe(
            &slow,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec(
                    "s-background",
                    Scope::All,
                    Mode::Batch(Duration::from_secs(30)),
                    Priority::Background,
                )
            },
        );

        // A board item first, so the batch tab is inside the cooldown that
        // item's push started rather than at its leading edge.
        bus.note_board();
        bus.flush();
        drained(&mut fast_rx, &fast_key);
        drained(&mut slow_rx, &slow_key);
        settle(DEFAULT_COALESCE_WINDOW).await;

        bus.note_board_lists(BoardLists::PROJECTS);
        bus.flush();
        let listed = json!([{ "project_id": "proj-1" }]);
        assert_eq!(
            frames(drained(&mut fast_rx, &fast_key))[0]["items"][0]["state"]["projects"],
            listed,
            "the realtime tab is due now"
        );
        assert!(
            frames(drained(&mut slow_rx, &slow_key)).is_empty(),
            "the batch tab is not due for another half minute"
        );

        settle(Duration::from_secs(30)).await;
        bus.flush();
        let sent = frames(drained(&mut slow_rx, &slow_key));
        assert_eq!(
            sent[0]["items"][0]["state"]["projects"], listed,
            "the batch tab is told the list moved too: {sent:?}"
        );
    }

    /// A board item's lists, from a lookup that answers a different thing
    /// each time it is asked. The first answer is the state a flush hits
    /// when the lookup cannot do its job; the second is the ordinary one.
    fn bus_answering_lists(first: Value, then: Value) -> Arc<ChangeBus> {
        let asked = Arc::new(AtomicU64::new(0));
        ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(Vec::new),
            Arc::new(move |requests: &[FactsRequest]| {
                let answer = match asked.fetch_add(1, Ordering::SeqCst) {
                    0 => first.clone(),
                    _ => then.clone(),
                };
                requests
                    .iter()
                    .map(|request| EntityFacts {
                        entity_id: request.entity_id.clone(),
                        state: (!request.lists.is_empty()).then(|| answer.clone()),
                        ..EntityFacts::default()
                    })
                    .collect()
            }),
        )
    }

    /// One `all`-scope subscription over the board, on a bus a test drives
    /// by hand.
    fn board_watcher(
        bus: &Arc<ChangeBus>,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
        String,
    ) {
        let (sender, rx, key) = SessionSender::observable("s-1");
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                kinds: [Kind::State].into_iter().collect(),
                ..spec("s-all", Scope::All, Mode::Realtime, Priority::Foreground)
            },
        );
        (rx, key)
    }

    /// The latch is taken when the frame is built, BEFORE the lookup runs.
    /// A lookup that cannot answer — the state gone, the board fact missing
    /// — would otherwise swallow the only news the client was going to get
    /// that its cached list is stale, and nothing would re-arm it. The list
    /// stays latched and rides the next board item instead.
    #[tokio::test(start_paused = true)]
    async fn a_list_the_lookup_could_not_answer_stays_latched() {
        let listed = json!([{ "project_id": "proj-1" }]);
        let bus = bus_answering_lists(json!({}), json!({ "projects": listed.clone() }));
        let (mut rx, key) = board_watcher(&bus);

        bus.note_board_lists(BoardLists::PROJECTS);
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["state"],
            json!({ "revision": 1 }),
            "the lookup had nothing to carry"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        // A row moved. No list was noted this time — the one carried here is
        // the one the failed lookup owes.
        bus.note_board();
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["state"],
            json!({ "revision": 2, "projects": listed })
        );
    }

    /// A list answered with something that is not a list is not one. The
    /// board item's `projects` and `workspaces` are arrays by contract, and
    /// a null on the wire is a client's `for (const w of …)` throwing rather
    /// than a client learning its cache is stale. The key is left off, and
    /// the latch stays up for an answer that is a list.
    #[tokio::test(start_paused = true)]
    async fn a_list_answered_with_a_null_is_not_shipped_as_one() {
        let listed = json!([{ "workspace_id": "ws-1" }]);
        let bus = bus_answering_lists(
            json!({ "workspaces": Value::Null }),
            json!({ "workspaces": listed.clone() }),
        );
        let (mut rx, key) = board_watcher(&bus);

        bus.note_board_lists(BoardLists::WORKSPACES);
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["state"],
            json!({ "revision": 1 }),
            "a null is not the list the client was promised"
        );

        settle(DEFAULT_COALESCE_WINDOW).await;
        bus.note_board();
        bus.flush();
        assert_eq!(
            frames(drained(&mut rx, &key))[0]["items"][0]["state"],
            json!({ "revision": 2, "workspaces": listed })
        );
    }

    /// The project and workspace lists ride the board item only when the
    /// change that noted it moved one of them — a row moving re-sends no
    /// list at all.
    #[tokio::test(start_paused = true)]
    async fn a_board_item_carries_the_lists_only_when_they_moved() {
        let bus = bus_listing_one_project();
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

    /// A flush serializes and encrypts every frame it owes, once per
    /// subscriber, and a frame can be large: a checkout's status and log, its
    /// root listing, the conversation tails (the diff body no longer rides,
    /// only its size). That is CPU on whichever thread runs it, so the daemon
    /// runs the flusher on its push runtime. Observe the worker immediately
    /// after each frame is serialized and encrypted; the main runtime's only
    /// worker must not do that work. On the main runtime a flush of 32 frames
    /// of a 260 KB status held that worker 2.3 s (#131).
    ///
    /// The frames here are small on purpose. Which thread encrypts does not
    /// depend on how big the frame is, and the check is the thread's name, not
    /// a clock. A 260 KB frame per subscriber made this test take 2.6 s idle
    /// and over 20 s on two loaded cores, so load failed it (#252).
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_heavy_flush_encrypts_off_the_main_runtime() {
        const SUBSCRIBERS: usize = 32;
        let patch = "M src/lib.rs\n";
        let facts: FactsSource = Arc::new(move |requests: &[FactsRequest]| {
            requests
                .iter()
                .map(|request| EntityFacts {
                    entity_id: request.entity_id.clone(),
                    status_key: Some("9f3c1a0b7e2d4c55".into()),
                    status: Some(json!({ "files": patch })),
                    ..EntityFacts::default()
                })
                .collect()
        });
        let bus = ChangeBus::with_sources(
            DEFAULT_COALESCE_WINDOW,
            Arc::new(|| vec!["run-7".to_string()]),
            facts,
        );
        let receivers: Vec<_> = (0..SUBSCRIBERS)
            .map(|n| {
                let (sender, rx, _) = SessionSender::observable(format!("s-{n}"));
                bus.subscribe(
                    &sender,
                    spec(
                        "s-focus",
                        Scope::Entity("run-7".into()),
                        Mode::Realtime,
                        Priority::Foreground,
                    ),
                );
                (sender, rx)
            })
            .collect();
        let (observe, mut delivered) = tokio::sync::mpsc::unbounded_channel();
        *bus.delivered_test_threads.lock().unwrap() = Some(observe);
        let push = crate::liveness::DedicatedRuntime::push().unwrap();
        ChangeBus::spawn_flusher_on(Arc::clone(&bus), Some(push.handle()));

        bus.note_kind("run-7", Kind::Git);
        // A guard against a flusher that never runs, not a budget: the whole
        // flush is milliseconds of work.
        let workers = tokio::time::timeout(Duration::from_secs(20), async {
            let mut workers = Vec::with_capacity(SUBSCRIBERS);
            for _ in 0..SUBSCRIBERS {
                workers.push(
                    delivered
                        .recv()
                        .await
                        .expect("delivery observer remains alive"),
                );
            }
            workers
        })
        .await
        .expect("every subscriber was sent its frame within 20 seconds");
        assert!(
            receivers.iter().all(|(_, rx)| !rx.is_empty()),
            "every subscriber received its encrypted frame"
        );
        assert!(
            workers
                .iter()
                .all(|worker| worker.as_deref() == Some("bridge-push")),
            "subscription frames were encrypted on {workers:?} instead of the push runtime"
        );
        push.stop();
    }
}
