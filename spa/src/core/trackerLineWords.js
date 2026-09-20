// The words an issue line says: what was done, and who did it.
//
// One table for both lines — the tracking notice (#38) and the agent's own
// action line (#18) — because the same verb reaching a reader two ways with
// two spellings is how a vocabulary drifts. They were two tables for a while
// and they had already drifted: one said "commented on" and the other said
// "commented", and neither handled the token the bridge actually sends.
//
// Zech, on the rolled build (#40): the rows read "commented_on #39" and
// "created #39" with no actor, and the project's own agent came out as
// "Agent 01M2" — it is not an agent of any workspace, so the feed had no label
// for it and it fell through to four characters of its id.
//
// Pure: no DOM, no app imports.

import { actorLabel } from "./trackerModel.js";

/**
 * What the bridge's verb is called on screen.
 *
 * Keys are matched after lowercasing and after underscores become spaces, so
 * `comment`, `commented` and `commented_on` are one entry rather than three.
 */
const ACTION_WORDS = Object.freeze({
  "create": "created",
  "created": "created",
  "comment": "commented on",
  "commented": "commented on",
  "comment on": "commented on",
  "commented on": "commented on",
  "assign": "assigned",
  "assigned": "assigned",
  // The wire's word is `issues.update`; a reader calls it an edit, and the
  // reader's word is the one on screen.
  "update": "edited",
  "updated": "edited",
  "edit": "edited",
  "edited": "edited",
  "move": "moved",
  "moved": "moved",
  "close": "closed",
  "closed": "closed",
  "reopen": "reopened",
  "reopened": "reopened",
  "link": "linked",
  "linked": "linked",
  "track": "tracked",
  "tracked": "tracked",
});

/**
 * The verb, in the sentence the line is.
 *
 * A phrase the bridge wrote — "moved to In review", "assigned to Agent 2" —
 * passes through as it reads: the verbs that need a target carry one, and a
 * client that rewrote those would have to know every column and every agent
 * there will ever be.
 *
 * A token this build has never heard of still reads as words rather than as a
 * token, because the underscore on screen is the defect being fixed and a new
 * verb from a later bridge must not reintroduce it.
 *
 * Always mid-sentence: since #49 the number leads every line, so nothing here
 * ever opens one.
 */
export function actionPhrase(action) {
  const said = String(action || "").trim();
  if (!said) return "";
  const words = said.replace(/_/g, " ");
  return ACTION_WORDS[words.toLowerCase()] || words;
}

/** What Build's own agent for a project is called, when nothing names the
 *  project. It is Build's agent either way; the project's name only makes it
 *  the right one of several. */
const BUILD = "Build";

const projectAgentName = (projectName) => `${String(projectName || "").trim() || BUILD} agent`;

/**
 * Who did it, as a reader knows them.
 *
 * Takes the tagged actor shape and the bare strings the wire and the body's
 * prose both carry, because a notice's actor arrives either way depending on
 * whether the structured field is there.
 *
 * Never a bare id. An agent of a workspace is what the rest of the project
 * calls it; the project's own agent is named after its project; an agent
 * nothing can name wears the four characters it wears everywhere else.
 */
export function actorName(actor, reading = {}) {
  if (!actor) return "";
  if (typeof actor !== "string") return taggedActorName(actor, reading);
  return writtenActorName(actor.trim(), reading);
}

/** The wire's tagged shape, which every field but a notice's actor uses. */
function taggedActorName(actor, { agentLabels = {}, projectName = "" }) {
  if (actor.kind === "project_agent") return projectAgentName(projectName);
  return actorLabel(actor, agentLabels);
}

/** A bare string: an id off the structured field, or a word out of the body's
 *  prose. Both reach this, because a notice's actor arrives either way
 *  depending on whether the bridge carries the field yet. */
function writtenActorName(said, { agentLabels = {}, projectName = "" }) {
  if (!said) return "";
  if (said.startsWith("project-")) return projectAgentName(projectName);
  if (said.startsWith("agent-")) return actorLabel({ kind: "agent", agent_id: said }, agentLabels);
  if (said === "user" || said === "you") return "You";
  return said;
}
