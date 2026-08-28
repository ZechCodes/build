// The inbox's pure model: which work items the rail lists, in what order, what
// each row says, and what Done promises before it destroys anything.
//
// One list across every project — the inbox is the user's, not a project's, so
// there are no per-project blocks and no worktree fold. The rows are issues,
// branches and captures. An issue a branch is implementing right now is not a
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
// the end of the list: still there, just not what today is about. With almost
// nothing above it, it opens by itself.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { entityIdOf } from "./entityId.js";

const DAY_MS = 24 * 3600 * 1000;

/** How long a row can say nothing before it belongs to Recent rather than to
 *  the list proper. */
export const RECENT_AFTER_MS = DAY_MS;

/** Under this many rows in the list proper, the inbox has nothing worth hiding
 *  behind a disclosure, so Recent opens itself. */
export const RECENT_AUTO_OPEN_BELOW = 5;

/** Line two, for a row that has not done anything measurable yet. */
export const GETTING_STARTED = "Getting started";

/** The states that mean the work is over. Nothing marked done is ever a row. */
const FINISHED_STATES = new Set(["merged", "abandoned", "archived"]);

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

/** Where an entry opens. A branch is (project, branch name); an issue is its
 *  own surface. A checkout with no branch is nameable by no URL, so it opens
 *  nowhere until it is on one.
 *
 *  A capture opens wherever it was routed. Until it is routed it opens its own
 *  decision page — what to do with it is a question, and a question deserves a
 *  surface. One this client is still holding has no record to decide about, so
 *  it opens nowhere. */
export function entryRoute(item) {
  if (item.kind === "capture") {
    if (item.routing) {
      return entryRoute({ ...item, kind: item.routing.kind === "issue" ? "issue" : "branch" });
    }
    return item.state === "queued" ? null : { name: "capture", id: item.capture_id };
  }
  if (item.kind === "issue") {
    return item.issue_id ? { name: "issue", projectId: item.project_id, id: item.issue_id } : null;
  }
  return item.branch ? { name: "branch", projectId: item.project_id, branch: item.branch, tab: "changes" } : null;
}

const ms = (iso) => {
  const parsed = Date.parse(iso || "");
  return Number.isFinite(parsed) ? parsed : null;
};

/** What names a row in the DOM — the one name every row has. Usually the
 *  entity id; a project's primary checkout is the repository, which takes no
 *  attention and has no entity, and it is still a row you open and clear.
 *  Exported so the wiring can match a feed row to the keys it is holding. */
export const entryKeyOf = (item) => {
  if (item.kind === "capture") return `capture:${item.capture_id}`;
  return entityIdOf(item) || (item.kind === "issue" ? `issue:${item.project_id}` : `branch:${item.project_id}:${item.branch}`);
};

/**
 * How `entity.dismiss` names the row being cleared. An entity by its id — the
 * bridge draws the line at the end of its conversation. A row with no entity
 * by what it IS: the project's own checkout (`primary: true`), or a branch in
 * the project — the bridge clears those at the commit they sit on, and a new
 * commit brings them back. Null for a row nothing can name, which is not a
 * row the feed produces.
 */
export function dismissParamsOf(entry) {
  if (entry.entityId) return { entity_id: entry.entityId };
  if (entry.primary && entry.projectId) return { project_id: entry.projectId, primary: true };
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
export function entryFactsText(item) {
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
  return CAPTURE_STATUS[entry.captureState] || "";
}

/** One capture row, as the inbox reads it. A capture holds no conversation and
 *  no agents, so what it needs is read off the record: the router's unanswered
 *  question is the reason, and a route that gave up is a retry. */
function toCaptureEntry(item) {
  const question = item.question && !item.question.answer ? item.question.text : "";
  const routing = item.routing || null;
  return {
    key: entryKeyOf(item),
    entityId: null,
    kind: "capture",
    captureId: item.capture_id,
    captureState: item.state,
    projectId: item.project_id || "",
    project: item.project || "",
    branch: item.branch || null,
    issueId: item.issue_id || null,
    name: item.title || "(nothing said)",
    title: item.title || "(nothing said)",
    text: item.text || "",
    state: item.unread ? "unread" : item.state === "routed" ? "inactive" : "working",
    reason: question || (item.unread_reason === "routing_failed" ? "Routing failed" : ""),
    question,
    routedTo: routing ? { project: item.project || item.project_id, kind: routing.kind } : null,
    unreadCount: item.unread_count || 0,
    muted: false,
    // A capture leaves the inbox by being routed, so it is never cleared.
    dismissed: false,
    canFinish: false,
    merged: false,
    warnings: [],
    primary: false,
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

/** One board.list row, as the inbox reads it. */
function toEntry(item) {
  if (item.kind === "capture") return toCaptureEntry(item);
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
    projectId: item.project_id,
    project: item.project || item.project_id || "",
    branch: item.branch || null,
    issueId: item.issue_id || null,
    name,
    title: item.title || item.branch || "(untitled)",
    state,
    reason: state === "unread" ? unreadReasonText(item.unread_reason, item.kind) : "",
    unreadCount: item.unread_count || 0,
    muted: !!item.muted,
    // The bridge's own word for "the user cleared this and nothing new has
    // happened since". It is not mute and not Done: the row is simply absent
    // until an attention event later than the dismissal brings it back.
    dismissed: !!item.dismissed,
    // Done destroys an entity — a branch's records, an issue's plans — and it
    // is spoken in that entity's name. A row that names none has nothing to
    // finish and no way to say it, so it is never offered Done however
    // finishable the feed calls it. Clear is the whole of such a row's menu.
    canFinish: !!item.can_finish && !!entityId,
    // Whether the work landed. It is the whole question an implemented issue's
    // fate turns on when its branch is deleted.
    merged: item.state === "merged",
    warnings: warningsOf(item),
    primary: !!item.primary,
    facts: entryFactsText(item),
    route: entryRoute(item),
    anchorMs: ms(item.anchor),
    lastActivityMs: ms(item.last_activity),
  };
}

/** Whether this feed row is inbox business at all: not over, and not an issue
 *  whose work is being done on a branch that has its own row. */
function isListed(item) {
  if (FINISHED_STATES.has(item.state)) return false;
  return !(item.kind === "issue" && item.implementation_active);
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
 * `{ entries, recent, autoOpen }`.
 *
 * Both lists are in anchor order, oldest first. `recent` is everything whose
 * last activity — a file changing, a message, an agent painting — is over a day
 * old, and `autoOpen` is whether the section should be open with nobody having
 * said either way.
 *
 * A row the user cleared (`dismissed`) is in neither list — that is what
 * clearing means, and it is the whole difference from Recent, where a row that
 * has only gone quiet still sits.
 *
 * `hiddenKeys` holds the row keys a verb from this client just removed — Done,
 * or a clear the daemon has not confirmed yet — so a stale feed paint cannot
 * put the row back on screen. Keys, not entity ids: every row has a key, and
 * the rows no entity stands behind can be cleared like any other.
 */
export function inboxEntries({ items = [], nowMs = Date.now(), hiddenKeys = new Set() } = {}) {
  const rows = items
    .filter(isListed)
    .map(toEntry)
    .filter((entry) => !entry.dismissed)
    .filter((entry) => !hiddenKeys.has(entry.key))
    .sort(byAnchor);
  const quiet = (entry) => entry.lastActivityMs !== null && nowMs - entry.lastActivityMs > RECENT_AFTER_MS;
  const entries = rows.filter((entry) => !quiet(entry));
  const recent = rows.filter(quiet);
  return { entries, recent, autoOpen: entries.length < RECENT_AUTO_OPEN_BELOW };
}

/** The entry the current route is standing on, so the list can mark it. Takes
 *  every row on screen — Recent included, since an open one is on screen. */
export function activeEntryKey(route, entries) {
  if (!route) return null;
  if (route.name === "capture") {
    const capture = entries.find((entry) => entry.kind === "capture" && entry.captureId === route.id);
    return capture ? capture.key : null;
  }
  // A capture names where it was routed, but it is not that work item — the row
  // the route stands on is the branch or the issue itself.
  const work = entries.filter((entry) => entry.kind !== "capture");
  const match =
    route.name === "branch"
      ? work.find((entry) => entry.projectId === route.projectId && entry.branch === route.branch)
      : route.name === "issue"
        ? work.find((entry) => entry.issueId === route.id)
        : null;
  return match ? match.key : null;
}

/** The row's action cluster: Done, where there is something to finish, with
 *  the row's menu — clear, mute, and Done — one caret away, in the same split
 *  button the git verbs wear. Rows with no Done keep the menu behind a ⋯.
 *
 *  Clear leads, and every row has it, because it is the one verb that costs
 *  nothing: the row leaves and comes back the moment something new needs the
 *  user. On a row with a conversation "something new" is the next attention
 *  event; on one with none (a bare checkout, the primary) the bridge clears it
 *  at the commit it sits on, and a new commit brings it back.
 *
 *  Mute keeps the row and takes its voice, so it needs an entity with a voice
 *  to take; Done destroys, and is offered only where there is something to
 *  finish. A row with neither still has its menu — Clear is what it is for. */
function menuHtml(entry, open) {
  const items = [
    `<div class="mi" data-dismiss="${esc(entry.key)}"><span class="mt">Clear from inbox</span><span class="md">Hides it until something new needs you</span></div>`,
  ];
  if (entry.entityId) {
    items.push(
      `<div class="mi" data-mute="${esc(entry.key)}"><span class="mt">${entry.muted ? "Unmute" : "Mute"}</span><span class="md">${
        entry.muted ? "Let this entry ask again" : "Keep this entry, stop it asking"
      }</span></div>`,
    );
  }
  if (entry.canFinish) {
    items.push(
      `<div class="mi" data-done="${esc(entry.key)}"><span class="mt">Done</span><span class="md">${
        entry.kind === "issue" ? "File the issue away" : "Delete the branch and its checkout"
      }</span></div>`,
    );
  }
  const menu = `<div class="splitmenu inbox-menu"${open ? "" : " hidden"}>${items.join("")}</div>`;
  // A finishable row wears the git verbs' split button: Done with the menu's
  // caret joined to its side. A row with no Done keeps the ⋯ on its own.
  if (entry.canFinish)
    return `<div class="splitbtn"><button class="btn mini" data-done="${esc(entry.key)}" type="button" aria-label="Done with ${esc(entry.name)}">Done</button><button class="btn mini caret" data-menu="${esc(entry.key)}" title="More" aria-label="More actions for ${esc(entry.name)}">▾</button>${menu}</div>`;
  return `<button class="iconbtn inbox-more" data-menu="${esc(entry.key)}" title="More" aria-label="More actions for ${esc(entry.name)}">⋯</button>
    ${menu}`;
}

/** Everything the two lines leave out, on the row itself: what the work is for,
 *  which project it lives in, and why it is asking for you. */
function rowTooltip(entry) {
  return [entry.title, entry.project, entry.reason].filter(Boolean).join(" — ");
}

/** One inbox row, in two lines: the state dot and what this is, with the unread
 *  count at the right edge; then what it weighs. `ui`: { activeKey,
 *  openMenuKey }. */
export function inboxRowHtml(entry, ui = {}) {
  if (entry.kind === "capture") return captureRowHtml(entry, ui);
  const unread = entry.unreadCount > 0 ? `<span class="badge inbox-unread">${entry.unreadCount}</span>` : "";
  // One list across every project: which project a row belongs to is the one
  // fact it cannot go without, so it leads line one — two rows both named
  // "main" must never read as the same thing.
  const projectTag = `<span class="inbox-tag">${esc(entry.project || "unknown project")}</span>`;
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
    <div class="inbox-actions">${menuHtml(entry, ui.openMenuKey === entry.key)}</div>
  </div>`;
}

/** What the rail says when it is holding nothing at all. */
export function inboxEmptyHtml() {
  return '<div class="inbox-clear dim">Nothing needs you. Work you start shows up here.</div>';
}

/** Whether Recent is open: what the user said if they have said anything, else
 *  what the partition decided for itself. */
export function recentIsOpen({ autoOpen = false } = {}, recentOpen) {
  return recentOpen === undefined || recentOpen === null ? !!autoOpen : !!recentOpen;
}

/** Recent's disclosure: the one control at the end of the list, counting what is
 *  behind it. The quiet rows themselves are ordinary rows, painted under it. */
export function recentToggleHtml(recent, open) {
  return `<button class="inbox-recent-toggle" type="button" data-recent-toggle aria-expanded="${open ? "true" : "false"}">
      <span class="inbox-recent-caret" aria-hidden="true">${open ? "▾" : "▸"}</span><span>Recent</span><span class="inbox-recent-count">${recent.length}</span>
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
 *  things a capture can become in it. An issue takes one tap — there is nothing
 *  else to say about it; a branch discloses the field that names it.
 *
 *  `ui`: { projects, rerouteBranchProject, rerouteBranches }. */
function rerouteMenuHtml(entry, ui = {}) {
  const rows = (ui.projects || [])
    .map(
      (project) => `<div class="reroute-project"><span class="mt">${esc(project.name || project.id)}</span>
        <span class="reroute-kinds">
          <button class="btn mini" type="button" data-reroute-project="${esc(project.id)}" data-reroute-kind="issue">Issue</button>
          <button class="btn mini${project.id === ui.rerouteBranchProject ? " primary" : ""}" type="button"
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
