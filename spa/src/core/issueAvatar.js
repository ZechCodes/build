// The picture beside a comment on the issue page (#54).
//
// Zech: "On issue comments show the harness icon as the profile picture."
//
// Every author wore one grey circle with a letter in it, and the letter was
// the one thing about an author that carries nothing: "A" for every agent
// that has ever commented, whichever harness it runs on and whether it was
// the project's own agent or one standing in a workspace.
//
// So the mark resolves from the author. The user keeps their own. The
// project's agent wears the project's face — the initial in a squared-off box,
// the same face the rail draws its bubble with (core/agentRailModel.js), so
// the page and the strip say whose agent this is the same way. A workspace
// agent wears the icon of the harness it runs on, drawn through the rail's own
// `harnessIconHtml` rather than a second table of providers here: a copy of
// that table is a copy that goes stale, and the artwork is already bundled.
//
// An agent this client has never read — one from a workspace it has not
// opened, or one since gone — keeps the generic mark. A picture is a claim
// about what wrote the comment, and guessing one is worse than not making it.
//
// Pure: HTML in, no DOM.

import { esc } from "./text.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { projectInitial } from "./agentRailModel.js";
import { actorInitials, actorLabel } from "./trackerModel.js";

/** The id a project's own agent is minted under. The bridge writes it as an
 *  ordinary agent actor on some paths and as its own actor kind on others, so
 *  both are asked about — one agent must not have two faces. */
const PROJECT_AGENT_PREFIX = "project-";

export const isProjectActor = (actor) =>
  actor?.kind === "project_agent" || String(actor?.agent_id || "").startsWith(PROJECT_AGENT_PREFIX);

/**
 * The mark for one author.
 *
 * `agentProviders` is what this client knows of the project's agents — built
 * off the same workspace agents the assignee picker reads
 * (core/trackerAssignee.js), so an agent is drawn by the record it is named
 * by. `projectName` is what the project's initial is cut from.
 *
 * The box is what it always was: same class, same size, same place in the
 * card. Only the picture inside it changes.
 */
export function issueAvatarHtml(actor, context = {}) {
  // The label the card already shows in its head, as hover text: the picture
  // is a hint, and a hint nobody can resolve is a worse one.
  const title = ` title="${esc(actorLabel(actor, context.agentLabels || {}))}"`;
  if (isProjectActor(actor)) return projectMarkHtml(actor, title, context.projectName);
  const provider = harnessOf(actor, context.agentProviders || {});
  if (provider) return `<span class="issue-avatar is-harness"${title} aria-hidden="true">${harnessIconHtml(provider)}</span>`;
  return `<span class="issue-avatar"${title} aria-hidden="true">${esc(actorInitials(actor))}</span>`;
}

/** The project's face: its initial, squared off by the CSS the way the rail
 *  squares its bubble. With no name for the project there is no initial to cut
 *  from, and the generic letter says more than a question mark does. */
const projectMarkHtml = (actor, title, projectName) =>
  `<span class="issue-avatar is-project"${title} aria-hidden="true">${
    esc(projectName ? projectInitial(projectName) : actorInitials(actor))}</span>`;

/** The harness an agent author runs on, as far as this client knows. */
const harnessOf = (actor, agentProviders) =>
  (actor?.kind === "agent" ? agentProviders[actor.agent_id] || "" : "");
