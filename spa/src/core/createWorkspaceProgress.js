import { creationMethodForRecovery, creationSupportForMethod } from "./writeOperationProgress.js";
import { environmentSupportAddress, readEnvironmentSupport } from "./environmentSupport.js";
import { subscribeCache } from "./localCache.js";
import { rowIdentityBadgeHtml } from "./environmentIdentity.js";
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
  const landed = record?.draft?.landed || {};
  const resource = name => result[name] || details[name] || landed[name] || null;
  return { environment: resource("environment"), project: resource("project"),
    workspace: result.workspace || bareWorkspace(result) };
}

function bareWorkspace(result) {
  return result.project_id && (result.workspace_id || result.id) ? result : null;
}

function progressSteps(record, phase, resources) {
  const progress = record?.progress || [];
  const held = Array.isArray(progress) ? progress : progress.steps || [];
  const names = creationStepNames(record?.request?.method);
  return names.map(step => {
    const current = held.find(item => item.step === step);
    const state = stepState(current, step, phase, resources, names);
    return { step, label: STEP_LABELS[step], state, detail: current?.detail || "" };
  });
}

function creationStepNames(method) {
  if (method === "workspace.create") return ["workspace"];
  return method === "environment.create" ? ["environment", "project", "workspace"] : ["project", "workspace"];
}
function stepState(current, step, phase, resources, names) {
  if (phase === "done") return "done";
  if (phase === "checking") return "checking";
  if (phase !== "failed") return current?.state || "waiting";
  const landed = resources.project ? names.indexOf("project") : resources.environment ? names.indexOf("environment") : -1;
  const index = names.indexOf(step);
  if (index <= landed) return "done";
  return index === landed + 1 ? "failed" : "waiting";
}

function outcomePhase(record) {
  if (record?.state === "failed") return knownFailure(record) ? "failed" : "checking";
  if (record?.state === "succeeded") return "done";
  if (needsChecking(record)) return "checking";
  return "creating";
}

function needsChecking(record) {
  return !record || record.checking || record.unresolved || UNKNOWN_STATES.has(record.state)
    || ["unknown", "reconciling"].includes(record.creation?.state);
}

function knownFailure(record) {
  const effect = record.effect || record.confirmation?.effect;
  return ["partial", "unapplied"].includes(effect);
}

function failureText(record, workspace) {
  const detail = operationError(record);
  const directoryError = workspace?.directories?.find(directory => directory.error)?.error;
  return messageOf(detail?.message || detail || directoryError || "Workspace creation failed.");
}

/** A write-operation record is the sole source of execution and outcome state.
 * A transport acknowledgment alone never means that provisioning has finished. */
export function createWorkspaceProgressModel(record, support = {}) {
  const resources = resourcesOf(record);
  const { environment, project, workspace } = resources;
  const phase = outcomePhase(record);
  const steps = progressSteps(record, phase, resources);
  const status = { done: "Done", failed: "Failed", checking: "Checking…", creating: "Creating…" }[phase];
  const error = phase === "failed" ? failureText(record, workspace) : "";
  return { record, phase, status, steps, environment, project, workspace, error,
    message: outcomeMessage(phase, resources, error), ...recoveryActions(record,outcomeActions(phase, resources),support) };
}

function recoveryActions(record, candidates, support) {
  // A pending cache read is not an unsupported device. Keep the action area neutral.
  if (support === null) return {actions:[],unavailable:""};
  const actions=candidates.filter(([action])=>{
    const method=creationMethodForRecovery(action,record?.request?.method);
    return !method || creationSupportForMethod(support,method);
  });
  const unavailable=actions.length===candidates.length ? "" : "Update Build on this device to use the remaining recovery actions. You can close this and return later.";
  return {actions,unavailable};
}
function outcomeMessage(phase, {environment, project}, error) {
  if (phase === "checking") return "Build lost the reply; checking what happened";
  if (phase !== "failed") return "";
  if (!project && environment) return `Environment ${environment.name || environment.environment_id} was created with no projects yet: ${error}`;
  if (!project) return `Nothing was created: ${error}`;
  const prefix = environment ? `Environment ${environment.name || environment.environment_id} was created. ` : "";
  return `${prefix}Project ${project.name || project.id} was created. Its workspace was not: ${error}`;
}

function outcomeActions(phase, {environment, project, workspace}) {
  if (phase === "done") return workspace ? [["open", "Open workspace"]] : [];
  if (phase !== "failed") return [];
  if (!project && environment) return [["resume", "Resume"], ["keepEnvironment", "Keep the environment"], ["discardEnvironment", "Discard the environment"]];
  return project
    ? [["retry", "Try the workspace again"], ["keep", "Keep it"], ["discard", "Discard this project"]]
    : [["back", "Back to the wizard"]];
}

function stepHtml({ step, label, state, detail }) {
  return `<li data-progress-step="${esc(step)}" data-state="${esc(state)}"><span>${esc(label)}</span>
    <span class="sub">${esc(state)}</span>${detail ? `<div class="sub create-progress-detail">${esc(detail)}</div>` : ""}</li>`;
}

function targetSummaryHtml(target, fallback, environment) {
  if (!target) return esc(fallback);
  const identity = target.kind === "new-environment" && environment
    ? { ...target, environment_id: environment.environment_id || environment.id } : target;
  const glyph = rowIdentityBadgeHtml(identity, target.kind === "project" ? "project" : "environment");
  return `<span class="create-kind" aria-hidden="true">${glyph}</span><span>${esc(target.summaryLabel || target.label || target.name || target.id)}</span>
    ${target.deviceLabel ? `<span class="dim inbox-device">${esc(target.deviceLabel)}</span>` : ""}
    ${target.offline ? '<span class="dim inbox-away">Offline</span>' : ""}`;
}

function targetOutcomeHtml(target) {
  if (!target) return "";
  const text = target.kind === "project" ? `Uses ${target.name}'s task board · ${target.taskText || "0 tasks"}` : "Creates a new task board";
  return `<p class="sub create-target-outcome">${esc(text)}</p>`;
}

function trapProgressTab(event, body) {
  if (event.key !== "Tab") return;
  const controls = [...body.querySelectorAll("button:not(:disabled)")];
  const edge = event.shiftKey ? controls[0] : controls.at(-1);
  if (document.activeElement !== edge) return;
  event.preventDefault();
  (event.shiftKey ? controls.at(-1) : controls[0])?.focus();
}

function progressBodyHtml(model, targetSummary, actionError, target) {
  return `<h3 id="create-workspace-progress-title" aria-live="polite">${esc(model.status)}</h3>
    <p class="create-target-summary">${targetSummaryHtml(target, targetSummary, model.environment)}</p>${targetOutcomeHtml(target)}
    <ol class="create-progress-steps" aria-label="Creation steps">${model.steps.map(stepHtml).join("")}</ol>
    <p role="status" aria-live="polite">${esc(model.message)}</p>
    ${model.error ? `<p class="warn create-error">${esc(model.error)}</p>` : ""}
    ${model.unavailable ? `<p class="sub" role="status">${esc(model.unavailable)}</p>` : ""}
    ${actionError ? `<p class="warn create-error" role="alert">${esc(actionError)}</p>` : ""}
    <div class="row create-row">${model.actions.map(([action, label]) => `<button class="btn" type="button" data-progress-action="${action}">${esc(label)}</button>`).join("")}
      <button class="btn" type="button" data-progress-action="close">Close</button></div>`;
}

function watchProgressInteraction(onInteraction) {
  const events = ["focusin", "focusout", "pointerdown", "keydown", "click"];
  events.forEach(type => document.addEventListener(type, onInteraction, true));
  return () => events.forEach(type => document.removeEventListener(type, onInteraction, true));
}

/** Closing only detaches this view. The durable service continues reconciling
 * the retained operation, and opening the same ID recovers its cached state. */
export function openCreateWorkspaceProgress(options) {
  const { operationId, deviceId, targetSummary = "", autoOpenDelay = 900 } = options;
  const service = options.writeOperations || deviceWriteOperations(deviceId);
  const opener = document.activeElement;
  let detached = false, timer = null, actionError = "", acting = false, supportGeneration = 0;
  let support = options.environmentSupport || null;
  let record = service?.readOperations().find(row => row.operationId === operationId);
  let model = createWorkspaceProgressModel(record,support);
  let previousPhase = null, userTookOver = false, painting = false;
  const modal = openModal({
    dialogHtml: modalDialogHtml(progressBodyHtml(model, targetSummary, actionError, options.target), { className: "modal-create create-workspace-sheet create-workspace-progress" }),
    scrimId: "create-workspace-progress-scrim", onClose: detachedView, initialFocus: ":not(*)",
  });
  modal.body.setAttribute("aria-labelledby", "create-workspace-progress-title");
  modal.body.onkeydown = event => trapProgressTab(event, modal.body);
  const unsubscribe = service?.subscribe(sync) || NOOP;
  const stopSupport = options.environmentSupport ? NOOP : subscribeCache(environmentSupportAddress(deviceId),()=>{void refreshSupport();});
  const stopInteraction = watchProgressInteraction(() => {
    if (support === null && !painting) userTookOver = true;
  });
  paint();
  const ready = Promise.all([Promise.resolve(service?.get(operationId)),refreshSupport()]).then(([row]) => {
    if (detached) return;
    record = service?.readOperations().find(item => item.operationId === operationId) || row;
    paint();
  }).catch(() => {});
  return { close, ready };

  function detachedView() {
    if (detached) return;
    detached = true;
    clearTimeout(timer);
    unsubscribe();stopSupport();stopInteraction();
    if (!document.activeElement?.closest('[role="dialog"]') || modal.body.contains(document.activeElement)) opener?.focus();
    options.onClose?.();
  }
  function close() {
    detachedView();
    return modal.close();
  }
  async function refreshSupport() {
    if(options.environmentSupport)return;
    const requested=++supportGeneration;
    const next=await readEnvironmentSupport(deviceId);
    if(detached || requested!==supportGeneration)return;
    support=next;paint();
  }
  function sync() {
    if (detached) return;
    record = service.readOperations().find(row => row.operationId === operationId);
    paint();
  }
  function paint() {
    if (detached) return;
    painting = true;
    model = createWorkspaceProgressModel(record,support);
    const terminalChange = model.phase !== previousPhase && ["done", "failed"].includes(model.phase);
    const focusedAction = modal.body.contains(document.activeElement) ? document.activeElement.dataset.progressAction : null;
    modal.body.innerHTML = progressBodyHtml(model, targetSummary, actionError, options.target);
    modal.body.querySelectorAll("[data-progress-action]").forEach(button => {
      button.disabled = acting;
      button.onclick = () => { void act(button.dataset.progressAction); };
    });
    restoreFocus(terminalChange, focusedAction);
    if (support !== null && model.phase === "done" && previousPhase !== "done") scheduleOpen();
    previousPhase = support === null ? null : model.phase;
    painting = false;
  }
  function restoreFocus(terminalChange, focusedAction) {
    if (!userTookOver && (previousPhase === null || (terminalChange && mayFocusOutcome()))) modal.body.querySelector("[data-progress-action]")?.focus();
    else if (focusedAction) modal.body.querySelector(`[data-progress-action="${focusedAction}"]`)?.focus();
  }
  function mayFocusOutcome() {
    return previousPhase === null || document.activeElement === document.body || modal.body.contains(document.activeElement);
  }
  function scheduleOpen() {
    if (!autoOpenDelay || !model.workspace || userTookOver) return;
    timer = setTimeout(() => {
      const focused = document.activeElement;
      if (!detached && modal.body.contains(focused) && focused.dataset.progressAction === "open") void act("open");
    }, autoOpenDelay);
  }
  async function act(action) {
    if (acting || detached) return;
    if (action === "close") return close();
    acting = true;
    actionError = "";
    const handlers = { open: options.onOpenWorkspace, retry: options.onRetryWorkspace, discard: options.onDiscardProject, keep: options.onKeepProject, back: options.onBack, resume: options.onResumeEnvironment, keepEnvironment: options.onKeepEnvironment, discardEnvironment: options.onDiscardEnvironment };
    const payload = { operationId, record, environment: model.environment, project: model.project, workspace: model.workspace };
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
