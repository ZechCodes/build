// The assignee control: one select over all five kinds, the sentence saying
// what the chosen one is about to do, and the extra fields the two creating
// kinds need.
//
// Two surfaces ask the same question and mean exactly the same thing by it —
// the picker a row, a card or the rail opens, and the new-issue form — so both
// ask it with this. The control renders and reads; whoever mounts it owns the
// repaint, because a change here changes which fields exist.
//
// The sentence under the select is not decoration. Assigning IS dispatching:
// choosing "New workspace and agent" cuts a workspace and starts an agent on
// it, and the reader must be told that before they press, not after.

import { esc } from "./text.js";
import { NO_AGENT_CHOICE, agentChoicePanelHtml, agentChoiceParams, readAgentChoice, reconcileAgentChoice } from "./agentChoice.js";
import { isolationOptionsHtml } from "./isolation.js";
import { WORKSPACE_FORM, assignParams, optionWaitsOnAWorkspace } from "./trackerAssignee.js";
import { fieldTraits } from "./fieldTraits.js";

/** Nothing chosen: the control opens on whatever the caller says is current. */
export const emptyAssigneeDraft = (optionId = "none") => ({
  optionId,
  name: "",
  isolation: "",
  choice: { ...NO_AGENT_CHOICE },
  choiceOpen: false,
});

const optionOf = (options, optionId) => options.find((option) => option.id === optionId) || null;

/** The select, with each workspace's agents under a heading of its own. Built
 *  from the flat option list rather than from a nested one, so the picker, the
 *  form and a future menu all group the same way without re-deriving it. */
function optionsHtml(options, chosen) {
  let openGroup = "";
  const parts = [];
  for (const option of options) {
    if (option.group !== openGroup) {
      if (openGroup) parts.push("</optgroup>");
      openGroup = option.group;
      if (openGroup) parts.push(`<optgroup label="${esc(openGroup)}">`);
    }
    parts.push(`<option value="${esc(option.id)}"${option.id === chosen ? " selected" : ""}>${esc(option.label)}</option>`);
  }
  if (openGroup) parts.push("</optgroup>");
  return parts.join("");
}

/** The new-workspace form's own two fields. A blank name means the issue's
 *  title and "Inherit project setting" means no isolation is sent at all, so
 *  neither is a required field and neither is prefilled with a guess. */
const workspaceFieldsHtml = (draft, prefix) => `<label class="create-label" for="${esc(prefix)}-workspace">Workspace label</label>
  <input id="${esc(prefix)}-workspace" type="text" ${fieldTraits("identifier")} placeholder="The issue's title" value="${esc(draft.name)}" />
  <label class="create-label" for="${esc(prefix)}-isolation">Isolation</label>
  <select id="${esc(prefix)}-isolation">${isolationOptionsHtml(draft.isolation, null, { inheritLabel: "Inherit project setting" })}</select>`;

/**
 * The control.
 *
 * `options` is what core/trackerAssignee.js offered; `draft` is what has been
 * chosen so far; `catalog` is the creation device's `models.list`. An option
 * that starts nothing shows no extra fields — there is no agent to configure.
 */
export function assigneeControlHtml(options, draft, { prefix, catalog, label = "Assignee" }) {
  const option = optionOf(options, draft.optionId);
  return `<div class="issue-assignee-control" data-assignee-control="${esc(prefix)}">
    <label class="create-label" for="${esc(prefix)}-assignee">${esc(label)}</label>
    <select id="${esc(prefix)}-assignee" data-assignee-select>${optionsHtml(options, draft.optionId)}</select>
    <p class="sub issue-assignee-hint">${esc(option?.hint || "")}</p>
    ${option?.form === WORKSPACE_FORM ? workspaceFieldsHtml(draft, prefix) : ""}
    ${option?.form ? agentChoicePanelHtml(catalog, draft.choice, { prefix: `${prefix}-choice`, open: draft.choiceOpen }) : ""}
  </div>`;
}

const valueOf = (root, id) => root.querySelector(`#${id}`)?.value || "";

/** What the control currently says, as the next draft. Read whole on every
 *  change, so a field that was hidden and shown again is read from the DOM that
 *  exists rather than from a memory of one that did. */
export function readAssigneeDraft(root, draft, prefix) {
  return {
    ...draft,
    optionId: valueOf(root, `${prefix}-assignee`) || draft.optionId,
    name: valueOf(root, `${prefix}-workspace`),
    isolation: valueOf(root, `${prefix}-isolation`),
  };
}

/**
 * Wire the control into a host that knows how to repaint itself.
 *
 * Every change here changes what the control IS — a different option shows
 * different fields, a different provider offers different models — so each one
 * hands the host a new draft and asks for a repaint. The host repaints, and
 * calls this again.
 */
export function wireAssigneeControl(root, draft, { prefix, onDraft }) {
  const control = root.querySelector(`[data-assignee-control="${prefix}"]`);
  if (!control) return;
  control.querySelector("[data-assignee-select]").onchange = () =>
    onDraft({ ...readAssigneeDraft(root, draft, prefix), choiceOpen: draft.choiceOpen });
  const name = root.querySelector(`#${prefix}-workspace`);
  if (name) name.oninput = () => onDraft(readAssigneeDraft(root, draft, prefix));
  const isolation = root.querySelector(`#${prefix}-isolation`);
  if (isolation) isolation.onchange = () => onDraft(readAssigneeDraft(root, draft, prefix));
  wireChoice(root, draft, { prefix, onDraft });
}

/** The three harness selects. A model belongs to its provider and an effort to
 *  its model, so changing one drops what hung off it — the same reconciliation
 *  the compose box makes, through the same pure function. */
function wireChoice(root, draft, { prefix, onDraft }) {
  const choicePrefix = `${prefix}-choice`;
  const holder = root.querySelector(`[data-assignee-control="${prefix}"] .agent-choice`);
  if (!holder) return;
  holder.querySelector("[data-agent-choice-toggle]").onclick = () =>
    onDraft({ ...draft, choiceOpen: !draft.choiceOpen });
  const onChange = (changed) => () =>
    onDraft({ ...draft, choice: reconcileAgentChoice(readAgentChoice(root, choicePrefix), changed) });
  const bind = (field, changed) => {
    const control = holder.querySelector(`#${choicePrefix}-${field}`);
    if (control) control.onchange = onChange(changed);
  };
  bind("provider", { providerChanged: true });
  bind("model", { modelChanged: true });
  bind("effort", {});
}

/**
 * What the draft is, as `issues.assign` params.
 *
 * The harness/model/effort go through `agentChoiceParams`, which clamps a stale
 * provider to what the select actually painted and omits everything nobody
 * chose — absent is absent, and `agent.add` reads a key's presence to tell "run
 * it on this" from "run it on whatever the workspace runs on". An option that
 * starts nothing is asked for no choice at all.
 */
export function draftAssignParams(options, draft, catalog, { issueId, note = "" } = {}) {
  const option = optionOf(options, draft.optionId);
  return assignParams(issueId, option, {
    name: draft.name,
    isolation: draft.isolation,
    choice: option?.form ? agentChoiceParams(catalog, draft.choice) : null,
    note,
  });
}

/** The same choice, as the `assignee` alone — what `issues.create` carries it
 *  as, since a create names its own issue and has no id to assign to yet. */
export const draftAssignee = (options, draft, catalog) =>
  draftAssignParams(options, draft, catalog, { issueId: "" }).assignee;

/** Whether the chosen option starts an agent, which is what a confirm button
 *  says out loud: "Assign" for the three that start nothing, and the option's
 *  own consequence for the two that do. */
export const draftStartsWork = (options, draft) => {
  const option = optionOf(options, draft.optionId);
  return Boolean(option) && option.kind !== "unassign" && option.kind !== "user";
};

/** Whether this draft is the one that makes the caller wait on a checkout
 *  being cut. What tells a dialog doing minutes of real work from a hung one. */
export const draftWaitsOnAWorkspace = (options, draft) =>
  optionWaitsOnAWorkspace(optionOf(options, draft.optionId));
