// Only cached workspace facts select the icon; RPC answers never repaint it.
import { ICON_LOCK, ICON_LOCK_OPEN } from "./icons.js";
import { contextFor } from "./deviceContexts.js";
import { notifyError } from "./notify.js";
const pending = new Set();
const keyOf = (state) => `${state.deviceId}/${state.workspaceId}`;
export function workspaceLockState(workspace, supported) {
  if (!workspace || !supported) return null;
  const state = { deviceId: workspace.deviceId, workspaceId: workspace.id, locked: workspace.locked === true };
  return { ...state, pending: pending.has(keyOf(state)) };
}
export function workspaceLockHtml(state) {
  if (!state) return "";
  const label = state.locked ? "Unlock workspace" : "Lock workspace";
  return `<button class="workspace-lock-toggle" type="button" data-workspace-lock aria-label="${label}" title="${label}" aria-pressed="${state.locked}"${state.pending ? " disabled" : ""}>${state.locked ? ICON_LOCK : ICON_LOCK_OPEN}</button>`;
}
async function toggle(state, changed) {
  const key = keyOf(state);
  if (pending.has(key)) return;
  pending.add(key);
  changed();
  try {
    const context = contextFor(state.deviceId);
    if (!context) throw new Error("Workspace unavailable");
    await context.rpc("workspace.set_locked", { workspace_id: state.workspaceId, locked: !state.locked });
  } catch (error) {
    notifyError(error?.code === "busy" ? "Try again in a moment" : error?.message || String(error));
  } finally {
    pending.delete(key);
    changed();
  }
}
export function wireWorkspaceLock(host, state, changed) {
  const button = host.querySelector("[data-workspace-lock]");
  if (button && state) button.onclick = () => void toggle(state, changed);
}
