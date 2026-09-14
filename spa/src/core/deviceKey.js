// The account-wide name of a project.
//
// The bridge mints project ids from a per-machine counter, so every device's
// first project is `proj-1`: the bare id names a project only once you also say
// which device it is on. That pair is written as one string here and nowhere
// else, so every key, fold, selector and lookup that spans devices agrees.
// Wire fields are never rewritten — the bridge still wants the bare id.

export const deviceKey = (deviceId, projectId) => `${deviceId}/${projectId}`;

/** The two halves back, split on the FIRST "/" — api device ids are uuids and
 *  bridge project ids are `proj-<n>`, so neither half can hold a slash. Answers
 *  null when the key is not a string or either half is empty. */
export function splitDeviceKey(key) {
  const seam = typeof key === "string" ? key.indexOf("/") : -1;
  if (seam < 1) return null; // not a string, no seam, or an empty device id
  const projectId = key.slice(seam + 1);
  return projectId ? { deviceId: key.slice(0, seam), projectId } : null;
}

/** The account-wide name of the project a route stands in, or null while the
 *  route names no machine or no project — a capture's does neither. A route
 *  carries the bare id one bridge minted, so its device is half the name. */
export const routeProjectKey = (route) =>
  route && route.deviceId && route.projectId ? deviceKey(route.deviceId, route.projectId) : null;
