// Each workspace's size on disk, asked for when the Workspaces tab opens
// (#273).
//
// The bridge measures a workspace's size only when it goes quiet, so a busy
// one has none. The size is only ever shown on that tab, so the tab asks the
// machine to measure its project's workspaces once each time it opens
// (`workspace.measure_sizes`). The bridge answers at once and walks them one
// at a time; each size lands on the workspace's row, the pushed list writes it
// to the cache, and the tab redraws from there. Nothing here holds a size.
//
// Whether a row with no size yet shows a placeholder is read from the cache
// (core/workspaceSizeSupport.js), never from here.

import { bridgeAdapter, bridgeCapabilities, onBridgeGreeted } from "./changeEvents.js";

/**
 * Ask one machine to measure one project's workspaces, once.
 *
 * A machine not greeted yet is asked when it greets; one whose greeting does
 * not name the verb is not asked at all. An ask that does not reach the
 * machine is made again at its next greeting. `stop` ends the wait: the tab
 * that asked has closed.
 */
export function askForWorkspaceSizes(deviceId, call, projectId) {
  let done = false;
  let stopListening = () => {};
  const finish = () => {
    done = true;
    stopListening();
  };
  const attempt = async () => {
    if (done || !bridgeAdapter(deviceId)) return;
    if (bridgeCapabilities(deviceId).workspaces?.measureSizes !== true) return finish();
    try {
      await call("workspace.measure_sizes", { project_id: projectId });
      finish();
    } catch {
      // Not reached; the next greeting asks again.
    }
  };
  stopListening = onBridgeGreeted((greeted) => {
    if (String(greeted) === String(deviceId)) void attempt();
  });
  void attempt();
  return { stop: finish };
}
