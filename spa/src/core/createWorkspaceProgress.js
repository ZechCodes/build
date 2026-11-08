import { modalDialogHtml, openModal } from "./modal.js";
import { esc, messageOf } from "./text.js";
import { deviceWriteOperations } from "./writeOperationRuntime.js";

const STEP_LABELS = { environment: "Creating environment", project: "Creating project", workspace: "Creating workspace" };
const UNKNOWN_STATES = new Set(["outcome_unknown", "result_expired"]);
const NOOP = () => {};

function operationResult(record) {
  return record?.confirmation?.result || record?.result || record?.ack?.result || {};
}

function operationError(record) {
  return record?.confirmation?.error || record?.error;
}

function resourcesOf(record) {
  const result = operationResult(record);
  const details = operationError(record)?.details || {};
  return { project: result.project || details.project || null,
    workspace: result.workspace || (result.project_id && result.id ? result : null) };
}

function progressSteps(record, phase, project) {
  const progress = record?.progress || [];
  const held = Array.isArray(progress) ? progress : progress.steps || [];
  const names = record?.request?.method === "workspace.create" ? ["workspace"] : ["project", "workspace"];
  return names.map(step => {
    const current = held.find(item => item.step === step);
    const state = current?.state || inferredStepState(step, phase, project, names[0]);
    return { step, label: STEP_LABELS[step], state, detail: current?.detail || "" };
  });
}

function inferredStepState(step, phase, project, firstStep) {
  if (phase === "done") return "done";
  if (phase === "checking") return "checking";
  if (phase !== "failed") return "waiting";
  if (project) return step === "project" ? "done" : "failed";
  return step === firstStep ? "failed" : "waiting";
}

function outcomePhase(record, workspace) {
  if (!record || record.unresolved || UNKNOWN_STATES.has(record.state)) return "checking";
  if (record.state === "failed" || workspace?.status === "failed") return "failed";
  if (record.state === "succeeded") return "done";
  return "creating";
}

function failureText(record, workspace) {
  const detail = operationError(record);
  const directoryError = workspace?.directories?.find(directory => directory.error)?.error;
  return messageOf(detail?.message || detail || directoryError || "Workspace creation failed.");
}

/** A write-operation record is the sole source of execution and outcome state.
 * A transport acknowledgment alone never means that provisioning has finished. */
export function createWorkspaceProgressModel(record) {
  const { project, workspace } = resourcesOf(record);
  const phase = outcomePhase(record, workspace);
  const steps = progressSteps(record, phase, project);
  const status = { done: "Done", failed: "Failed", checking: "Checking…", creating: "Creating…" }[phase];
  const error = phase === "failed" ? failureText(record, workspace) : "";
  return { record, phase, status, steps, project, workspace, error,
    message: outcomeMessage(phase, project, error), actions: outcomeActions(phase, project, workspace) };
}

function outcomeMessage(phase, project, error) {
  if (phase === "checking") return "Build lost the reply; checking what happened";
  if (phase !== "failed") return "";
  return project ? `Project ${project.name || project.id} was created. Its workspace was not: ${error}` : "Nothing was created.";
}

function outcomeActions(phase, project, workspace) {
  if (phase === "done") return workspace ? [["open", "Open workspace"]] : [];
  if (phase !== "failed") return [];
  return project
    ? [["retry", "Try the workspace again"], ["keep", "Keep it"], ["discard", "Discard this project"]]
    : [["back", "Back to the wizard"]];
}

function stepHtml({ step, label, state, detail }) {
  return `<li data-progress-step="${esc(step)}" data-state="${esc(state)}"><span>${esc(label)}</span>
    <span class="sub">${esc(state)}</span>${detail ? `<div class="sub create-progress-detail">${esc(detail)}</div>` : ""}</li>`;
}

function progressBodyHtml(model, targetSummary, actionError) {
  return `<h3 id="create-workspace-progress-title">${esc(model.status)}</h3>
    <p class="sub create-target-summary">${esc(targetSummary)}</p>
    <ol class="create-progress-steps" aria-label="Creation steps">${model.steps.map(stepHtml).join("")}</ol>
    <p role="status" aria-live="polite">${esc(model.message)}</p>
    ${model.error ? `<p class="warn create-error">${esc(model.error)}</p>` : ""}
    ${actionError ? `<p class="warn create-error" role="alert">${esc(actionError)}</p>` : ""}
    <div class="row create-row">${model.actions.map(([action, label]) => `<button class="btn" type="button" data-progress-action="${action}">${esc(label)}</button>`).join("")}
      <button class="btn" type="button" data-progress-action="close">Close</button></div>`;
}

/** Closing only detaches this view. The durable service continues reconciling
 * the retained operation, and opening the same ID recovers its cached state. */
export function openCreateWorkspaceProgress(options) {
  const { operationId, deviceId, targetSummary = "", autoOpenDelay = 900 } = options;
  const service = options.writeOperations || deviceWriteOperations(deviceId);
  const opener = document.activeElement;
  let detached = false, timer = null, actionError = "", acting = false;
  let record = service?.readOperations().find(row => row.operationId === operationId);
  let model = createWorkspaceProgressModel(record);
  let previousPhase = null;
  const modal = openModal({
    dialogHtml: modalDialogHtml(progressBodyHtml(model, targetSummary, actionError), { className: "modal-create create-workspace-sheet create-workspace-progress" }),
    scrimId: "create-workspace-progress-scrim", onClose: detachedView, initialFocus: ":not(*)",
  });
  modal.body.setAttribute("aria-labelledby", "create-workspace-progress-title");
  const unsubscribe = service?.subscribe(sync) || NOOP;
  paint();
  const ready = Promise.resolve(service?.get(operationId)).then(row => {
    if (detached) return;
    record = service?.readOperations().find(item => item.operationId === operationId) || row;
    paint();
  }).catch(() => {});
  return { close, ready };

  function detachedView() {
    if (detached) return;
    detached = true;
    clearTimeout(timer);
    unsubscribe();
    opener?.focus();
    options.onClose?.();
  }
  function close() {
    detachedView();
    return modal.close();
  }
  function sync() {
    if (detached) return;
    record = service.readOperations().find(row => row.operationId === operationId);
    paint();
  }
  function paint() {
    if (detached) return;
    model = createWorkspaceProgressModel(record);
    const terminalChange = model.phase !== previousPhase && ["done", "failed"].includes(model.phase);
    const focusedAction = modal.body.contains(document.activeElement) ? document.activeElement.dataset.progressAction : null;
    modal.body.innerHTML = progressBodyHtml(model, targetSummary, actionError);
    modal.body.querySelectorAll("[data-progress-action]").forEach(button => {
      button.disabled = acting;
      button.onclick = () => { void act(button.dataset.progressAction); };
    });
    restoreFocus(terminalChange, focusedAction);
    if (model.phase === "done" && previousPhase !== "done") scheduleOpen();
    previousPhase = model.phase;
  }
  function restoreFocus(terminalChange, focusedAction) {
    if (terminalChange && mayFocusOutcome()) modal.body.querySelector("[data-progress-action]")?.focus();
    else if (focusedAction) modal.body.querySelector(`[data-progress-action="${focusedAction}"]`)?.focus();
  }
  function mayFocusOutcome() {
    return previousPhase === null || document.activeElement === document.body || modal.body.contains(document.activeElement);
  }
  function scheduleOpen() {
    if (!autoOpenDelay || !model.workspace) return;
    timer = setTimeout(() => {
      const focused = document.activeElement;
      if (!detached && modal.body.contains(focused) && focused.dataset.progressAction === "open") void act("open");
    }, autoOpenDelay);
  }
  async function act(action) {
    if (acting || detached) return;
    if (["close", "keep"].includes(action)) return close();
    acting = true;
    actionError = "";
    const handlers = { open: options.onOpenWorkspace, retry: options.onRetryWorkspace, discard: options.onDiscardProject, back: options.onBack };
    const payload = { operationId, record, project: model.project, workspace: model.workspace };
    paint();
    try {
      await handlers[action]?.(payload);
      await close();
    } catch (error) {
      acting = false;
      actionError = messageOf(error);
      paint();
    }
  }
}
