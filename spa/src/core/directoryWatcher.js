// Mounted Files roots retain bounded invalidation subscriptions, including
// while folded. Their hint handler refreshes only currently visible readers.
import { bridgeCapabilities, onBridgeGreeted, watchChanges } from "./changeEvents.js";
import { lazyViewReadsEnabled } from "./scopedSyncRuntime.js";
import { applyDirectoryHint, chooseFilePushKind } from "./directoryPages.js";

const projectSource = (scope) => scope.project_id && scope.source_id && scope.source_path
  ? { kind: "project_source", project_id: scope.project_id, source_id: scope.source_id, source_path: scope.source_path } : null;

function watchScope(scope, deviceId) {
  const source = projectSource(scope);
  if (source && bridgeCapabilities(deviceId).changes?.projectSources) return { scope: source, kinds: [chooseFilePushKind(deviceId)] };
  const entity = scope.entity_id || scope.run_id || scope.worktree_id;
  if (entity) return { entity, kinds: [chooseFilePushKind(deviceId)] };
  // Secondary workspace directories have no filesystem producer. Preserve
  // the board wake fallback; never subscribe to a nonexistent workspace id.
  if (scope.workspace_id) return { entity: null, kinds: ["state"] };
  return { entity: scope.project_id || null, kinds: [chooseFilePushKind(deviceId)] };
}

export function createDirectoryWatcher({ deviceId, scopeOf, entityIdOf, active = () => true }) {
  let watcher = null;
  const invalidate = (files) => applyDirectoryHint({ deviceId, entityId: entityIdOf(), files, active });
  const refresh = () => invalidate({ paths: [], truncated: true });
  const onChanges = (items) => {
    const files = items.map((item) => item.files).filter(Boolean);
    return files.length ? Promise.all(files.map(invalidate)) : refresh();
  };
  const bind = () => {
    watcher?.dispose(); watcher = null;
    if (!active() || !lazyViewReadsEnabled(deviceId)) return;
    watcher = watchChanges({ deviceId, ...watchScope(scopeOf(), deviceId), onChanges, refresh, mode: "realtime" });
  };
  const stopGreet = onBridgeGreeted((changed) => { if (changed === deviceId) bind(); });
  bind();
  return { dispose() { stopGreet(); watcher?.dispose(); watcher = null; } };
}
