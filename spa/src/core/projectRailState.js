// Explicit returns keep the project's last face; ordinary project links keep
// opening Tasks. This belongs to local UI state, separate from file replicas.
import { projectLayoutCacheId } from "./directoryScope.js";
import { uiAddress, watchUiState } from "./localUiState.js";
import { projectRoute } from "./projectModel.js";

const FACES = new Set(["tasks", "files", "workspaces"]);
const pendingWrites = new Map();
const addressOf = (route) => uiAddress({
  deviceId: route.deviceId, entityId: projectLayoutCacheId(route.projectId),
  view: "project", kind: "rail", sub: "return",
});
const keyOf = (route) => JSON.stringify([route.deviceId, route.projectId]);

function locationOf(route) {
  const tab = FACES.has(route.tab) ? route.tab : "tasks";
  if (tab === "tasks") return { tab, ...(route.view ? { view: route.view } : {}) };
  if (tab !== "files") return { tab };
  return {
    tab,
    ...(route.sourceId ? { sourceId: route.sourceId } : {}),
    ...(route.file ? { file: route.file } : {}),
    ...(route.file && Number.isInteger(route.line) && route.line > 0 ? { line: route.line } : {}),
  };
}

async function storeLocation(route, location) {
  const record = watchUiState(addressOf(route), () => {});
  try {
    await record.write(location);
    return true;
  } finally {
    record.dispose();
  }
}

export function rememberProjectRailRoute(route) {
  if (route?.name !== "project" || !route.deviceId || !route.projectId) return Promise.resolve(false);
  const key = keyOf(route);
  const location = locationOf(route);
  const write = (pendingWrites.get(key) || Promise.resolve())
    .then(() => storeLocation(route, location)).catch(() => false);
  pendingWrites.set(key, write);
  void write.then(() => { if (pendingWrites.get(key) === write) pendingWrites.delete(key); });
  return write;
}

export async function projectReturnRoute(project) {
  const route = projectRoute(project);
  if (!route.deviceId || !route.projectId) return route;
  await pendingWrites.get(keyOf(route));
  const record = watchUiState(addressOf(route), () => {});
  try {
    const saved = await record.ready;
    return FACES.has(saved?.tab) ? { ...route, ...locationOf(saved) } : route;
  } finally {
    record.dispose();
  }
}
