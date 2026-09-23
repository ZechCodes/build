import { esc } from "./text.js";
import { modalDialogHtml, openModal } from "./modal.js";
import { readCached, subscribeCache, writeCached } from "./localCache.js";

const TARGET_LABEL = { workspace: "Workspace copy", source: "Original source", both: "Both" };

function availableTargets(options, results = []) {
  const workspace = targetAvailable(options.workspace);
  const source = targetAvailable(options.source);
  const failed = new Set(results.filter((result) => result.status === "failed").map((result) => result.target));
  const retryWorkspace = failed.has("workspace");
  const retrySource = failed.has("source");
  const both = bothAvailable(workspace, source, failed, retryWorkspace, retrySource);
  return [(workspace || retryWorkspace) && "workspace", (source || retrySource) && "source", both && "both"].filter(Boolean);
}

const targetAvailable = (option) => option?.available && (!option.is_git || option.needs_reconciliation);
const bothAvailable = (workspace, source, failed, retryWorkspace, retrySource) => workspace && source || failed.has("both") || retryWorkspace && retrySource;

function targetButton(target, selected, pending) {
  return `<button type="button" class="workspace-init-choice" data-init-target="${target}" aria-pressed="${target === selected}"${pending ? " disabled" : ""}>${TARGET_LABEL[target]}</button>`;
}

function outcomeMessage(result) {
  if (result.status === "failed") return `${TARGET_LABEL[result.target]}: ${result.error || "Git initialization failed"}`;
  if (result.status === "initialized") return `${TARGET_LABEL[result.target]} initialized.`;
  return result.status === "already_initialized" ? `${TARGET_LABEL[result.target]} was already initialized.` : "";
}

function choiceDescription(options) {
  const samePath = options.workspace?.path === options.source?.path;
  if (samePath) return "The workspace copy and original source resolve to this same folder. Both applies to that one folder.";
  return "Both creates two independent repositories. They do not share commit history.";
}

const optionPath = (option) => option?.path || "Unavailable";
const sourceSeparation = (samePath) => samePath ? " This is the same folder as the workspace path." : " This workspace copy stays separate.";

function unavailableMessage(options) {
  return [options.workspace, options.source]
    .filter((entry) => entry?.available === false && entry.reason)
    .map((entry) => entry.reason)
    .join(" · ");
}

function dialogContent(options, state) {
  const choices = availableTargets(options, state.results);
  const selected = choices.includes(state.target) ? state.target : choices[0];
  state.target = selected;
  const outcomes = state.results.map(outcomeMessage).filter(Boolean).join(" · ");
  const samePath = options.workspace?.path === options.source?.path;
  const unavailable = unavailableMessage(options);
  return `<h3>Initialize Git</h3>
    <p class="sub">Choose where Git should be initialized.</p>
    <div class="workspace-init-path workspace-init-copy"><strong>Workspace copy</strong><code>${esc(optionPath(options.workspace))}</code></div>
    <p class="workspace-init-detail">Adds Changes to this workspace. Your open file and edits stay in place.</p>
    <div class="workspace-init-path workspace-init-source"><strong>Original source</strong><code>${esc(optionPath(options.source))}</code></div>
    <p class="workspace-init-detail">Affects the source folder and future workspaces created from it.${sourceSeparation(samePath)}</p>
    <div class="workspace-init-choices" role="group" aria-label="Git initialization target">${choices.map((target) => targetButton(target, selected, state.pending)).join("")}</div>
    <p class="workspace-init-note">${esc(choiceDescription(options))}</p>
    <div class="workspace-init-error" data-init-error role="status">${esc(outcomes || unavailable || (!selected ? "No Git initialization is needed for these paths." : ""))}</div>
    <div class="row"><button class="btn" type="button" data-cancel-init-git>Cancel</button><button class="btn primary" type="button" data-confirm-init-git${state.pending || !selected ? " disabled" : ""}>${esc(state.retryLabel || "Initialize Git")}</button></div>`;
}

function retryTarget(results) {
  const failed = results.filter((result) => result.status === "failed").map((result) => result.target);
  if (failed.length === 1) return failed[0];
  if (failed.includes("workspace") && failed.includes("source")) return "both";
  return null;
}

async function submitInitialization(context) {
  const { state, options, modal, isActive, callRpc, workspaceId, sourceId, onUpdate, render } = context;
  if (state.pending) return;
  if (!requestBelongsToView(isActive)) {
    state.results = [{ target: state.target, status: "failed", error: "The active device changed. Reopen this dialog to continue." }];
    render();
    return;
  }
  state.pending = true;
  render();
  try {
    const answer = await callRpc("workspace.init_git", { workspace_id: workspaceId, source_id: sourceId, target: state.target });
    if (!requestBelongsToView(isActive)) return;
    state.results = resultList(answer);
    const cachedOptions = await onUpdate(answer);
    syncOptions(options, cachedOptions, sourceId);
    if (prepareRetry(state)) return;
    await modal.close();
  } catch (error) {
    if (!requestBelongsToView(isActive)) return;
    state.results = [{ target: state.target, status: "failed", error: errorMessage(error) }];
    state.retryLabel = "Retry";
  } finally {
    state.pending = false;
    if (isActive() && modal.body.isConnected) render();
  }
}

function syncOptions(options, cached, sourceId) {
  if (!cached || cached.source_id !== sourceId) return;
  options.workspace = { ...options.workspace, ...cached.workspace };
  options.source = { ...options.source, ...cached.source };
}

/** Whether this answer still belongs to the view that asked for it. The view's
 *  own `isActive` is the whole question: it already asks whether its machine is
 *  the one the route is about and can still answer (views/workspaceView.js). */
const requestBelongsToView = (isActive) => isActive();
const resultList = (answer) => answer.results || answer.outcomes || [];
const errorMessage = (error) => error.message || String(error);

function prepareRetry(state) {
  const retry = retryTarget(state.results);
  if (!retry) return false;
  state.target = retry;
  state.retryLabel = `Retry ${TARGET_LABEL[retry].toLowerCase()}`;
  return true;
}

function createDialogController({ options, callRpc, workspaceId, sourceId, isActive, onUpdate, onClosed }) {
  options = { ...options, workspace: { ...options.workspace }, source: { ...options.source } };
  const state = { target: availableTargets(options)[0], pending: false, results: [], retryLabel: "" };
  let modal;
  const render = () => {
    modal.body.innerHTML = dialogContent(options, state);
    modal.body.querySelectorAll("[data-init-target]").forEach((button) => {
      button.onclick = () => { state.target = button.dataset.initTarget; state.results = []; state.retryLabel = ""; render(); };
    });
    modal.body.querySelector("[data-cancel-init-git]").onclick = () => modal.close();
    const confirm = modal.body.querySelector("[data-confirm-init-git]");
    if (confirm) confirm.onclick = () => submitInitialization({ state, options, modal, isActive, callRpc, workspaceId, sourceId, onUpdate, render });
  };
  modal = openModal({ dialogHtml: modalDialogHtml("", { className: "modal-workspace-init" }), onClose: onClosed });
  modal.updateOptions = (next) => {
    options = { ...next, workspace: { ...next.workspace }, source: { ...next.source } };
    render();
  };
  render();
  return modal;
}

export function mountWorkspaceGitInitialization({ host, workspaceId, sourceId, callRpc, cacheScope, isActive, onUpdate }) {
  let dialog = null;
  let loading = false;
  let opening = false;
  let revision = 0;
  let latestRead = Promise.resolve();
  const address = cacheScope?.address({ entityId: workspaceId, kind: "git-init-options", sub: sourceId });
  const button = document.createElement("button");
  button.className = "workspace-init-open";
  button.type = "button";
  button.dataset.initGit = "";
  button.textContent = "Initialize Git…";
  host.appendChild(button);
  const status = document.createElement("span");
  status.className = "workspace-init-open-status";
  status.setAttribute("role", "status");
  host.appendChild(status);
  const paintFromCache = async () => {
    if (!address) return;
    const options = (await readCached(address))?.value;
    if (!options || !opening || !isActive()) return;
    if (dialog) dialog.updateOptions(options);
    else dialog = createDialogController({ options, callRpc, workspaceId, sourceId, isActive, onUpdate, onClosed: () => {
      dialog = null;
      opening = false;
      loading = false;
      button.disabled = false;
    } });
  };
  const unwatch = address ? subscribeCache(address, () => {
    revision += 1;
    latestRead = paintFromCache();
  }) : () => {};
  button.onclick = async () => {
    if (loading || dialog) return;
    loading = true;
    opening = true;
    button.disabled = true;
    try {
      await paintFromCache();
      const startedAt = revision;
      const pulled = await callRpc("workspace.git_init_options", { workspace_id: workspaceId, source_id: sourceId });
      if (!isActive()) return;
      if (!address || revision !== startedAt) return;
      await writeCached(address, pulled);
      await latestRead;
    } catch (error) {
      if (isActive() && !dialog) status.textContent = `Could not load Git options: ${errorMessage(error)}`;
    } finally {
      loading = false;
      if (isActive()) button.disabled = false;
    }
  };
  return { dispose: () => { opening = false; unwatch(); dialog?.close(); } };
}
