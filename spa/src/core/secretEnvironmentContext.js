import { readCached, subscribeCache } from "./localCache.js";
import { readEnvironmentSupport } from "./environmentSupport.js";

/** Cached membership and support are the only inputs to rendering the scope. */
export function watchSecretEnvironment({ deviceId, projectId, onChange }) {
  let disposed = false, generation = 0;
  const read = async () => {
    const mine = ++generation;
    const [support, projects] = await Promise.all([readEnvironmentSupport(deviceId), readCached({ deviceId, entityId: "", kind: "projects" })]);
    if (disposed || mine !== generation) return;
    const id = typeof projectId === "function" ? projectId() : projectId;
    const project = projects?.value?.find(row => row.project_id === id);
    onChange(support.secrets && project?.environment_id ? project.environment_id : null);
  };
  const stop = subscribeCache({ deviceId }, address => {
    if (["projects", "environment-support"].includes(address.kind)) void read();
  });
  void read();
  return Object.freeze({ refresh: read, dispose() { disposed = true; generation += 1; stop(); } });
}
