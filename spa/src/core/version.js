// The frontend version watcher. The bundle carries the version it was built as
// (VITE_BUILD_VERSION, stamped by CI as the git SHA); the build also emits that
// version as static/version.json, so whatever the server currently serves is
// one no-store fetch away. A client that finds itself behind latches stale
// ONCE and offers a reload — it never nags and never reloads on its own.
//
// The check runs on an interval and on visibility resume. Resume is the
// load-bearing trigger: a suspended PWA is not connected while a deploy
// happens, so no push at server startup can reach it — waking up is the
// moment it can learn it is stale.

const CHECK_INTERVAL_MS = 10 * 60 * 1000;

/** The served version, read past every HTTP cache a PWA shell may hold. */
export async function fetchServedVersion(fetchImpl = fetch) {
  const response = await fetchImpl("/app/static/version.json", { cache: "no-store" });
  if (!response.ok) throw new Error(`version.json: ${response.status}`);
  const body = await response.json();
  return String(body.version || "");
}

export function createVersionWatcher({
  currentVersion,
  fetchVersion,
  onStale,
  intervalMs = CHECK_INTERVAL_MS,
  documentLike = document,
  setIntervalImpl = (fn, ms) => setInterval(fn, ms),
}) {
  let stale = false;

  const check = async () => {
    if (stale) return;
    let served;
    try {
      served = await fetchVersion();
    } catch {
      return; // offline or a mid-deploy blip: the next check answers
    }
    if (served && served !== currentVersion) {
      stale = true;
      onStale(served);
    }
  };

  const start = () => {
    // A dev build has no deploys to fall behind; watching would only flag the
    // gap between a dev server and production.
    if (!currentVersion || currentVersion === "dev") return;
    setIntervalImpl(check, intervalMs);
    documentLike.addEventListener("visibilitychange", () => {
      if (documentLike.visibilityState === "visible") return check();
      return undefined;
    });
  };

  return { check, start };
}
