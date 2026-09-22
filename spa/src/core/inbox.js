// The inbox's pure model: which work items the rail lists, in what order, what
// each row says, and what Done promises before it destroys anything.
//
// One list across every project — the inbox is the user's, not a project's, so
// there are no per-project blocks and no worktree fold. (The rail's other face,
// core/inboxProjects.js, gathers these same rows under their projects; the rows
// and their order are decided here either way.) The rows are issues, branches
// and captures. An issue a branch is implementing right now is not a
// row: one piece of work, one row, and the branch is where that work is. Delete
// that branch without merging and the issue is work again, so it comes back —
// with an event on its conversation naming the branch it lost (the bridge writes
// that; this list just stops hiding it). Nothing that is over is ever a row.
//
// The order is the anchor's, oldest first. An anchor is seeded when a thing is
// created and moved only by the user picking the work back up, so an agent
// working all night, a diff landing and a doc being read leave a row exactly
// where it is. The top of the list is what has been waiting longest; a fresh
// pickup appends to the bottom instead of shoving everything down.
//
// A row is two lines: what it is, with the unread count pulled to the right
// edge, and what it weighs — files touched, ahead/behind, +/−. A row with
// nothing to weigh yet says so.
//
// Everything that has said nothing for a day is partitioned off into Recent at
// the end of the list: still there, just not what today is about. It starts
// shut, always — opening it is the user's, and holds until they shut it.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { carriesWatching } from "./trackerWatch.js";
import { entityIdOf } from "./entityId.js";
import { ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT } from "./icons.js";
import { workspaceRoute } from "./projectModel.js";
import { standsOnProjectCheckout, workspaceDisplayName, workspaceRun, workspaceStatusText } from "./workspaceModel.js";

const DAY_MS = 24 * 3600 * 1000;

/** Work still local to every Git directory in a durable workspace. The bridge
 * omits the summary when even one repository cannot be read, so absence must
 * remain visibly unknown rather than looking like a clean workspace.
 *
 * A checkout the bridge could not build has no work to summarize at all, and
 * calling that an unavailable summary reads as a hiccup in the reporting rather
 * than as the thing that went wrong. It says what the toolbar's switcher says
 * for the same workspace. */
function workspaceFacts(workspace) {
  if (workspace.status === "failed") return workspaceStatusText(workspace);
  const summary = workspace.work_summary;
  const values = summary && [summary.pushes, summary.additions, summary.deletions];
  if (!values || values.some((value) => !Number.isSafeInteger(value) || value < 0)) return "Work summary unavailable";
  // Older bridges omit `behind`; its absence is unknown, not zero. Once a
  // bridge supplies it, however, it is part of the same all-or-nothing summary.
  const hasBehind = Object.hasOwn(summary, "behind");
  if (hasBehind && (!Number.isSafeInteger(summary.behind) || summary.behind < 0)) return "Work summary unavailable";
  return [`↑${summary.pushes}`, hasBehind ? `↓${summary.behind}` : "", `+${summary.additions}`, `−${summary.deletions}`].filter(Boolean).join(" ");
}

/** Line two of a row standing on a checkout Build does not own: that it is
 *  adopted, and the branch it has out. A summary would say what the work
 *  weighs, and nobody here has said what the work is. */
function adoptedCheckoutFacts(workspace) {
  const branch = (workspace.directories || []).map((directory) => directory.branch).find(Boolean);
  return branch ? `Adopted checkout · ${branch}` : "Adopted checkout";
}

const firstText = (...values) => values.find(Boolean) || "";

/** How a workspace row is named in the rail's DOM and in every set the wiring
 *  keeps beside it. Minted here, beside `entryKeyOf`, so the row a paint draws
 *  and the row an error or a live-key sweep names are the same row. */
export const workspaceEntryKey = (workspace) => `workspace:${workspace.workspaceKey}`;

function toWorkspaceEntry(workspace, projectNames, conversation) {
  const activity = conversation || { working: workspace.status === "active" };
  // The workspace list carries the conversation owner independently of the
  // board row. On a cold replay it can therefore lead the row that carries the
  // roster; keep that owner so separately cached conversation records are
  // addressable while the roster record is still landing.
  const entityId = entityIdOf(conversation) || workspace.entity_id || workspace.run_id || null;
  // A checkout Build only adopted, with nobody talking in it: somebody else's
  // folder, listed so it can be opened. There is no work to summarize and
  // nothing for Done to remove, so the row says what it is instead.
  const adopted = workspace.managed === false && !entityId;
  return {
    key: workspaceEntryKey(workspace),
    kind: "workspace",
    workspaceId: workspace.id,
    workspaceKey: workspace.workspaceKey,
    deviceId: workspace.deviceId,
    projectId: workspace.project_id,
    projectKey: workspace.projectKey,
    project: firstText(projectNames.get(workspace.projectKey), workspace.project, workspace.project_id),
    // What the user called it, never the slug its folder and its branch were
    // derived from (core/workspaceModel.js) — and the tooltip says the same
    // thing the row does, because the checkout's path is machinery rather than
    // a longer version of the name.
    name: workspaceDisplayName(workspace),
    title: workspaceDisplayName(workspace),
    entityId,
    state: entryState(activity),
    unreadCount: activity.unread_count || 0,
    reason: unreadReasonText(activity.unread_reason, "branch"),
    muted: !!activity.muted,
    dismissed: !!activity.dismissed,
    working: !!activity.working,
    // Done removes the workspace, so only the bridge decides when it is
    // offered: it is the one that can see every repository and every agent at
    // once. A missing verdict is not a yes.
    // What the workspace is mounted out of. It is on the list entry and
    // nowhere else — a board row is a work item and says nothing about
    // sources — and a conversation's file references are read against it
    // (core/threadLinks.js).
    directories: workspace.directories || [],
    ready: workspace.status === "ready",
    canFinish: workspace.status === "ready" && workspace.can_finish === true,
    finishBlockers: workspace.finish_blockers || [],
    adopted,
    facts: adopted ? adoptedCheckoutFacts(workspace) : workspaceFacts(workspace),
    route: workspaceRoute(workspace),
    // The conversation owns the inbox anchor whenever there is one: this is
    // the same user-pickup ordering used by ordinary work rows. The workspace
    // creation date is only the fallback for one that has never spoken.
    anchorMs: ms(firstText(activity.anchor, workspace.created_at, workspace.updated_at)),
    lastActivityMs: ms(workspace.updated_at),
  };
}

/** Every device's workspaces as rows, ordered together by the inbox anchor. A
 * workspace belongs to one machine, so it
 *  is named — and its project and its conversation are looked up — by the
 *  account-wide names the feed stamped (core/deviceKey.js): two machines each
 *  hold a `proj-1`, and a run id on one says nothing about the other. */
export function workspaceEntries(workspaces = [], projects = [], items = []) {
  const projectsByKey = new Map(projects.map((project) => [project.projectKey, project]));
  const projectNames = new Map(projects.map((project) => [project.projectKey, project.name]));
  const conversations = new Map(items.filter((item) => item.kind === "branch" && entityIdOf(item))
    .map((item) => [JSON.stringify([item.projectKey, entityIdOf(item)]), item]));
  // Nothing finished is a row, and neither is the project's own checkout
  // (core/workspaceModel.js): it is the template the rows were cut from.
  const listed = (workspace) =>
    workspace.status !== "finished" && !standsOnProjectCheckout(workspace, projectsByKey.get(workspace.projectKey));
  return workspaces.filter(listed).map((workspace) => {
    const owner = workspace.entity_id || workspace.run_id || workspace.id;
    const conversation = conversations.get(JSON.stringify([workspace.projectKey, owner])) || workspaceRun(workspace, items);
    return toWorkspaceEntry(workspace, projectNames, conversation);
  }).sort(byAnchor);
}

/** How long a row can say nothing before it belongs to Recent rather than to
 *  the list proper. */
export const RECENT_AFTER_MS = DAY_MS;

/** Line two, for a row that has not done anything measurable yet. */
export const GETTING_STARTED = "Getting started";

/** The states that mean the work is over. Nothing marked done is ever a row. */
const FINISHED_STATES = new Set(["merged", "abandoned", "archived"]);

/** Whether a row's state means the work is over. The cache asks it of a pushed
 *  row: a workspace nobody can come back to keeps none of its data. */
export const isFinishedState = (state) => FINISHED_STATES.has(state);

/** Three states, one glance. Unread wins over working: an entry that asked you
 *  something while it kept going is still asking. Working is "any agent is
 *  running on it" — the bridge resolves that for every kind of row. */
export function entryState(item) {
  if (item.unread) return "unread";
  if (item.working) return "working";
  return "inactive";
}

// Why an entry needs you, keyed by the attention event that made it unread
// (a ThreadEventKind wire token, or `agent_message`). Salvaged from the
// notification cards this replaces, in the inbox's shorter voice.
const REASON_COPY = {
  agent_message: "The agent sent a message",
  done: "The agent finished — review the work",
  blocked: "Blocked — the agent needs you",
  review_blocked: "Blocked on your review notes",
  run_failed: "The run failed",
  stage_failed: "A stage failed",
  recovery_failed: "The checkout could not be recovered",
  idle_unreported: "The session went idle without reporting",
  interrupted: "The session was interrupted",
  merged: "Merged",
  abandoned: "Abandoned",
};

// An issue speaks about its own work: the same event kinds, its own words.
const ISSUE_REASON_COPY = {
  done: "The draft is ready to review",
  blocked: "Planning is blocked — the agent needs you",
  run_failed: "Planning failed",
  interrupted: "Planning was interrupted",
  idle_unreported: "Planning went idle without reporting",
};

/** The sentence an unread entry shows. "" when the entry is read; a generic
 *  line when the bridge names an event this client has never heard of — a new
 *  attention kind must still read as "this needs you", never as nothing. */
export function unreadReasonText(reason, kind) {
  if (!reason) return "";
  if (kind === "issue" && ISSUE_REASON_COPY[reason]) return ISSUE_REASON_COPY[reason];
  return REASON_COPY[reason] || "Something needs you";
}

// ---- lifecycle verbs in flight -------------------------------------------------
//
// The daemon runs a create, an adopt or a discard with its state lock released
// and puts a row on the board for the length of that git (`board.list`'s
// `pending`). The row stands where the record will be, under the id the record
// will carry, so the list never goes quiet while work is being made — and the
// record replaces it in place when it lands.

/** What a row in flight says on its second line. A state this client has never
 *  heard of still reads as work — the dot says so — it just has no line of its
 *  own to say. */
const PENDING_LINE = {
  creating: "Creating…",
  discarding: "Removing…",
  updating: "Updating…",
};

/** One pending row as a feed row: a branch under the id its record will settle
 *  as, working, dated by nothing — an anchor it has not earned would put it
 *  somewhere in the list it has no claim to.
 *
 *  `placeholder` is what tells this row apart from a card the same verb is
 *  running ON: there is nothing behind it yet, so it opens nowhere until its
 *  record lands under the same key. A standing card keeps its own surface for
 *  the whole of the verb — the reader's issue or run does not stop opening
 *  because a plan workspace is being cut in it. */
const pendingItem = (row) => ({
  kind: "branch",
  entity_id: row.entity_id,
  deviceId: row.deviceId,
  project_id: row.project_id,
  projectKey: row.projectKey,
  project: row.project,
  title: row.title,
  branch: row.branch || null,
  // How the checkout being cut is isolated, said the way the card that lands
  // in this row's place says it. A verb cutting nothing names none.
  isolation: row.isolation || null,
  pending: row.state,
  placeholder: true,
  working: true,
});

/**
 * The board's rows for the lifecycle verbs running right now, merged into the
 * rows the daemon already has.
 *
 * A row names two ids and a card may be listed under either: the record it will
 * settle as (`entity_id` — a run, an issue) and the checkout it holds
 * (`checkout_id`, a worktree hash). An adopt leaves the run on the board while
 * it claims the checkout, and a planning workspace holds no checkout at all, so
 * a card matching either id is the card this verb is running on and is said ON —
 * one piece of work, one row. A verb making a card that is not there yet stands
 * on its own until its record lands under the same id, at which point the record
 * is the row and the placeholder is gone.
 *
 */
export function mergePendingRows(items = [], pending = []) {
  const rows = [...(items || [])];
  const cardOf = (row) => (item) => {
    const id = entityIdOf(item);
    return id !== null && (id === row.entity_id || id === row.checkout_id);
  };
  for (const row of pending || []) {
    const standing = rows.findIndex(cardOf(row));
    if (standing < 0) rows.push(pendingItem(row));
    else rows[standing] = { ...rows[standing], pending: row.state, working: true };
  }
  return rows;
}

/** Where each kind of entry opens, looked up rather than walked. A branch is
 *  (device, project, branch name); an issue is its own surface on the machine
 *  holding it. A checkout with no branch is nameable by no URL, so it opens
 *  nowhere until it is on one.
 *
 *  A capture opens wherever it was routed — it answers as the work item it
 *  names. Until it is routed it opens its own decision page: what to do with it
 *  is a question, and a question deserves a surface. One this client is still
 *  holding has no record to decide about, so it opens nowhere. */
/// A watched issue of the tracker (#65). NOT `issue`, which is the legacy
/// multi-stage issue and opens the plan/stages page — two different things
/// that would otherwise share a word and a row.
export const TRACKER_ISSUE = "tracker_issue";

const OPENS_AT = {
  capture: (item) => {
    if (item.routing) return entryRoute({ ...item, kind: item.routing.kind === "issue" ? "issue" : "branch" });
    return item.state === "queued" ? null : { name: "capture", id: item.capture_id };
  },
  issue: (item) =>
    item.issue_id ? { name: "issue", deviceId: item.deviceId, projectId: item.project_id, id: item.issue_id } : null,
  [TRACKER_ISSUE]: (item) =>
    item.issue_id
      ? { name: "trackerIssue", deviceId: item.deviceId, projectId: item.project_id, issueId: item.issue_id }
      : null,
  branch: (item) =>
    item.branch ? { name: "branch", deviceId: item.deviceId, projectId: item.project_id, branch: item.branch, tab: "changes" } : null,
};

/** Where an entry opens, and null for a row with nowhere to go. */
export function entryRoute(item) {
  // A row standing in for a card that does not exist yet opens nowhere: there
  // is nothing on the other side of it. It opens itself the moment its record
  // lands under the same key. A card that is already there keeps its surface
  // however busy the daemon is with it.
  if (item.placeholder) return null;
  // A checkout is the kind a row wears when it says nothing else.
  return (OPENS_AT[item.kind] || OPENS_AT.branch)(item);
}

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

/** What names a row in the DOM — the one name every row has. Usually the
 *  entity id; a checkout nobody has claimed has no entity, and it is still a
 *  row you open and clear.
 *
 *  A row with no entity is named by its project, and a project is only named
 *  once you also say which device it is on (core/deviceKey.js): two machines
 *  both call their first project `proj-1`, and their `main` rows are two rows.
 *  So the key carries the projectKey the feed stamped, in the shape it always
 *  had. Exported so the wiring can match a feed row to the keys it is holding. */
export const entryKeyOf = (item) => {
  if (item.kind === "capture") return `capture:${item.capture_id}`;
  // An issue is one issue wherever it is listed: its own id names the row.
  if (item.kind === TRACKER_ISSUE) return `${TRACKER_ISSUE}:${item.issue_id}`;
  return entityIdOf(item) || (item.kind === "issue" ? `issue:${item.projectKey}` : `branch:${item.projectKey}:${item.branch}`);
};

/**
 * How `entity.dismiss` names the row being cleared. An entity is named by its
 * id; a row with no entity is named by what it is, a branch in the project. In
 * either case the bridge records the current message boundary, and a later user
 * or agent message clears that marker. Null means the row cannot be named on
 * the wire.
 */
export function dismissParamsOf(entry) {
  if (entry.entityId) return { entity_id: entry.entityId };
  if (entry.projectId && entry.branch) return { project_id: entry.projectId, branch: entry.branch };
  return null;
}

/**
 * Line two: what this row weighs. Files touched, how it stands against the ref
 * it is compared with, and the +/− it carries — in that order, and only the
 * parts it actually has. "" when there is nothing to say, which the row renders
 * as GETTING_STARTED.
 *
 * The files and the +/− are the UNCOMMITTED tree, not the branch against its
 * base: the inbox says what is waiting on the reader, and a long branch's
 * lifetime totals bury that under thousands of lines they have already seen.
 * The branch's whole weight is what the Changes surface opens on. Ahead/behind
 * stay as they are — they are the row's sync facts, not its change facts.
 */
// eslint-disable-next-line complexity -- ratchet: entryFactsText is at 14, cap 10 — reduce it, then drop this line
export function entryFactsText(item) {
  // What is being done to the row outranks what it weighs: a checkout being cut
  // has nothing to weigh, and one being removed is about to have nothing.
  if (item && item.pending) return PENDING_LINE[item.pending] || "";
  const stat = item && item.stat;
  if (!stat) return "";
  const uncommitted = stat.uncommitted || {};
  const parts = [];
  const files = uncommitted.files_changed || 0;
  if (files > 0) parts.push(`${files} file${files === 1 ? "" : "s"}`);
  const sync = [];
  if (stat.ahead) sync.push(`↑${stat.ahead}`);
  if (stat.behind) sync.push(`↓${stat.behind}`);
  if (sync.length) parts.push(sync.join(" "));
  if (uncommitted.insertions || uncommitted.deletions) {
    parts.push(`+${uncommitted.insertions || 0} −${uncommitted.deletions || 0}`);
  }
  return parts.join(" · ");
}

/** What Done on this row would cost, in the bridge's own words. The bridge
 *  never refuses a Done: it says what is about to be lost, and the user
 *  confirms through it. */
const warningsOf = (item) => ((item.finish && item.finish.warnings) || []).map((warning) => warning.message);

// What a capture is doing, said in the order it happens. The queued state is
// the client's own: a capture it is holding because the device is away.
const CAPTURE_STATUS = {
  queued: "Waiting for your device",
  unrouted: "Waiting to be routed",
  routing: "Deciding where this goes",
  failed: "Routing failed",
};

/** The line a capture row shows about its route: where it went, else what is
 *  happening to it. A question nobody has answered is not the router working —
 *  it is the router waiting on the user, and saying "waiting to be routed"
 *  there names the wrong party. */
export function captureStatusText(entry) {
  if (entry.captureState === "routed" && entry.routedTo) {
    return `→ ${entry.routedTo.project} as ${entry.routedTo.kind}`;
  }
  if (entry.question) return "Waiting for your answer";
  if (entry.progress) return entry.progress;
  return CAPTURE_STATUS[entry.captureState] || "";
}

/** One capture row, as the inbox reads it. A capture holds no conversation and
 *  no agents, so what it needs is read off the record: the router's unanswered
 *  question is the reason, and a route that gave up is a retry. */
// eslint-disable-next-line complexity -- ratchet: toCaptureEntry is at 20, cap 10 — reduce it, then drop this line
function toCaptureEntry(item) {
  const question = item.question && !item.question.answer ? item.question.text : "";
  const routing = item.routing || null;
  return {
    key: entryKeyOf(item),
    entityId: null,
    kind: "capture",
    captureId: item.capture_id,
    captureState: item.state,
    // Which machine answered for this row, and the account-wide name of the
    // project it is on — both stamped by the feed, both carried as they came.
    deviceId: item.deviceId,
    projectKey: item.projectKey,
    projectId: item.project_id || "",
    project: item.project || "",
    branch: item.branch || null,
    issueId: item.issue_id || null,
    name: item.title || "(nothing said)",
    title: item.title || "(nothing said)",
    text: item.text || "",
    state: item.unread ? "unread" : item.state === "routed" ? "inactive" : "working",
    // The status word covers queued/unrouted captures too; only the feed's
    // working bit means an agent is actually active for recency purposes.
    working: !!item.working,
    reason: question || (item.unread_reason === "routing_failed" ? "Routing failed" : ""),
    question,
    progress: item.progress || "",
    routedTo: routing ? { project: item.project || item.project_id, kind: routing.kind } : null,
    unreadCount: item.unread_count || 0,
    muted: false,
    // A capture leaves the inbox by being routed, so it is never cleared.
    dismissed: false,
    canFinish: false,
    merged: false,
    warnings: [],
    facts: "",
    route: entryRoute(item),
    // What the user said is when they said it. A capture this client is still
    // holding has no record on the daemon and so no anchor of its own yet — it
    // is dated by when it was taken, which is the anchor the work it becomes
    // will inherit anyway.
    anchorMs: ms(item.anchor || item.created_at),
    lastActivityMs: ms(item.last_activity || item.created_at),
  };
}

/**
 * A watched issue as a row (#65).
 *
 * Its own shape rather than a detour through the branch/issue mapping below:
 * a tracker issue has no state machine, no checkout and no agent working in
 * it, and the fields that row is built from are all about those. What it has
 * is a number, a title, and the last thing that happened to it.
 *
 * The subtitle arrives composed (#61's form) and is shown as given. The bridge
 * knows the event; a second sentence assembled here would be free to drift
 * from the one the issue's own timeline shows.
 */
const issueTitleOf = (item) => item.title || "(untitled)";

/** The number is how a person says which issue, so it leads the line. */
const issueNameOf = (item) => (item.number ? `#${item.number} ${issueTitleOf(item)}` : issueTitleOf(item));

/** Where the row is, which every kind of row says the same way. */
const issuePlaceOf = (item) => ({
  deviceId: item.deviceId,
  projectKey: item.projectKey,
  projectId: item.project_id,
  project: item.project || item.project_id || "",
  deviceName: item.deviceName || null,
});

/** The fields a row carries about a checkout, which an issue has none of. A
 *  tracker issue has no branch, no agent working in it and nothing to finish. */
const NOT_A_CHECKOUT = Object.freeze({
  branch: null,
  working: false,
  reason: "",
  pending: null,
  placeholder: false,
  canFinish: false,
  merged: false,
  warnings: [],
});

/**
 * When a watched issue last moved.
 *
 * The row's own `anchor` and `last_activity`, as every other row in this feed
 * carries them (#64, settled). The event it carries is the fallback: the two
 * say the same thing, and a row that dated neither would sort under everything
 * rather than where it belongs.
 */
function issueTimesOf(item) {
  const anchorMs = ms(item.anchor) ?? ms(item.last_event?.at);
  return { anchorMs, lastActivityMs: ms(item.last_activity) ?? anchorMs };
}

function toTrackerIssueEntry(item) {
  const times = issueTimesOf(item);
  const unread = item.unread || 0;
  return {
    ...NOT_A_CHECKOUT,
    ...issuePlaceOf(item),
    key: entryKeyOf(item),
    entityId: item.issue_id || null,
    kind: TRACKER_ISSUE,
    issueId: item.issue_id || null,
    number: item.number ?? null,
    name: issueNameOf(item),
    title: issueTitleOf(item),
    status: item.status || null,
    // An issue handed to the reader outranks one that merely moved — the one
    // departure from activity order, and only among the issue rows.
    assignedToUser: !!item.assigned_to_user,
    state: unread ? "unread" : "idle",
    unreadCount: unread,
    muted: !!item.muted,
    // Done on an issue row means the same as on a conversation: cleared until
    // the next event.
    dismissed: !!item.done_until_next,
    facts: item.last_event?.text || "",
    route: entryRoute(item),
    ...times,
  };
}

/// The kinds that are their own kind of row. Everything else is a checkout —
/// a branch or a legacy issue — which `toEntry` builds below. A table rather
/// than a chain of `if`s because each new kind would otherwise be one more
/// branch in a function that is already over the cap.
const ENTRY_BUILDERS = {
  capture: toCaptureEntry,
  [TRACKER_ISSUE]: toTrackerIssueEntry,
};

/** One board.list row, as the inbox reads it. */
// eslint-disable-next-line complexity -- ratchet: toEntry is at 14, cap 10 — reduce it, then drop this line
function toEntry(item) {
  const ownKind = ENTRY_BUILDERS[item.kind];
  if (ownKind) return ownKind(item);
  const state = entryState(item);
  const entityId = entityIdOf(item);
  // Line one is what the thing is CALLED: a branch by its branch name, an issue
  // by its own words. The goal a branch was cut for is a longer story, and it
  // is on the row's title where a second look finds it.
  const name = item.kind === "issue" ? item.title || "(untitled)" : item.branch || item.title || "(detached)";
  return {
    key: entryKeyOf(item),
    entityId,
    kind: item.kind,
    deviceId: item.deviceId,
    projectKey: item.projectKey,
    projectId: item.project_id,
    project: item.project || item.project_id || "",
    // The machine this row is on, said after the project name — only where two
    // machines use that name, which the list decides once (core/inboxView.js).
    deviceName: item.deviceName || null,
    branch: item.branch || null,
    issueId: item.issue_id || null,
    name,
    title: item.title || item.branch || "(untitled)",
    state,
    // Unread wins visually, but execution is an independent classification
    // input: an unread row can also have an agent actively working on it.
    working: !!item.working,
    reason: state === "unread" ? unreadReasonText(item.unread_reason, item.kind) : "",
    unreadCount: item.unread_count || 0,
    muted: !!item.muted,
    // The bridge's own word for "the user cleared this and no user or agent has
    // spoken since". The row remains accessible in Recent until a message
    // causes the bridge to remove this marker.
    dismissed: !!item.dismissed,
    // What the daemon is doing to this row right now, if anything. A row with
    // a verb in flight is not the reader's to act on until that verb settles.
    pending: item.pending || null,
    // Whether this row IS the verb in flight rather than a card it is running
    // on. Only a placeholder has nothing to open.
    placeholder: !!item.placeholder,
    // Done destroys an entity — a branch's records, an issue's plans — and it
    // is spoken in that entity's name. A row that names none has nothing to
    // finish and no way to say it, so it is never offered Done however
    // finishable the feed calls it. Clear is the whole of such a row's menu.
    canFinish: !!item.can_finish && !!entityId,
    // Whether the work landed. It is the whole question an implemented issue's
    // fate turns on when its branch is deleted.
    merged: item.state === "merged",
    warnings: warningsOf(item),
    facts: entryFactsText(item),
    route: entryRoute(item),
    anchorMs: ms(item.anchor),
    lastActivityMs: ms(item.last_activity),
  };
}

/** Whether this feed row is inbox business at all: not over, and not an issue
 *  whose work is being done on a branch that has its own row. */
function isListed(item) {
  // A watched issue is listed because it is watched. The rules below are the
  // legacy issue's — a state machine and a branch implementing it — and a
  // tracker issue has neither; `status` here is a board column, not a state.
  //
  // Gated on the bridge that pushes it (#65): a machine below 1.9.0 sends no
  // such row, and one arriving from anywhere else is not something this client
  // can act on — Mute and Done on it would call verbs that bridge refuses.
  if (item.kind === TRACKER_ISSUE) return carriesWatching(item.deviceId);
  if (FINISHED_STATES.has(item.state)) return false;
  return !(item.kind === "issue" && item.implementation_active);
}

/** The captures the rail lists: what this client is holding because no machine
 *  could take it, and what the router has not placed yet. A capture belongs to
 *  no project until it is routed — at which point it stops being a capture and
 *  becomes the work it was routed to — so these stand above the workspaces
 *  rather than under any project's block. */
export function captureEntries(items = []) {
  return items
    .filter((item) => item.kind === "capture" && isListed(item))
    .map(toCaptureEntry)
    .sort(byAnchor);
}

/**
 * Issues the reader was handed, above the issues that merely moved (#65).
 *
 * Only among the ISSUE rows, and it keeps their places: the assigned ones take
 * the positions the issue rows already occupy, in their own activity order, and
 * every conversation row stays exactly where it was. An assignment is a reason
 * to look at one issue before another — it is not a reason to lift an issue
 * over a conversation that moved a minute ago.
 */
function pinAssignedIssues(rows) {
  const issueAt = rows.map((row, index) => (row.kind === TRACKER_ISSUE ? index : -1)).filter((index) => index >= 0);
  if (issueAt.length < 2) return rows;
  const issues = issueAt.map((index) => rows[index]);
  const ordered = [...issues.filter((row) => row.assignedToUser), ...issues.filter((row) => !row.assignedToUser)];
  if (ordered.every((row, index) => row === issues[index])) return rows;
  const out = [...rows];
  issueAt.forEach((index, which) => {
    out[index] = ordered[which];
  });
  return out;
}

/** Oldest anchor first. A row nobody can date sorts under the ones somebody
 *  can — unknown age is not evidence of being old. */
function byAnchor(left, right) {
  if (left.anchorMs === right.anchorMs) return 0;
  if (left.anchorMs === null) return 1;
  if (right.anchorMs === null) return -1;
  return left.anchorMs - right.anchorMs;
}

/**
 * The rows the inbox lists, split into the list proper and Recent:
 * `{ entries, recent }`.
 *
 * Both lists are in anchor order, oldest first. `recent` contains every cleared
 * row (`dismissed`), plus rows whose message/agent activity is at least a day
 * old while no agent is working. Unknown activity remains in the Inbox proper;
 * missing data is not evidence that a row is stale.
 */
export function inboxEntries({ items = [], nowMs = Date.now() } = {}) {
  const rows = pinAssignedIssues(
    items
      .filter(isListed)
      .map(toEntry)
      .sort(byAnchor),
  );
  const quiet = (entry) =>
    entry.dismissed || (!entry.working && entry.lastActivityMs !== null && nowMs - entry.lastActivityMs >= RECENT_AFTER_MS);
  const entries = rows.filter((entry) => !quiet(entry));
  const recent = rows.filter(quiet);
  return { entries, recent };
}

/** The entities whose local caches stay warm: the inbox's own partition is the
 *  rule. Every listed entry's entity is active; going Recent, being cleared,
 *  and finishing all mean the row stops being named here, and the cache evicts
 *  what this stops naming. One addition: the issue an active branch is
 *  implementing is not listed — the branch carries the row — but its surfaces
 *  are one click away, so its cache stays warm with the branch's. */
export function cacheableEntityIds({ items = [], nowMs = Date.now() } = {}) {
  const ids = new Set(
    inboxEntries({ items, nowMs })
      .entries.map((entry) => entry.entityId)
      .filter(Boolean),
  );
  for (const item of items) {
    if (item.kind !== "issue" || !item.implementation_active) continue;
    if (item.dismissed || FINISHED_STATES.has(item.state)) continue;
    const id = entityIdOf(item);
    if (id) ids.add(id);
  }
  return [...ids];
}

/** Which row each kind of route stands on, as a test one row answers. A
 *  capture names where it was routed, but it is not that work item: a work
 *  route stands on the branch or the issue itself, and a capture route stands
 *  only on the capture. */
const STANDS_ON = {
  // A branch is named by its machine as well as its project: two devices each
  // hold a `proj-1` with a `main` in it, and those are two rows.
  branch: (route) => (entry) =>
    entry.kind !== "capture" &&
    entry.deviceId === route.deviceId &&
    entry.projectId === route.projectId &&
    entry.branch === route.branch,
  // An issue id is a uuid, so it names one row wherever it is.
  issue: (route) => (entry) => entry.kind !== "capture" && entry.issueId === route.id,
  // A workspace is named by its machine too: a workspace id is one bridge's.
  workspace: (route) => (entry) =>
    entry.kind === "workspace" && entry.deviceId === route.deviceId && entry.workspaceId === route.workspaceId,
  capture: (route) => (entry) => entry.kind === "capture" && entry.captureId === route.id,
  // A project's page stands on the conversation that project holds — the row
  // the board keeps for the owner `project.list` names in `entity_id`. Without
  // this the project page names no entity at all, which is a page the sync
  // layer neither leads its pass with nor watches in realtime: the reader
  // watching their project agent work would be the one reader on a page that
  // hears nothing.
  project: (route, view) => {
    const conversation = projectConversationId(route, view);
    return (entry) => Boolean(conversation) && entry.entityId === conversation;
  },
};

/** The conversation a project holds on the machine the route names, as
 *  `project.list` answers it. Null for a project nobody has talked to yet, and
 *  wherever the snapshot has not caught up with the route. */
function projectConversationId(route, view) {
  const project = (view?.projects || []).find(
    (candidate) => candidate.deviceId === route.deviceId && (candidate.id || candidate.project_id) === route.projectId,
  );
  return project?.entity_id || project?.run_id || null;
}

/** The entry the current route is standing on, so the list can mark it. Takes
 *  every row on screen — Recent included, since an open one is on screen. A
 *  route that names no work item stands on nothing. */
export function activeEntryKey(route, entries) {
  const standsOn = route && STANDS_ON[route.name];
  if (!standsOn) return null;
  const match = entries.find(standsOn(route, null));
  return match ? match.key : null;
}

/**
 * The entity the route is standing on, in one device's snapshot — what the
 * sync layer's active subscription names.
 *
 * Every kind of route is answered the same way the list marks its own active
 * row: the snapshot's rows and its workspace rows, asked the route's question.
 * A workspace's entity is the conversation it holds, which is why the
 * workspace rows are folded in rather than the workspace id taken as the
 * answer: they are not the same id, and only the entity one addresses a cache.
 *
 * Null when the route names no work item, when the snapshot has not caught up
 * with it, or when the row it names is on another machine.
 */
export function routedEntityId(route, view = {}) {
  return routedEntry(route, view)?.entityId || null;
}

/** The whole entry the route is standing on — what it is called as well as
 *  what it is addressed by. A workspace's name lives here and nowhere else in
 *  the cache: its row is the conversation's, and the conversation is not what
 *  the reader named. */
export function routedEntry(route, view = {}) {
  const standsOn = route && STANDS_ON[route.name];
  if (!standsOn) return null;
  const items = view.items || [];
  const rows = [...items.filter(isListed).map(toEntry), ...workspaceEntries(view.workspaces, view.projects, items)];
  return rows.find(standsOn(route, view)) || null;
}

/** The row's action cluster: one quiet ⋯, and behind it the row's menu — Done
 *  where there is something to finish, then Clear, then Mute.
 *
 *  Done leads because it is the verb the row is for, and it sits behind the
 *  menu rather than on the row because it destroys (a branch's checkout and
 *  records, an issue to the archive) — one step and a confirmation is the
 *  right distance for that, and a button laid over the row's own words was
 *  not. It is offered only where there is something to finish.
 *
 *  Every row has Clear, because it is the one verb that costs nothing: the
 *  row moves to Recent until the next user or agent message removes its clear
 *  marker. File and commit changes do not revive it.
 *
 *  Mute keeps the row and takes its voice, so it needs an entity with a voice
 *  to take. A row with neither Done nor Mute still has its menu — Clear is
 *  what it is for. */
function menuHtml(entry, open) {
  if (entry.kind === "workspace") return "";
  // A row the daemon is in the middle of making or removing is not the
  // reader's to act on: every verb here would race the one already running,
  // and the daemon refuses a second claim on the same thing anyway.
  if (entry.pending) return "";
  const items = [];
  if (entry.canFinish) {
    items.push(
      `<div class="mi" data-done="${esc(entry.key)}"><span class="mt">Done</span><span class="md">${
        entry.kind === "issue" ? "File the issue away" : "Delete the branch and its checkout"
      }</span></div>`,
    );
  }
  items.push(
    `<div class="mi" data-dismiss="${esc(entry.key)}"><span class="mt">Clear from inbox</span><span class="md">Moves it to Recent until a new message</span></div>`,
  );
  if (entry.entityId) {
    items.push(
      `<div class="mi" data-mute="${esc(entry.key)}"><span class="mt">${entry.muted ? "Unmute" : "Mute"}</span><span class="md">${
        entry.muted ? "Let this entry ask again" : "Keep this entry, stop it asking"
      }</span></div>`,
    );
  }
  // The menu is in the markup only while it is open: the DOM patcher leaves a
  // split menu's `hidden` alone (a poll must not shut what the reader opened),
  // so a menu that closes has to leave rather than hide.
  const menu = open ? `<div class="splitmenu inbox-menu">${items.join("")}</div>` : "";
  return `<button class="iconbtn inbox-more" data-menu="${esc(entry.key)}" title="More" aria-label="More actions for ${esc(entry.name)}">⋯</button>
    ${menu}`;
}

/** The machine something is on, said dim after its name. Minted here because a
 *  row wears it (projectTagHtml) and so does a project block's head
 *  (core/inboxProjects.js deviceTagHtml): it is one mark, in one place.
 *
 *  It wears a class of its own beside `dim` so it can be sized as well as
 *  greyed: which machine a thing is on is a secondary fact about it — the
 *  answer to "which of the two `relaydb`s is this", read once — and at the
 *  row's own size it competed with the name it qualifies. */
export const dimDeviceHtml = (deviceName) => (deviceName ? ` <span class="dim inbox-device">${esc(deviceName)}</span>` : "");

/** The names in a list that more than one machine holds. A name the account
 *  uses once says which thing it is; one two machines both use does not, and
 *  whatever wears it says its machine after it. The rail's project blocks and
 *  the archive's rows both ask here, so the two pages agree about when a name
 *  needs its machine said. */
export function clashingNames(rows, nameOf) {
  const devicesByName = new Map();
  for (const row of rows) {
    const name = nameOf(row);
    if (!devicesByName.has(name)) devicesByName.set(name, new Set());
    devicesByName.get(name).add(row.deviceId);
  }
  return new Set([...devicesByName].filter(([, devices]) => devices.size > 1).map(([name]) => name));
}

/** What a row says it is in: its project, and — only where two machines use
 *  that name — the machine it is on, after it. Whether the name needs its
 *  machine said is decided once for the whole list and carried on the entry, so
 *  this prints what it is given. A row under its project's own block has
 *  already been told which project it is in, and says nothing. */
function projectTagHtml(entry, ui) {
  if (ui.showProject === false) return "";
  return `<span class="inbox-tag">${esc(entry.project || "unknown project")}${dimDeviceHtml(entry.deviceName)}</span>`;
}

/** The project a row's title names, with the machine after it where two
 *  machines share the name. */
const titleProject = (entry) => (entry.deviceName ? `${entry.project} (${entry.deviceName})` : entry.project);

/** How each blocker the bridge names reads to a person. The bridge decides
 *  whether Done is available; this is only how the row says why it is not. */
const FINISH_BLOCKER_HINTS = {
  unpushed: "Push to remote first",
  dirty: "Commit or discard changes first",
  agent_working: "Agent is working",
  plain_directory: "Remove the folder that is not a repository first",
  unknown: "Still reading this workspace",
};

/** What stands between this workspace and Done, in one line. Empty when Done
 *  is available, or when the row is not a workspace's. */
export const finishBlockerHint = (blockers = []) =>
  blockers.map((blocker) => FINISH_BLOCKER_HINTS[blocker] || FINISH_BLOCKER_HINTS.unknown).join(" · ");

/** A workspace is put away from its own row. Done removes it — the files, the
 * record and the conversation — so it is offered only once the bridge says the
 * work is somewhere else, and until then the button is there and shut, wearing
 * the reason. A workspace that is not ready is not a workspace to finish and
 * shows nothing. Neither is an adopted checkout: that folder is not Build's
 * to remove, and the bridge refuses Done on one. Asks the wiring whether this
 * one is already being finished, so the row painter does not have to. */
function workspaceDoneHtml(entry, ui) {
  if (entry.kind !== "workspace" || !entry.ready || entry.adopted) return "";
  const pending = ui.finishingWorkspaces?.has(entry.key);
  const hint = finishBlockerHint(entry.finishBlockers);
  const shut = pending || !entry.canFinish;
  return `<button class="btn mini inbox-workspace-done" type="button" data-workspace-done="${esc(entry.key)}" aria-label="Finish workspace ${esc(entry.name)}" title="${esc(hint)}"${shut ? " disabled" : ""}>${pending ? "Done…" : "Done"}</button>`;
}

/** Everything the two lines leave out, on the row itself: what the work is for,
 *  which project it lives in, and why it is asking for you. */
function rowTooltip(entry) {
  return [entry.title, titleProject(entry), entry.reason].filter(Boolean).join(" — ");
}

/** One inbox row, in two lines: the state dot and what this is, with the unread
 *  count at the right edge; then what it weighs. `ui`: { activeKey,
 *  openMenuKey, showProject, quiet }. A quiet row — one in Recent — is one
 *  line instead (quietRowHtml). */
export function inboxRowHtml(entry, ui = {}) {
  if (entry.kind === "capture") return captureRowHtml(entry, ui);
  if (ui.quiet) return quietRowHtml(entry, ui);
  const unread = entry.unreadCount > 0 ? `<span class="badge inbox-unread">${entry.unreadCount}</span>` : "";
  // One list across every project: which project a row belongs to is the one
  // fact it cannot go without, so it leads line one — two rows both named
  // "main" must never read as the same thing. A row painted under its project's
  // own block (`showProject: false`) has already been told.
  const projectTag = projectTagHtml(entry, ui);
  const classes = [
    "srow",
    "inbox-entry",
    entry.key === ui.activeKey ? "active" : "",
    entry.muted ? "inbox-muted" : "",
    entry.route ? "" : "inbox-unroutable",
  ]
    .filter(Boolean)
    .join(" ");
  return `<div class="${classes}" data-key="${esc(entry.key)}"${
    entry.entityId ? ` data-entity="${esc(entry.entityId)}"` : ""
  } title="${esc(rowTooltip(entry))}">
    <span class="sdot sdot-${entry.state}" title="${entry.state}"></span>
    <div class="inbox-body">
      <div class="inbox-line inbox-name">${projectTag}<span class="stitle">${esc(entry.name)}</span>${unread}</div>
      <div class="inbox-facts">${esc(entry.facts || GETTING_STARTED)}</div>
      <span class="warn" data-done-error hidden></span>
    </div>
    <div class="inbox-actions">${workspaceDoneHtml(entry, ui)}${menuHtml(entry, ui.openMenuKey === entry.key)}</div>
  </div>`;
}

/** A quiet row, which is what Recent holds: one line — what this is, with
 *  what it weighs floating over the line's right edge — and no state dot. A
 *  row that has said nothing for a day has no state worth a glance, and what
 *  it weighs is the one fact left worth reading, so it takes the right edge
 *  the way the unread count does on a live row. Same key, same verbs, same
 *  element shape, so a row going quiet keeps its element. */
function quietRowHtml(entry, ui) {
  const unread = entry.unreadCount > 0 ? `<span class="badge inbox-unread">${entry.unreadCount}</span>` : "";
  const projectTag = projectTagHtml(entry, ui);
  const classes = [
    "srow",
    "inbox-entry",
    "inbox-quiet",
    entry.key === ui.activeKey ? "active" : "",
    entry.muted ? "inbox-muted" : "",
    entry.route ? "" : "inbox-unroutable",
  ]
    .filter(Boolean)
    .join(" ");
  return `<div class="${classes}" data-key="${esc(entry.key)}"${
    entry.entityId ? ` data-entity="${esc(entry.entityId)}"` : ""
  } title="${esc(rowTooltip(entry))}">
    <div class="inbox-body">
      <div class="inbox-line inbox-name">${projectTag}<span class="stitle">${esc(entry.name)}</span>${unread}</div>
      <span class="warn" data-done-error hidden></span>
    </div>
    ${entry.facts ? `<span class="inbox-facts inbox-facts-float">${esc(entry.facts)}</span>` : ""}
    <div class="inbox-actions">${menuHtml(entry, ui.openMenuKey === entry.key)}</div>
  </div>`;
}

/** What the rail says when it is holding nothing at all. */
export function inboxEmptyHtml() {
  return '<div class="inbox-clear dim">Nothing needs you. Work you start shows up here.</div>';
}

/** Whether Recent is open: only if the user opened it. It never opens by
 *  itself, however thin the list above it. */
export function recentIsOpen(recentOpen) {
  return recentOpen === true;
}

/** Recent's disclosure: the one control at the end of the list, counting what is
 *  behind it. The quiet rows themselves are ordinary rows, painted under it.
 *  `scope` names whose Recent this is — the inbox's, or one project block's —
 *  so a press opens the right one. */
export function recentToggleHtml(recent, open, scope = "inbox") {
  return `<button class="inbox-recent-toggle" type="button" data-recent-toggle="${esc(scope)}" aria-expanded="${open ? "true" : "false"}">
      <span class="inbox-recent-caret" aria-hidden="true">${open ? ICON_CHEVRON_DOWN : ICON_CHEVRON_RIGHT}</span><span>Recent</span><span class="inbox-recent-count">${recent.length}</span>
    </button>`;
}

// ---- capture rows ------------------------------------------------------------
//
// A capture is on the inbox while it is still unfinished business — being
// routed, failed, or holding a question. The row is where routing is made
// visible and reversible: it says where the capture went, says when the router
// is waiting on an answer, and offers the two verbs that move it.
//
// The answer itself is not taken here. What to do with a capture is a decision
// — the router's choices, a destination named by hand, words, or abandoning it
// — and the row opens the page that holds all of them (views/captureDecision).

/** The branch field the picker discloses: which branch in this project the work
 *  goes on, offered from the branches the project already has. Naming one is
 *  optional — with none the branch is named after what was said, the same rule
 *  the router dispatches by. */
function rerouteBranchHtml(projectId, branches) {
  const options = (branches || []).map((branch) => `<option value="${esc(branch)}"></option>`).join("");
  return `<div class="reroute-branch">
    <input type="text" class="path" data-reroute-branch="${esc(projectId)}" list="reroute-branches"
      placeholder="a new branch, named after what you said" aria-label="Branch" autocomplete="off" />
    <datalist id="reroute-branches">${options}</datalist>
    <button class="btn mini primary" type="button" data-reroute-project="${esc(projectId)}" data-reroute-kind="branch">Dispatch</button>
  </div>`;
}

/** The destination picker behind the reroute chip: every project, and the two
 *  branch destination a capture can become in it.
 *
 *  `ui`: { projects, rerouteBranchProject, rerouteBranches }. */
function rerouteMenuHtml(entry, ui = {}) {
  const rows = (ui.projects || [])
    .map(
      (project) => `<div class="reroute-project"><span class="mt">${esc(project.name || project.id)}</span>
        <span class="reroute-kinds"><button class="btn mini${project.id === ui.rerouteBranchProject ? " primary" : ""}" type="button"
            data-reroute-branch-open="${esc(project.id)}">Branch</button>
        </span></div>${project.id === ui.rerouteBranchProject ? rerouteBranchHtml(project.id, ui.rerouteBranches) : ""}`,
    )
    .join("");
  return `<div class="splitmenu reroute-menu">
    <div class="tb-group">Send it somewhere else</div>
    ${rows || '<div class="tb-none dim">No projects on this device.</div>'}
  </div>`;
}

/** One capture row. `ui`: { activeKey, rerouteKey, projects, rerouteBranchProject,
 *  rerouteBranches }. */
// eslint-disable-next-line complexity -- ratchet: captureRowHtml is at 14, cap 10 — reduce it, then drop this line
export function captureRowHtml(entry, ui = {}) {
  const working = entry.captureState === "queued" || entry.captureState === "unrouted" || entry.captureState === "routing";
  // A question is the router at rest, waiting on the user: a spinner there
  // says something is happening when nothing is.
  const spinning = working && !entry.question;
  const status = captureStatusText(entry);
  const actions = [];
  if (entry.captureState === "failed") {
    actions.push(`<button class="btn mini" type="button" data-capture-retry="${esc(entry.captureId)}">Retry</button>`);
  }
  if (entry.captureState === "routed") {
    actions.push(
      `<button class="chip capture-chip" type="button" data-capture-reroute="${esc(entry.captureId)}">Reroute</button>`,
    );
  }
  const classes = ["srow", "inbox-entry", "capture-entry", entry.key === ui.activeKey ? "active" : "", entry.route ? "" : "inbox-unroutable"]
    .filter(Boolean)
    .join(" ");
  return `<div class="${classes}" data-key="${esc(entry.key)}" data-capture="${esc(entry.captureId)}" title="${esc(entry.text || entry.name)}">
    <span class="sdot sdot-${entry.state}" title="${entry.state}"></span>
    <div class="inbox-body">
      <div class="inbox-line"><span class="stitle">${esc(entry.name)}</span></div>
      <div class="inbox-line capture-status${spinning ? " dim" : ""}">${
        spinning ? '<span class="capture-spinner" aria-hidden="true"></span>' : ""
      }<span>${esc(status)}</span></div>
      ${entry.question ? `<div class="inbox-reason">${esc(entry.question)}</div>` : ""}
      <span class="warn" data-capture-error hidden></span>
    </div>
    <div class="inbox-actions">${actions.join("")}${
      ui.rerouteKey === entry.key ? rerouteMenuHtml(entry, ui) : ""
    }</div>
  </div>`;
}

// ---- the Done confirmations --------------------------------------------------
//
// Done destroys. On a branch it deletes the branch, its checkout and its
// records; on an issue it files the issue away. Neither is ever refused — the
// bridge sends what the destruction would cost (`finish.warnings`) and the
// confirmation is where the user reads it, above the outline of exactly what
// will happen (the core/confirm.js contract).

/** Done on a branch. `entry` is an inbox entry, or the same four facts off any
 *  other surface standing in the branch: { branch, issueId, merged, warnings }.
 *
 *  The issue's fate follows whether the work landed: a merge files it away with
 *  its branch, and any other ending hands it back to the inbox with an event
 *  naming the branch it lost. */
export function branchDoneConfirm(entry) {
  const name = entry.branch || "this checkout";
  const actions = [`Delete branch ${name}`, "Remove its checkout", "Take its conversation off the inbox"];
  if (entry.issueId) {
    actions.push(
      entry.merged
        ? "Archive the issue it implements, with its stage plans"
        : `Return the issue it implements to the inbox, noting that ${name} was deleted`,
    );
  }
  return {
    title: `Done with ${name}?`,
    intro: "Done deletes the branch. This cannot be undone.",
    warnings: entry.warnings || [],
    actions,
    confirmLabel: "Delete",
    danger: true,
  };
}

/** Done on an issue: it goes to the archive, where it can be read again. */
export function issueDoneConfirm(entry) {
  return {
    title: "Done with this issue?",
    intro: "The issue is filed away, and can be read again from the archive.",
    warnings: entry.warnings || [],
    actions: ["Move the issue and its stage plans to the archive", "Take it off the inbox"],
    confirmLabel: "Done",
    danger: false,
  };
}
