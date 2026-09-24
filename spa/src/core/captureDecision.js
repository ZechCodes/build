// The capture decision page's pure model: what the page says about one capture,
// the router's offer read as choices a user can tap, and the call each way of
// answering makes.
//
// A capture the router cannot place is a question, and a question deserves a
// surface rather than a text field wedged into an inbox row. This page states
// what was said, what the router asked, and every way out of it: the choices
// the router offered, a destination the user names themselves, words typed in
// answer, and abandoning the capture altogether.
//
// No DOM, no app imports — core/captureDecisionView.js wires these.

import { esc } from "./text.js";
import { captureStatusText } from "./inbox.js";
import { fieldTraits } from "./fieldTraits.js";

const trimmed = (value) => String(value ?? "").trim();

const projectNameOf = (projects, projectId) =>
  (projects || []).find((project) => project.id === projectId)?.name || projectId || "";

/** What an option stands for, in the terms the user picked it by: the project,
 *  and the thing it becomes there. An option that is only a label says nothing
 *  here rather than guessing a destination for it. */
export function optionDestinationText(option, projects = []) {
  const project = projectNameOf(projects, trimmed(option.project_id));
  const branch = trimmed(option.branch);
  // A branch name says the option is a branch — the same rule the bridge
  // numbers options by.
  const kind = branch ? "branch" : option.kind;
  const becomes = branch ? `branch ${branch}` : kind === "branch" ? "a new branch" : kind === "issue" ? "a new issue" : "";
  return [project, becomes].filter(Boolean).join(" · ");
}

/**
 * One capture, as the decision page reads it.
 *
 * The routing states are the inbox's own words (captureStatusText) so the page
 * and the row it was opened from never disagree — with one state the row has
 * no name for: a question nobody has answered is not the router working, it is
 * the router waiting on this page.
 */
// eslint-disable-next-line complexity -- ratchet: captureDecisionModel is at 21, cap 10 — reduce it, then drop this line
export function captureDecisionModel(capture, { projects = [] } = {}) {
  const record = capture || {};
  const question = record.question || null;
  const routing = record.routing || null;
  const state = trimmed(record.state) || "unrouted";
  const awaitingAnswer = Boolean(question) && !question.answer;
  const routedTo = routing ? { project: projectNameOf(projects, routing.project_id), kind: routing.kind } : null;
  return {
    captureId: trimmed(record.id),
    said: String(record.text ?? ""),
    state,
    routed: state === "routed",
    routedTo,
    statusText: awaitingAnswer ? "Waiting for your answer" : (trimmed(record.progress) || captureStatusText({ captureState: state, routedTo })),
    spinning: !awaitingAnswer && (state === "unrouted" || state === "routing"),
    question: question ? String(question.text ?? "") : "",
    answer: question && question.answer ? String(question.answer) : "",
    awaitingAnswer,
    chosenOptionId: (question && question.chosen_option_id) || null,
    options: (question?.options || []).filter((option) => option.kind !== "issue" || trimmed(option.branch)).map((option) => ({
      id: option.id,
      label: String(option.label ?? ""),
      destination: optionDestinationText(option, projects),
    })),
  };
}

/** The answer a destination the user named reads as: the same sentence the
 *  bridge writes when a router's own option is tapped, so the router hears a
 *  hand-picked destination in exactly the terms it routes in. */
export function manualRouteAnswer({ projectId, branch = "" } = {}) {
  const project = trimmed(projectId);
  if (!project) return "";
  const named = trimmed(branch);
  return `Route this to project ${project} as a branch${named ? `, on the branch ${named}` : ""}`;
}

/** What a reroute asks for — the destination stated to the daemon rather than
 *  to the router. This is the way out when there is no question to answer: an
 *  unnamed branch is the daemon naming it after what was said. */
export function manualRouteParams(captureId, { projectId, branch = "" } = {}) {
  const params = { capture_id: captureId, project_id: trimmed(projectId), kind: "branch" };
  const named = trimmed(branch);
  return named ? { ...params, branch: named } : params;
}

/** What `capture.answer` asks for: the option that was tapped, or the words
 *  that were typed. `null` when neither says anything — an empty answer answers
 *  nothing, and the bridge would refuse it. */
export function answerParams(captureId, { optionId = "", text = "" } = {}) {
  const chosen = trimmed(optionId);
  if (chosen) return { capture_id: captureId, option_id: chosen };
  const said = trimmed(text);
  return said ? { capture_id: captureId, text: said } : null;
}

/** The way out, outlined. Cancelling forgets the record, so the modal says so
 *  before it happens rather than after. */
export function captureCancelConfirm(model) {
  return {
    title: "Cancel this capture?",
    intro: model.said,
    actions: ["Stop the router deciding where this goes", "Forget what was captured", "Take it off the inbox"],
    confirmLabel: "Cancel the capture",
    cancelLabel: "Keep it",
    danger: true,
  };
}

// ---- the page ----------------------------------------------------------------

/** What was said, and what is happening to it. */
function saidHtml(model) {
  return `<div class="panel capture-said">
    <h3>You said</h3>
    <div class="capture-said-text">${esc(model.said)}</div>
    <div class="capture-decide-status${model.spinning ? " dim" : ""}">${
      model.spinning ? '<span class="capture-spinner" aria-hidden="true"></span>' : ""
    }<span>${esc(model.statusText)}</span></div>
  </div>`;
}

/** The router's question, and the choices it offered beside it. Every choice is
 *  its own control: the offer is meant to be one tap. */
function askHtml(model, ui) {
  if (!model.question) return "";
  const shut = !model.awaitingAnswer || ui.busy ? " disabled" : "";
  const choices = model.options
    .map(
      (choice) => `<button class="capture-option${choice.id === model.chosenOptionId ? " chosen" : ""}" type="button"
        data-capture-option="${esc(choice.id)}"${shut}>
        <span class="capture-option-label">${esc(choice.label)}</span>${
          choice.destination ? `<span class="capture-option-where">${esc(choice.destination)}</span>` : ""
        }</button>`,
    )
    .join("");
  return `<div class="panel capture-ask">
    <h3>The router asks</h3>
    <p class="capture-ask-text">${esc(model.question)}</p>
    ${model.answer ? `<div class="capture-answered dim">You answered: ${esc(model.answer)}</div>` : ""}
    ${choices ? `<div class="capture-options">${choices}</div>` : ""}
  </div>`;
}

/** The destination named by hand: which project, what it becomes there, and —
 *  for a branch, the one destination with something left to say — which branch.
 *  The same three fields the compose box's manual panel offers, because it is
 *  the same decision. */
function manualHtml(model, ui) {
  const busy = ui.busy ? " disabled" : "";
  const projectOptions = (ui.projects || [])
    .map(
      (project) =>
        `<option value="${esc(project.id)}"${project.id === ui.projectId ? " selected" : ""}>${esc(
          project.name || project.id,
        )}</option>`,
    )
    .join("");
  const branches = (ui.branches || []).map((branch) => `<option value="${esc(branch)}"></option>`).join("");
  return `<div class="panel capture-manual">
    <h3>${model.question ? "Or send it somewhere yourself" : "Send it somewhere yourself"}</h3>
    <label for="capture-project">Project</label>
    <select id="capture-project"${busy}>${
      projectOptions || '<option value="">No projects on this device</option>'
    }</select>
    <div class="compose-kinds"><button class="btn mini primary" type="button" data-capture-kind="branch"${busy}>Branch</button></div>
    <label for="capture-branch">Branch</label>
           <input id="capture-branch" type="text" class="path" list="capture-branches" ${fieldTraits("identifier")}
             placeholder="a new branch, named after what you said" value="${esc(ui.branch || "")}"${busy} />
           <datalist id="capture-branches">${branches}</datalist>
    <button class="btn mini primary compose-manual" id="capture-route" type="button"${
      ui.busy || !ui.projectId ? " disabled" : ""
    }>Send it to the branch</button>
  </div>`;
}

/** Words, always. Whatever the router thought of, the keyboard answers it —
 *  and when the router has asked nothing there is nothing to answer, so the
 *  panel says which way out is open instead. */
function freeFormHtml(model, ui) {
  const shut = !model.awaitingAnswer || ui.busy;
  return `<div class="panel capture-freeform">
    <h3>${model.question ? "Or answer in your own words" : "Answer the router"}</h3>
    <textarea id="capture-answer" rows="3" ${fieldTraits("prose")} placeholder="Tell the router what you want"
      aria-label="Answer the router"${ui.busy ? " disabled" : ""}>${esc(ui.answer || "")}</textarea>
    <button class="btn primary" id="capture-answer-send" type="button"${shut ? " disabled" : ""}>Send</button>
    ${model.awaitingAnswer ? "" : '<div class="compose-note dim">The router has not asked anything about this one.</div>'}
  </div>`;
}

/** The whole page. `ui`: { projects, branches, projectId, kind, branch, answer,
 *  busy, error }. */
export function captureDecisionHtml(model, ui = {}) {
  return `<div class="board-head"><div><h1>What should happen with this?</h1>
      <p>${
        model.awaitingAnswer
          ? "The router needs one thing before it can send this anywhere."
          : "Where this goes is yours to say."
      }</p></div></div>
    <div class="capture-decide">
      ${saidHtml(model)}
      ${askHtml(model, ui)}
      ${manualHtml(model, ui)}
      ${freeFormHtml(model, ui)}
      <div class="capture-decide-foot">
        <button class="btn danger" id="capture-cancel" type="button"${ui.busy ? " disabled" : ""}>Cancel this capture</button>
        <span class="dim">Nothing is kept — the capture goes with it.</span>
      </div>
      <div class="warn capture-decide-error"${ui.error ? "" : " hidden"}>${esc(ui.error || "")}</div>
    </div>`;
}
