// The part of a Git source's card that keeps its base branch in step with its
// remote (#267): the setting, what the last sync concluded, and Sync now.
//
// Offered only on a machine whose greeting named `sources.syncBase`. The
// status line is read off the cached row; the bridge's service writes each
// sync onto the row and the project list push brings it here.

import { esc } from "../core/text.js";
import { syncStatusLine } from "../core/sourceSyncModel.js";

/** Whether Sync now was pressed and its sync has not landed on the row yet. */
const awaitingSync = (source, syncing) =>
  syncing.has(source.id) && syncing.get(source.id) === (source.sync?.last_attempt_ms ?? null);

export function syncControlsHtml(source, index, { syncsBase, syncing = new Map(), inFlight = new Set(), now = Date.now() }) {
  if (!syncsBase || source.is_git === false) return "";
  const id = `ps-syncbase-${index}`;
  const waiting = inFlight.has(source.id) || awaitingSync(source, syncing);
  const status = waiting ? "Syncing…" : syncStatusLine(source, now);
  return `<div class="ps-sync" data-source-sync>
      <label class="toggle ps-sync-toggle" for="${id}"><input type="checkbox" id="${id}" data-sync-base${source.sync_base ? " checked" : ""}>
        Keep the base branch up to date with the remote</label>
      <div class="dim ps-hint" data-sync-status role="status">${esc(status)}</div>
      <div class="row"><button class="btn mini" type="button" data-sync-now${waiting ? " disabled" : ""}>Sync now</button></div></div>`;
}

/** Wire one card's controls: the setting saves the moment it is changed, and
 *  Sync now asks the bridge to sync the source at once. */
export function mountSyncControls(card, source, { callRpc, projectId, write, syncInFlight, onSyncAccepted, onSyncSettled }) {
  const toggle = card.querySelector("[data-sync-base]");
  if (!toggle) return;
  const error = card.querySelector("[data-source-error]");
  const ids = { project_id: projectId, source_id: source.id };
  toggle.onchange = () => void write(
    () => callRpc("project.update_source", { ...ids, sync_base: toggle.checked }),
    toggle,
    error,
  );
  const syncNow = card.querySelector("[data-sync-now]");
  // Only an accepted write creates the pending marker. A greeting or ownership
  // refusal must leave the source idle and retryable through later repaints.
  syncNow.onclick = async () => {
    if (syncInFlight.has(source.id)) return;
    syncInFlight.add(source.id);
    try {
      await write(
        () => callRpc("project.sync_source", ids), syncNow, error,
        () => onSyncAccepted(source),
      );
    } finally {
      syncInFlight.delete(source.id);
      if (!syncNow.isConnected) onSyncSettled();
    }
  };
}
