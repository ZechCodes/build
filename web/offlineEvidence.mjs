// What a browser check needs to prove a page really went offline, and that it
// came back by dialling a fresh session (#130).
//
// Playwright's `setOffline` refuses new connections but leaves an open
// WebSocket alive, so a page still holding its relay socket can signal a
// restart through it while "offline" — and reach a restarted bridge without
// the fresh dial a reconnect check is there to prove. So each page's sockets
// are tracked from before its scripts run, and the page counts as offline only
// once none is open with the network already off.

/** The init script: every WebSocket the page opens, and how many are still
 *  opening or open. Pass to `context.addInitScript` before any page loads. */
export function trackWebSockets() {
  const Native = globalThis.WebSocket;
  const sockets = new Set();
  globalThis.__openSockets = () => [...sockets].filter((socket) => socket.readyState <= Native.OPEN).length;
  globalThis.WebSocket = class TrackedWebSocket extends Native {
    constructor(...args) {
      super(...args);
      sockets.add(this);
      this.addEventListener("close", () => sockets.delete(this));
    }
  };
}

/** How many WebSockets each page holds opening or open right now. NaN for a
 *  page without the tracker, which never counts as none. */
export const openSocketCounts = (pages) =>
  Promise.all(pages.map((page) => page.evaluate(() => globalThis.__openSockets?.() ?? NaN)));

async function untilNoOpenSocket(pages, what, waitMs) {
  const started = Date.now();
  let counts = await openSocketCounts(pages);
  while (!counts.every((count) => count === 0)) {
    if (Date.now() - started >= waitMs) {
      throw new Error(`timed out after ${waitMs} ms waiting for ${what} (open sockets per page: ${counts.join(", ")})`);
    }
    await new Promise((done) => setTimeout(done, 250));
    counts = await openSocketCounts(pages);
  }
}

/**
 * Take the context offline with no page holding a socket.
 *
 * Checked twice: before, so the usual case goes offline clean; and again once
 * offline has applied, because a retry or a direct-pair timer can open one
 * between that sample and `setOffline`. What happens next (a bridge restart)
 * needs the precondition to hold at that moment, so a socket that slipped in
 * is waited out — offline, it can close but nothing new can open — and one
 * that stays open fails the check.
 *
 * `afterFirstSample` runs between the two, for a test to open one there.
 */
export async function goOfflineWithNoOpenSocket(context, pages, { waitMs = 60000, afterFirstSample = async () => {} } = {}) {
  await untilNoOpenSocket(pages, "every page to hold no open WebSocket", waitMs);
  await afterFirstSample();
  await context.setOffline(true);
  await untilNoOpenSocket(pages, "every page to hold no open WebSocket once offline", waitMs);
}

/**
 * Bring the context back online, and answer each page's clock as it stood
 * just before. Read first: a fresh dial can land the moment networking is
 * back, before anything here could ask a page the time.
 */
export async function goOnline(context, pages) {
  const cutoffs = await Promise.all(pages.map((page) => page.evaluate(() => Date.now())));
  await context.setOffline(false);
  return cutoffs;
}

/** The sessions this page's peer links first connected to `deviceId` at or
 *  after `since` — the `connected` / `initial` diagnostic core/peerLink.js
 *  records, on a connection named `<deviceId>:<sessionId>`. */
export const freshDials = (page, since, deviceId) => page.evaluate(({ after, device }) =>
  (globalThis.buildConnectionDiagnostics?.().events || []).filter((entry) =>
    entry.event === "connected" && entry.phase === "initial" && entry.at >= after
    && String(entry.connection).startsWith(`${device}:`)), { after: since, device: deviceId });
