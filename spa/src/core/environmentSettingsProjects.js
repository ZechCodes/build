// Settings needs project.list's chosen names and counts. Device journals own
// their lean directory rows independently and must never receive these pulls.
import { onBridgeGreeted } from "./changeEvents.js";
import { subscribeCache } from "./localCache.js";
import { messageOf } from "./text.js";
import { watchSettingsRecord } from "./settingsRecords.js";

export const environmentSettingsProjectsAddress = deviceId => ({ deviceId, entityId: "", kind: "environment-settings-projects" });

export function watchEnvironmentSettingsProjects({ deviceId, callRpc, active, paint, onState }) {
  const record = watchSettingsRecord(environmentSettingsProjectsAddress(deviceId), paint);
  let generation = 0;
  async function refresh() {
    if (!active()) return false;
    const request = ++generation;
    const current = () => active() && request === generation;
    onState({ pending: true, error: null });
    try {
      const accepted = await record.pull(async () => {
        const answer = await callRpc("project.list");
        if (!Array.isArray(answer?.projects)) throw new Error("The device returned no project list");
        return answer.projects;
      }, { accept: current });
      if (current()) onState({ pending: false, error: null });
      return accepted;
    } catch (error) {
      if (current()) onState({ pending: false, error: messageOf(error) });
      return false;
    }
  }
  const requestRefresh = () => { void refresh(); };
  const unsubscribeGreeting = onBridgeGreeted(id => { if (id === deviceId) requestRefresh(); });
  // A committed cursor is the journal wake's completed cache publication,
  // including task-only wakes that do not replace the project directory.
  const unsubscribeJournal = subscribeCache({ deviceId }, address => { if (address.kind === "sync-cursor") requestRefresh(); });
  return { ...record, refresh, dispose() {
    generation += 1;
    unsubscribeGreeting(); unsubscribeJournal(); record.dispose();
  } };
}
