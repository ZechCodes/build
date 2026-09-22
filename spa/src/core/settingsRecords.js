import { mergeCached, readCached, subscribeCache, writeCached } from "./localCache.js";

export const deviceSettingsAddress = (deviceId = "") => ({ deviceId, entityId: "", kind: "settings" });
export const deviceModelsAddress = (deviceId = "") => ({ deviceId, entityId: "", kind: "models" });
export const projectSettingsAddress = (deviceId = "", projectId) => ({ deviceId, entityId: projectId, kind: "project-settings" });
export const deviceProjectsAddress = (deviceId = "") => ({ deviceId, entityId: "", kind: "device-projects" });
export const workspaceSettingsAddress = (deviceId = "", workspaceId) => ({ deviceId, entityId: workspaceId, kind: "workspace-settings" });
export const downloadsAddress = { deviceId: "", entityId: "", kind: "downloads" };

/** A project mutation answers its row. Keep the sheet's record and the device
 * project list coherent before either listener redraws. */
export async function writeProjectSetting(deviceId, project) {
  await writeCached(projectSettingsAddress(deviceId, project.project_id), project);
  await mergeCached(deviceProjectsAddress(deviceId), (projects) =>
    Array.isArray(projects)
      ? projects.some((item) => item.project_id === project.project_id)
        ? projects.map((item) => item.project_id === project.project_id ? project : item)
        : [...projects, project]
      : null);
}

export async function removeProjectSetting(deviceId, projectId) {
  await mergeCached(deviceProjectsAddress(deviceId), (projects) =>
    Array.isArray(projects) ? projects.filter((item) => item.project_id !== projectId) : null);
}

/** One addressable record. The callback sees only re-reads after cache writes,
 * including the first read at mount. A pull arriving after another writer's
 * announcement cannot replace that newer record. */
export function watchSettingsRecord(address, paint, { owner } = {}) {
  let active = true;
  let revision = 0;
  let reads = Promise.resolve();
  let observer;
  const reread = () => {
    reads = reads.then(async () => {
      const record = await readCached(address);
      if (active) paint(record?.value);
    });
    return reads;
  };
  const unsubscribe = subscribeCache(address, () => {
    revision += 1;
    void reread();
  });
  const dispose = () => {
    active = false;
    unsubscribe();
    observer?.disconnect();
  };
  if (owner && typeof MutationObserver !== "undefined") {
    observer = new MutationObserver(() => {
      if (!owner.isConnected) dispose();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
  }
  void reread();

  return {
    read: reread,
    write: async (value) => {
      if (!active) return;
      await writeCached(address, value);
      await reads;
    },
    pull: async (fetch) => {
      const began = revision;
      const value = await fetch();
      if (active && began === revision) await writeCached(address, value);
      await reads;
    },
    dispose,
  };
}
