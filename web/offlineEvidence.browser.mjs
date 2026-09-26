// The reconnect check's two boundaries, in a real Chromium against a local
// WebSocket server — no stack needed (#130).
//
// Needs Playwright and Chromium, which the web image (Containerfile, installed
// --omit=dev) has neither of, so it is named out of `node --test`'s default
// discovery and run on its own, locally:
//
//   CHROMIUM_PATH=/usr/bin/chromium npm run test:browser

import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { chromium } from "playwright";
import { freshDials, goOfflineWithNoOpenSocket, goOnline, openSocketCounts, trackWebSockets } from "./offlineEvidence.mjs";

/** A page to load and a WebSocket endpoint that accepts and holds sockets. */
function localServer() {
  const server = createServer((request, response) => response.end("<p>page</p>"));
  server.on("upgrade", (request, socket) => {
    const accept = createHash("sha1")
      .update(`${request.headers["sec-websocket-key"]}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`).digest("base64");
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    socket.on("data", () => socket.end(Buffer.from([0x88, 0x00]))); // any frame from the page is its close
    socket.on("error", () => {});
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

let server;
let browser;
let origin;

before(async () => {
  server = await localServer();
  origin = `127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });
});

after(async () => {
  await browser?.close();
  server?.closeAllConnections();
  server?.close();
});

async function trackedPage() {
  const context = await browser.newContext();
  await context.addInitScript(trackWebSockets);
  const page = await context.newPage();
  await page.goto(`http://${origin}/`);
  return { context, page };
}

/** Open a socket from the page and wait until it is open; the page keeps it
 *  as `__held` so a case can close it later. */
const openSocket = (page) => page.evaluate((host) => new Promise((resolve, reject) => {
  // The test's own loopback server, which speaks no TLS.
  const socket = new WebSocket(`ws://${host}/`); // nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
  globalThis.__held = socket;
  socket.addEventListener("open", () => resolve(socket.readyState));
  socket.addEventListener("error", reject);
}), origin);

describe("going offline with no socket open", () => {
  it("does not go offline on the first sample alone when a socket opens before offline applies", async () => {
    const { context, page } = await trackedPage();
    try {
      await assert.rejects(
        goOfflineWithNoOpenSocket(context, [page], { waitMs: 1000, afterFirstSample: () => openSocket(page) }),
        /once offline \(open sockets per page: 1\)/,
      );
      assert.deepEqual(await openSocketCounts([page]), [1]); // still open, offline or not
    } finally {
      await context.close();
    }
  });

  it("waits out a socket that slipped in, once it closes while offline", async () => {
    const { context, page } = await trackedPage();
    try {
      await goOfflineWithNoOpenSocket(context, [page], {
        waitMs: 5000,
        afterFirstSample: async () => {
          await openSocket(page);
          await page.evaluate(() => setTimeout(() => globalThis.__held.close(), 300));
        },
      });
      assert.deepEqual(await openSocketCounts([page]), [0]);
      assert.equal(await page.evaluate(() => navigator.onLine), false);
    } finally {
      await context.close();
    }
  });
});

describe("the evidence of a fresh dial", () => {
  /** A page whose diagnostics record a dial for two machines the instant the
   *  network is back — faster than anything outside the page can ask it. */
  async function dialsOnOnline() {
    const tracked = await trackedPage();
    await tracked.page.evaluate(() => {
      const events = [];
      globalThis.buildConnectionDiagnostics = () => ({ since: 0, dropped: 0, events: [...events] });
      addEventListener("online", () => {
        for (const connection of ["dev-seed:sess-fresh", "dev-other:sess-fresh"])
          events.push({ at: Date.now(), connection, event: "connected", phase: "initial" });
      });
    });
    await tracked.context.setOffline(true);
    return tracked;
  }

  it("counts a dial that lands the moment networking is back, for the seeded machine only", async () => {
    const { context, page } = await dialsOnOnline();
    try {
      const [cutoff] = await goOnline(context, [page]);
      await page.waitForFunction(() => globalThis.buildConnectionDiagnostics().events.length === 2);
      const dials = await freshDials(page, cutoff, "dev-seed");
      assert.deepEqual(dials.map((entry) => entry.connection), ["dev-seed:sess-fresh"]);
    } finally {
      await context.close();
    }
  });

  // The ordering goOnline replaces: a cutoff read after the network is back
  // can postdate the dial it is meant to admit.
  it("would miss that dial with a cutoff read after networking is back", async () => {
    const { context, page } = await dialsOnOnline();
    try {
      await context.setOffline(false);
      await page.waitForFunction(() => globalThis.buildConnectionDiagnostics().events.length === 2);
      await new Promise((done) => setTimeout(done, 20));
      const late = await page.evaluate(() => Date.now());
      assert.deepEqual(await freshDials(page, late, "dev-seed"), []);
    } finally {
      await context.close();
    }
  });
});
