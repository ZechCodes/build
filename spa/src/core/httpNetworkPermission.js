import { httpFailure } from "./httpEndpoint.js";

function localIPv4(host) {
  if (!/^\d+\.\d+\.\d+\.\d+$/.test(host)) return false;
  const [first, second] = host.split(".").map(Number);
  return first === 10 || (first === 192 && second === 168)
    || (first === 172 && second >= 16 && second <= 31) || (first === 169 && second === 254);
}

export function httpNetworkPermission(origin) {
  const host = new URL(origin).hostname.replace(/\.$/u, "");
  if (host === "localhost" || host.endsWith(".localhost") || /^127\./u.test(host) && !/[a-z]/iu.test(host)
    || host === "[::1]") return "loopback-network";
  if (host.endsWith(".local") || localIPv4(host) || /^\[(?:f[cd]|fe[89ab])/iu.test(host)) return "local-network";
  return null;
}

export async function checkHttpNetworkPermission(origin, permissions = navigator.permissions, callerOrigin = location.origin) {
  const name = httpNetworkPermission(origin);
  if (!name || !permissions?.query) return;
  const caller = httpNetworkPermission(callerOrigin);
  if (caller === name || caller === "loopback-network") return;
  const status = await permissions.query({ name }).catch(() => null);
  if (status?.state === "denied") throw httpFailure("network_permission_denied");
  if (status?.state === "prompt") throw httpFailure("network_permission");
}

// Start the browser's own permission check within the click. This request
// carries no challenge or credential and its application response is unused.
export function requestHttpNetworkPermission(origin, fetchImpl = fetch) {
  return fetchImpl(origin + "/.well-known/build-bridge", {
    method: "GET", credentials: "omit", redirect: "error", referrerPolicy: "no-referrer",
    cache: "no-store", signal: AbortSignal.timeout(5000),
  }).then((response) => response.body?.cancel()).catch(() => {});
}
