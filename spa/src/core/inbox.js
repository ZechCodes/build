// The inbox's pure model: which work items the rail lists, in what order, what
// each row says, and the copy behind "why this needs you".
//
// One list across every project — the inbox is the user's, not a project's, so
// there are no per-project blocks and no worktree fold. Entries are branches and
// issues (board.list's items[]) in three states: unread (an attention event is
// waiting, and the row says which), working (an agent has the message and has
// not reported), inactive. Status events move the metadata line and nothing
// else.
//
// No DOM, no app imports — the wiring (core/inboxView.js) renders these.

import { esc } from "./text.js";
import { entityIdOf } from "./entityId.js";
import { itemProgressFacts } from "./progressFacts.js";

const DAY_MS = 24 * 3600 * 1000;

/** How many rows the list tops itself up to when little is alive. A floor, not
 *  a ceiling: everything unread, working, finishable or recent is shown however
 *  many that is. */
export const INBOX_MINIMUM = 8;

/** Three states, one glance. Unread wins over working: an entry that asked you
 *  something while it kept going is still asking. */
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

const STATE_RANK = { unread: 0, working: 1, inactive: 2 };

/** What names a row in the DOM. Usually the entity id — but a project's primary
 *  checkout is the repository, which takes no attention and has no entity, and
 *  it is still a row you open. */
const keyOf = (item) => {
  if (item.kind === "capture") return `capture:${item.capture_id}`;
  return entityIdOf(item) || (item.kind === "issue" ? `issue:${item.project_id}` : `branch:${item.project_id}:${item.branch}`);
};

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
    key: keyOf(item),
    entityId: null,
    kind: "capture",
    captureId: item.capture_id,
    captureState: item.state,
    projectId: item.project_id || "",
    project: item.project || "",
    branch: item.branch || null,
    issueId: item.issue_id || null,
    title: item.title || "(nothing said)",
    text: item.text || "",
    state: item.unread ? "unread" : item.state === "routed" ? "inactive" : "working",
    reason: question || (item.unread_reason === "routing_failed" ? "Routing failed" : ""),
    question,
    routedTo: routing ? { project: item.project || item.project_id, kind: routing.kind } : null,
    unreadCount: item.unread_count || 0,
    muted: false,
    canFinish: false,
    primary: false,
    facts: "",
    route: entryRoute(item),
    resumeMs: ms(item.resume_at || item.created_at),
  };
}

/** One board.list row, as the inbox reads it. */
function toEntry(item, nowMs) {
  if (item.kind === "capture") return toCaptureEntry(item);
  const state = entryState(item);
  return {
    key: keyOf(item),
    entityId: entityIdOf(item),
    kind: item.kind,
    projectId: item.project_id,
    project: item.project || "",
    branch: item.branch || null,
    issueId: item.issue_id || null,
    title: item.title || item.branch || "(untitled)",
    state,
    reason: state === "unread" ? unreadReasonText(item.unread_reason, item.kind) : "",
    unreadCount: item.unread_count || 0,
    muted: !!item.muted,
    canFinish: !!item.can_finish,
    primary: !!item.primary,
    facts: itemProgressFacts(item, nowMs),
    route: entryRoute(item),
    resumeMs: ms(item.resume_at),
  };
}

/**
 * The rows the inbox lists, in display order.
 *
 * Shown outright: everything unread, everything an agent is working in,
 * everything ready to be finished, every project's primary checkout (the
 * project's own row, and the only always-present handle on it), and everything
 * picked up within the day. If that is thin, top it up with what was picked up
 * most recently — coming back after a weekend, that is what you came back for.
 *
 * `dismissed` holds the entity ids the user just said Done to, so a stale feed
 * paint cannot put an archived row back on screen.
 */
export function inboxEntries({ items = [], nowMs = Date.now(), minimum = INBOX_MINIMUM, dismissed = new Set() } = {}) {
  const entries = items
    .map((item) => toEntry(item, nowMs))
    .filter((entry) => entry.entityId === null || !dismissed.has(entry.entityId));

  const chosen = new Set();
  for (const entry of entries) {
    const recent = entry.resumeMs !== null && nowMs - entry.resumeMs <= DAY_MS;
    if (entry.state !== "inactive" || entry.canFinish || entry.primary || recent) chosen.add(entry);
  }
  if (chosen.size < minimum) {
    entries
      .filter((entry) => !chosen.has(entry))
      .sort((a, b) => (b.resumeMs ?? 0) - (a.resumeMs ?? 0))
      .slice(0, minimum - chosen.size)
      .forEach((entry) => chosen.add(entry));
  }

  // Unread first, then what is moving, then the quiet — and within each, what
  // was picked up most recently. Ties break by title for a stable paint.
  return [...chosen].sort(
    (a, b) =>
      STATE_RANK[a.state] - STATE_RANK[b.state] ||
      (b.resumeMs ?? 0) - (a.resumeMs ?? 0) ||
      a.title.localeCompare(b.title),
  );
}

/** The entry the current route is standing on, so the list can mark it. */
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

/** The row's own menu: mute, and the archive-equivalent Done, one step behind
 *  the row itself. Reuses the split button's menu markup. A row with no entity
 *  behind it (a project's primary checkout) has neither verb, so it has no
 *  menu — an empty ⋯ is a lie about what is one step behind it. */
function menuHtml(entry, open) {
  if (!entry.entityId) return "";
  const items = [
    `<div class="mi" data-mute="${esc(entry.key)}"><span class="mt">${entry.muted ? "Unmute" : "Mute"}</span><span class="md">${
      entry.muted ? "Let this entry ask again" : "Keep this entry, stop it asking"
    }</span></div>`,
  ];
  if (entry.canFinish) {
    items.push(
      `<div class="mi" data-done="${esc(entry.key)}"><span class="mt">Done</span><span class="md">Archive it out of the inbox</span></div>`,
    );
  }
  return `<button class="iconbtn inbox-more" data-menu="${esc(entry.key)}" title="More" aria-label="More actions for ${esc(entry.title)}">⋯</button>
    <div class="splitmenu inbox-menu"${open ? "" : " hidden"}>${items.join("")}</div>`;
}

/** One inbox row: the state dot, what the work is, where it lives, why it needs
 *  you, and what it has been doing. `ui`: { activeKey, openMenuKey }. */
export function inboxRowHtml(entry, ui = {}) {
  if (entry.kind === "capture") return captureRowHtml(entry, ui);
  const badge = entry.state === "unread" && entry.unreadCount > 1 ? `<span class="badge">${entry.unreadCount}</span>` : "";
  const mark =
    entry.kind === "issue"
      ? '<span class="inbox-mark">Issue</span>'
      : entry.branch
        ? `<span class="inbox-mark mono">${esc(entry.branch)}</span>`
        : '<span class="inbox-mark">detached</span>';
  const done = entry.canFinish
    ? `<button class="btn mini" data-done="${esc(entry.key)}" type="button" aria-label="Done with ${esc(entry.title)}">Done</button>`
    : "";
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
  } title="${esc(entry.title)}">
    <span class="sdot sdot-${entry.state}" title="${entry.state}"></span>
    <div class="inbox-body">
      <div class="inbox-line"><span class="stitle">${esc(entry.title)}</span>${badge}</div>
      <div class="inbox-line inbox-where"><span class="inbox-tag">${esc(entry.project)}</span>${mark}${
        entry.muted ? '<span class="inbox-mark">muted</span>' : ""
      }</div>
      ${entry.reason ? `<div class="inbox-reason">${esc(entry.reason)}</div>` : ""}
      ${entry.facts ? `<div class="inbox-facts">${esc(entry.facts)}</div>` : ""}
      <span class="warn" data-done-error hidden></span>
    </div>
    <div class="inbox-actions">${done}${menuHtml(entry, ui.openMenuKey === entry.key)}</div>
  </div>`;
}

export function inboxListHtml(entries, ui = {}) {
  if (!entries.length) {
    return '<div class="inbox-clear dim">Nothing needs you. Work you start shows up here.</div>';
  }
  return entries.map((entry) => inboxRowHtml(entry, ui)).join("");
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
  return `<div class="${classes}" data-key="${esc(entry.key)}" data-capture="${esc(entry.captureId)}" title="${esc(entry.text || entry.title)}">
    <span class="sdot sdot-${entry.state}" title="${entry.state}"></span>
    <div class="inbox-body">
      <div class="inbox-line"><span class="stitle">${esc(entry.title)}</span></div>
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
// Done is decisive: it archives the entry, and for a branch implementing an
// issue it marks both. The modal outlines exactly what will happen (the
// core/confirm.js contract), and the linked marking's override sits one step
// behind the refusal that names it.

export function branchDoneConfirm(entry) {
  const branch = entry.branch || "this checkout";
  const actions = [`Remove the checkout for ${branch}`, `Keep branch ${branch}`];
  if (entry.issueId) actions.push("Archive the issue it implements, with its stage plans");
  return {
    title: `Done with ${branch}?`,
    intro: "Its work is committed and pushed.",
    actions,
    confirmLabel: "Done",
    danger: false,
  };
}

export function issueDoneConfirm(entry) {
  return {
    title: "Done with this issue?",
    intro: "Its implementation is complete.",
    actions: ["Move the issue and its stage plans to the archive", "Keep the implementation lineage"],
    confirmLabel: "Done",
    danger: false,
  };
}

/** The disclosure behind the linked marking: the bridge refuses to archive an
 *  issue that is not implemented, and names the way past it. Its own words are
 *  the intro — the rule belongs to the bridge, not to a copy of it here. */
export function unlinkDisclosure(entry, message) {
  return {
    title: "Finish the branch alone?",
    intro: message,
    actions: [`Remove the checkout for ${entry.branch || "this checkout"}`, "Leave the issue it implements open"],
    confirmLabel: "Finish branch only",
    danger: false,
  };
}
