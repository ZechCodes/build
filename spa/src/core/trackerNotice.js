// A tracking notice, as one line in the conversation it lands in.
//
// The maintainer, with a screenshot of a project-agent conversation: "Tracking
// notices come in looking like user messages (same color and on the right).
// They should be a single line 'X did Y on Z' deep linking."
//
// They looked like that because that is what they are on the wire: a message
// on the user's side, marked `from_build`, carrying a `from_task` envelope
// and a body that reads as prose. Drawn as an ordinary bubble it reads as
// though the user wrote it — right-aligned, their colour, the whole comment
// body under it — which is three wrong claims in one row.
//
// So a message carrying both marks is a NOTICE: one quiet left-aligned line,
// the whole of it an anchor to the task, and the comment body not shown at
// all. A press is what opens it; the line is the news that there is something
// to open.
//
// # Two sources, one shape
//
// `task_notice` is the structured field (#35), and is read when it is there.
// Everything sent before it exists carries the same facts only as prose, so
// the body's first line is parsed as a fallback.
//
// The parse is allowed to fail. `from_task` already carries the id, the
// number and the title, so the LINK never depends on it — only the actor and
// the verb do, and a line that cannot name them says the task alone rather
// than nothing. A notice that degrades to "#32 Title" is still a notice, still
// on the left, still one line, and still opens the right page.
//
// Pure: no DOM, no app imports.

import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { actionPhrase, actorName } from "./trackerLineWords.js";
import { FALLBACK_COLUMNS } from "./trackerModel.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { projectInitial } from "./agentRailModel.js";
import { actorHref } from "./trackerIdentity.js";

export const NOTICE_CLASS = "thread-task-notice";

/** Whether this message is a tracking notice rather than something a person
 *  or an agent said. Both marks, because `from_build` alone is the restart
 *  notice — which is an instruction to the agent and reads as one. */
export const isTaskNotice = (message) => Boolean(message?.from_build && message?.from_task);

/**
 * The body's first line, as the facts it states.
 *
 * The shape the bridge writes is `#32 Some title — agent-01M… commented: …`.
 * Split at the LAST em dash before the tail, because a title may contain one
 * and the actor never does.
 */
function noticeFromBody(body) {
  const line = String(body || "").split("\n")[0].trim();
  const at = line.lastIndexOf(" — ");
  if (!line.startsWith("#") || at === -1) return null;
  const tail = line.slice(at + 3).trim();
  // `<actor> <verb>` up to the colon that introduces what was said.
  const said = tail.split(":")[0].trim();
  const gap = said.indexOf(" ");
  if (gap === -1) return null;
  return { actor: said.slice(0, gap), action: said.slice(gap + 1).trim() };
}

/**
 * One notice's facts, from the structured field where there is one and from
 * the body's prose where there is not.
 *
 * The task itself always comes from the envelope: it is the one part that was
 * never prose, and the link is built from it whether the rest parses or not.
 */
/** Which task, from the envelope first: it is the one part that was never
 *  prose, so it is the one part that cannot be lost to a parse. */
const taskPartOf = (envelope, stated) => ({
  task_id: envelope.task_id || stated?.task_id || "",
  number: envelope.number ?? stated?.number ?? null,
  title: envelope.title || stated?.title || "",
});

/** Who did what, from the field where there is one and from the prose where
 *  there is not. The prose carries no comment id — there is nowhere in a
 *  sentence for one — so a fallback notice lands on the task rather than on
 *  the comment, which is the right page either way. */
function actPartOf(stated, body) {
  if (stated) {
    return statedActPart(stated);
  }
  const parsed = noticeFromBody(body) || {};
  return { actor: parsed.actor || "", action: parsed.action || "", assignee: null, assignee_identity: null, comment_id: null, to: null };
}

const statedActPart = (stated) => ({
  actor: stated.actor || "",
  action: stated.action || "",
  assignee: stated.assignee || null,
  assignee_identity: stated.assignee_identity || null,
  comment_id: stated.comment_id || null,
  to: stated.to || null,
});

export function taskNoticeOf(message) {
  if (!isTaskNotice(message)) return null;
  const stated = message.task_notice || null;
  return { ...taskPartOf(message.from_task || {}, stated), ...actPartOf(stated, message.body) };
}

/** Where the line goes: the task's page, and the comment itself when the
 *  notice is about one. A conversation with no project to stand in writes no
 *  href, and the line draws as plain text rather than pointing nowhere. */
export function noticeHref(notice, place) {
  if (!place?.projectId || !notice?.task_id) return "";
  const base = hashFromRoute({
    name: "trackerTask",
    projectId: place.projectId,
    deviceId: place.deviceId ?? null,
    taskId: notice.task_id,
  });
  return notice.comment_id ? `${base}#comment-${encodeURIComponent(notice.comment_id)}` : base;
}

/**
 * The line: "{actor} {action} on #{number} {title}".
 *
 * The actor and the verb are dropped when neither source named them, rather
 * than being guessed at or left as an empty gap — "#32 Title" is a true
 * sentence about a task, and "acted on #32" is a claim nothing backs.
 */
/**
 * What happened, with the detail the verb needs.
 *
 * "assigned" on its own leaves the reader asking the obvious question, and the
 * notice already carries the answer. Every other verb either needs nothing or
 * arrives as a phrase that carries its own ("moved to In review").
 */
export function noticeAction(notice, reading = {}) {
  const did = actionPhrase(notice?.action);
  // A phrase the bridge wrote already says where or to whom; saying it twice
  // is worse than not saying it at all.
  if (!did || /\bto\b/.test(did)) return did;
  const target = actionTarget(notice, noticeReading(notice, reading));
  return target ? `${did} to ${target}` : did;
}

const noticeReading = (notice, reading) => {
  const identity = notice?.assignee_identity;
  return identity ? { ...reading, identities: { ...reading.identities, [identity.agent_id]: identity } } : reading;
};

/** What the verb points at: the column a move landed in (its own field on the
 *  notice; the maintainer, 21:15Z: "What does 'moved by' mean? Moved where?"),
 *  or the person an assignment went to. */
function actionTarget(notice, reading) {
  if (notice?.to) return columnName(notice.to);
  if (!notice?.assignee) return "";
  const toWhom = actorName(notice.assignee, reading);
  if (!toWhom) return "";
  return toWhom.toLowerCase() === "you" ? "you" : toWhom;
}

/** The column's display name for a slug the bridge sent, or the slug itself as
 *  words when the bridge named a column this client has never heard of. */
const columnName = (slug) =>
  FALLBACK_COLUMNS.find((column) => column.id === slug)?.name ?? String(slug).replace(/_/g, " ");

/**
 * What happened, apart from what it happened to. The line leads with the verb
 * (#217) and the target is a section of its own — a line of its own when the
 * notice stacks — so a phrase that carries its target ("moved to Done",
 * "assigned to Agent 2") is cut at its "to".
 */
const TO = " to ";
const actionParts = (did) => {
  const at = did.indexOf(TO);
  return at === -1 ? { verb: did, target: "" } : { verb: did.slice(0, at), target: did.slice(at + TO.length) };
};
const capitalised = (text) => text.charAt(0).toUpperCase() + text.slice(1);

/** The whole line as words: "Moved #41 to In review by transport-liveness · Agent 1". */
export function noticeLineText(notice, reading = {}) {
  const { verb, target } = actionParts(noticeAction(notice, reading));
  const who = actorName(notice?.actor, reading);
  return [capitalised(verb), `#${notice?.number ?? ""}`, target && `to ${target}`, who && `by ${who}`]
    .filter(Boolean).join(" ");
}

const noticeActorMarkHtml = (actor, projectName) => {
  if (actor?.kind === "project_agent" || actor?.agent_id?.startsWith("project-")) {
    return `<span class="thread-task-actor-mark is-project" aria-hidden="true">${esc(projectInitial(projectName || "Build"))}</span>`;
  }
  const provider = actor?.identity?.provider;
  return provider ? `<span class="thread-task-actor-mark" aria-hidden="true">${harnessIconHtml(provider)}</span>` : "";
};

/** An agent's name, "airlock-queue · Queue hardener": the workspace and the
 *  agent. On one line they read as written; stacked, the agent's name stays
 *  beside the "by" that introduces it and the workspace sits under it
 *  (styles/tasks.css). Either part too long for its line ellipsises, its whole
 *  text kept as hover text. One name, one link. */
const NAME_SEPARATOR = " · ";
const namePartHtml = (kind, text) => `<span class="thread-task-${kind}" title="${esc(text)}">${esc(text)}</span>`;
const namePartsHtml = (name, mark) => {
  const parts = String(name).split(NAME_SEPARATOR);
  const agent = namePartHtml("agent-name", parts.pop());
  if (!parts.length) return `${mark}${agent}`;
  const workspace = namePartHtml("workspace", parts.join(NAME_SEPARATOR));
  return `${mark}${workspace}<span class="thread-task-name-sep">${NAME_SEPARATOR}</span>${agent}`;
};
const nameHtml = (name, mark, href) => (href
  ? `<a class="thread-task-name" href="${esc(href)}">${namePartsHtml(name, mark)}</a>`
  : `<span class="thread-task-name">${namePartsHtml(name, mark)}</span>`);

/** One name on the line — the assignee, who did it — which the stacked form
 *  puts on its own indented line. The word introducing it ("to", "by") is its
 *  own box, so every name starts at the same x under the one above. */
const sectionHtml = (kind, word, name) =>
  `<span class="thread-task-section thread-task-${kind}"><span class="thread-task-word">${word}</span> ${name}</span>`;

const noticeSaidHtml = (verb, taskHref) => {
  if (!verb) return "";
  const said = esc(capitalised(verb));
  return `<span class="thread-task-said">${taskHref ? `<a class="thread-task-action" href="${esc(taskHref)}">${said}</a>` : said}</span>`;
};

const noticeNumberHtml = (notice, taskHref) => {
  const number = `#${esc(String(notice.number ?? ""))}`;
  return taskHref
    ? `<a class="thread-task-number" href="${esc(taskHref)}">${number}</a>`
    : `<span class="thread-task-number">${number}</span>`;
};

/** Whom an assignment went to: a name, drawn as the actor's is, with their
 *  mark and a link to their conversation. Anything else a verb points at — a
 *  column — is part of what happened and stays on the first line. */
const namesAnAssignee = (notice) => notice.action === "assigned" && Boolean(notice.assignee);

const noticeAssigneeHtml = (notice, reading, linkContext) => {
  const assignee = actorName(notice.assignee, noticeReading(notice, reading));
  const mark = noticeActorMarkHtml({ ...notice.assignee, identity: notice.assignee_identity }, reading.projectName);
  return sectionHtml("to", "to", nameHtml(assignee, mark, linkContext && actorHref(notice.assignee, linkContext)));
};

/** The first line, whole in either form: what happened and to which task —
 *  "Commented on #216", "Moved #32 to In review" (#217, the maintainer: "keep
 *  the issue number on the same line as the action"). */
const noticeHeadHtml = (notice, verb, target, taskHref) => {
  const column = target && !namesAnAssignee(notice) ? ` to ${esc(target)}` : "";
  const said = [noticeSaidHtml(verb, taskHref), noticeNumberHtml(notice, taskHref)].filter(Boolean).join(" ");
  return `<span class="thread-task-first-line">${said}${column}</span>`;
};

const noticeByHtml = (notice, who, reading, linkContext) => {
  if (!who) return "";
  const mark = noticeActorMarkHtml(notice.actor, reading.projectName);
  return sectionHtml("by", "by", nameHtml(who, mark, linkContext && actorHref(notice.actor, linkContext)));
};

const noticeSpansHtml = (notice, did, who, reading, taskHref = "", linkContext = null) => {
  const { verb, target } = actionParts(did);
  return [
    noticeHeadHtml(notice, verb, target, taskHref),
    target && namesAnAssignee(notice) ? noticeAssigneeHtml(notice, reading, linkContext) : "",
    noticeByHtml(notice, who, reading, linkContext),
  ].filter(Boolean).join(" ");
};

const noticeLinkContext = (notice, place, workspaces) => {
  if (!place) return null;
  const identities = [notice.actor?.identity, notice.assignee_identity]
    .filter((identity) => identity?.agent_id)
    .reduce((byId, identity) => ({ ...byId, [identity.agent_id]: identity }), {});
  return { ...place, identities, workspaces };
};

const splitNoticeNames = (notice, context) => Boolean(context &&
  (notice.actor?.identity || notice.assignee_identity || actorHref(notice.actor, context)));

const noticeWrapHtml = (notice, said, href, hover, split) => {
  const attrs = `${hover} data-task-notice="${esc(notice.task_id)}"`;
  if (split || !href) return `<span class="${NOTICE_CLASS}"${attrs}>${said}</span>`;
  return `<a class="${NOTICE_CLASS}" href="${esc(href)}"${attrs}>${said}</a>`;
};

/**
 * The line: `Moved #41 to In review by transport-liveness · Agent 1`.
 *
 * The maintainer, on the lines as #40 shipped them: "Relevant info is getting
 * pushed out of view … Move what happened first and don't show the task
 * title." And on #217, when a long one wrapped into five ragged lines: lead
 * with the action, and when the line does not fit put each section on its
 * own indented line (core/noticeFit.js decides which).
 *
 * So what happened leads, then the number — the deep link and the thing a
 * person says out loud — then where it went and who did it. The title is not
 * on the line: it was the longest part and the first to be cut off, and it is
 * the heading of the page the link opens. It stays as hover text, where
 * length costs nothing.
 */
export function taskNoticeLineHtml(notice, { place = null, agentLabels = {}, projectName = "", workspaces } = {}) {
  if (!notice?.task_id) return "";
  const reading = { agentLabels, projectName };
  const did = noticeAction(notice, reading);
  const who = actorName(notice.actor, reading);
  const hover = notice.title ? ` title="${esc(notice.title)}"` : "";
  const href = noticeHref(notice, place);
  const linkContext = noticeLinkContext(notice, place, workspaces);
  const splitNames = splitNoticeNames(notice, linkContext);
  const said = noticeSpansHtml(notice, did, who, reading, splitNames ? href : "", splitNames ? linkContext : null);
  return noticeWrapHtml(notice, said, href, hover, splitNames);
}
