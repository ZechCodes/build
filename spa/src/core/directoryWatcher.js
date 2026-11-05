// A modern hint can stay subscribed while folded: it carries no listing.
// A scoped legacy files body belongs only to a visible root.
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

export function createDirectoryWatcher({ deviceId, scopeOf, entityIdOf, active = () => true, visible = () => true, onVisible = () => {} }) {
  let watcher = null, signature = null, generation = 0, disposed = false;
  let bindingCurrent = () => false;
  const shown = () => visible() && !globalThis.document?.hidden;
  let wasShown = shown();
  const retire = () => {
    generation += 1;
    watcher?.dispose(); watcher = null; signature = null;
  };
  const wantsSubscription = () => !disposed && active() && lazyViewReadsEnabled(deviceId)
    && (chooseFilePushKind(deviceId) === "files.hints" || shown());
  const subscribe = (spec) => {
    const binding = generation;
    bindingCurrent = () => !disposed && active() && binding === generation;
    const current = bindingCurrent;
    const invalidate = (files) => applyDirectoryHint({ deviceId, entityId: entityIdOf(), files, active: current });
    const refresh = () => invalidate({ paths: [], truncated: true });
    const onChanges = (items) => {
      const files = items.map((item) => item.files).filter(Boolean);
      return files.length ? Promise.all(files.map(invalidate)) : refresh();
    };
    watcher = watchChanges({ deviceId, ...spec, onChanges, refresh, mode: "realtime",
      pausesWhileHidden: !spec.kinds.includes("files.hints"),
    });
  };
  const bind = (force = false) => {
    const spec = wantsSubscription() ? watchScope(scopeOf(), deviceId) : null;
    const next = spec ? JSON.stringify(spec) : null;
    if (!force && next === signature) return;
    retire(); signature = next;
    if (spec) subscribe(spec);
  };
  const visibilityChanged = () => {
    const next = shown(), returned = next && !wasShown;
    wasShown = next;
    bind();
    if (returned && lazyViewReadsEnabled(deviceId) && active()) void onVisible(bindingCurrent);
  };
  const stopGreet = onBridgeGreeted((changed) => { if (changed === deviceId) bind(true); });
  globalThis.document?.addEventListener("visibilitychange", visibilityChanged);
  bind();
  return {
    setVisible: visibilityChanged,
    dispose() {
      disposed = true; retire(); stopGreet();
      globalThis.document?.removeEventListener("visibilitychange", visibilityChanged);
    },
  };
}
