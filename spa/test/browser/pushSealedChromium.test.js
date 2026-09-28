// #200 in a real Chromium: the service worker opens sealed pushes with
// Chromium's own WebCrypto and IndexedDB, and shows what they say.
//
// 1. The fixture (fixtures/push/sealed-v1.json) opened by the real public/sw.js
//    code, run in a page against Chromium's WebCrypto, with the recipient key
//    imported non-extractable into IndexedDB.
// 2. End to end: the real sw.js registered at /app/sw.js (scope /app/), a real
//    push subscription, the notification key made by the real src/pushKeys.js,
//    a blob sealed to it, a push delivered over CDP
//    (`ServiceWorker.deliverPushMessage`), and the notification read back with
//    `registration.getNotifications()`. Then the same blob again: a replay,
//    shown with the generic copy.
//
// Both write what Chromium showed to a proof log (BUILD_PUSH_PROOF_DIR, default
// $TMPDIR/build-push-proof): the decrypted title and body, never a key.

import { constants } from "node:fs";
import { access, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";
import { afterAll, beforeAll, expect, it } from "vitest";

const spaRoot = fileURLToPath(new URL("../../", import.meta.url));
const fixturePath = fileURLToPath(new URL("../../../fixtures/push/sealed-v1.json", import.meta.url));
const proofDir = process.env.BUILD_PUSH_PROOF_DIR || join(tmpdir(), "build-push-proof");
const proofLog = join(proofDir, "sealed-push-chromium.log");

// What the server hands out: the real worker and the real key module, and the
// test sealer (a bridge's part) beside them. Nothing else.
const ROUTES = {
  "/app/sw.js": [join(spaRoot, "public/sw.js"), "text/javascript"],
  "/app/pushKeys.js": [join(spaRoot, "src/pushKeys.js"), "text/javascript"],
  "/app/seal.js": [join(spaRoot, "test/pushSealFixture.js"), "text/javascript"],
  "/app/fixture.json": [fixturePath, "application/json"],
};
const PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>Build push proof</title></head>
<body><pre id="shown"></pre></body></html>`;

async function chromiumExecutable() {
  const configured = process.env.CHROMIUM_PATH;
  if (configured) {
    await access(configured, constants.X_OK);
    return configured;
  }
  for (const directory of (process.env.PATH || "").split(delimiter)) {
    for (const name of ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"]) {
      try {
        await access(join(directory, name), constants.X_OK);
        return join(directory, name);
      } catch { /* the next one */ }
    }
  }
  throw new Error("the push proof needs Chromium on PATH (or CHROMIUM_PATH)");
}

let server;
let origin;
let browser;
const proof = [];

beforeAll(async () => {
  server = createServer(async (request, response) => {
    const path = new URL(request.url, "http://x").pathname;
    const route = ROUTES[path];
    if (route) {
      response.writeHead(200, { "Content-Type": route[1], "Service-Worker-Allowed": "/app/" });
      response.end(await readFile(route[0]));
      return;
    }
    if (path === "/app/" || path === "/app/index.html") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end(PAGE);
      return;
    }
    response.writeHead(404);
    response.end();
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  origin = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({ executablePath: await chromiumExecutable(), headless: true, args: ["--no-sandbox"] });
  await mkdir(proofDir, { recursive: true });
}, 60_000);

afterAll(async () => {
  await browser?.close();
  await new Promise((done) => (server ? server.close(done) : done()));
  if (proof.length) await writeFile(proofLog, `${proof.join("\n")}\n`);
});

/** Load the key module and the sealer into the page. Vitest rewrites dynamic
 *  imports inside test callbacks, so page.evaluate reads them off window. */
async function loadPushModules(page) {
  await page.addScriptTag({ type: "module", content: `
    import * as keys from "/app/pushKeys.js";
    import * as seal from "/app/seal.js";
    window.__push = { keys, seal };
  ` });
  await page.waitForFunction(() => window.__push);
}

function record(line) {
  proof.push(`${new Date().toISOString()} ${line}`);
}

it("opens the fixture with Chromium's WebCrypto and shows its title and body", async () => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(`${origin}/app/`);
  await loadPushModules(page);
  const shown = await page.evaluate(async () => {
    const fixture = await (await fetch("/app/fixture.json")).json();
    const swSource = await (await fetch("/app/sw.js")).text();
    const { b64uDecode } = window.__push.seal;
    const { keys } = window.__push;
    // The app lays the database out; the fixture's key goes in non-extractable.
    await keys.storedKeyIds();
    const privateKey = await crypto.subtle.importKey(
      "jwk", fixture.recipient_private_jwk, { name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"]);
    const db = await new Promise((resolve) => {
      const request = indexedDB.open("build-push");
      request.onsuccess = () => resolve(request.result);
    });
    await new Promise((resolve) => {
      const tx = db.transaction("keys", "readwrite");
      tx.objectStore("keys").put({
        sid: fixture.subscription_id, privateKey, publicKey: b64uDecode(fixture.recipient_public_key),
      });
      tx.oncomplete = resolve;
    });
    db.close();
    // The worker's own code, in this page: Chromium's crypto and IndexedDB,
    // the fixture's subscription, and the clock near the fixture's iat.
    const iat = JSON.parse(fixture.plaintext).iat;
    const realNow = Date.now;
    Date.now = () => iat * 1000 + 5000;
    const notifications = [];
    const handlers = {};
    const self = {
      addEventListener: (type, handler) => { handlers[type] = handler; },
      crypto, indexedDB,
      registration: {
        showNotification: async (title, options) => { notifications.push({ title, ...options }); },
        pushManager: { getSubscription: async () => ({ endpoint: fixture.endpoint }) },
      },
      clients: {},
    };
    new Function("self", swSource)(self); // eslint-disable-line no-new-func
    const payload = { task_id: fixture.entity_id, kind: fixture.kind, url: "/app/#/generic", sealed: fixture.blob };
    const deliver = async (sent) => {
      const waits = [];
      handlers.push({ data: { json: () => sent }, waitUntil: (p) => waits.push(p) });
      await Promise.all(waits);
    };
    await deliver(payload);
    await deliver({ ...payload, task_id: "run-other" }); // AAD tampered: the generic copy
    Date.now = realNow;
    document.querySelector("#shown").textContent = JSON.stringify(notifications, null, 2);
    return { notifications, expected: JSON.parse(fixture.plaintext), userAgent: navigator.userAgent };
  });
  record(`fixture ${shown.userAgent}`);
  record(`fixture opened: title=${JSON.stringify(shown.notifications[0].title)} body=${JSON.stringify(shown.notifications[0].body)} url=${shown.notifications[0].data.url}`);
  record(`fixture with a tampered entity id: title=${JSON.stringify(shown.notifications[1].title)} body=${JSON.stringify(shown.notifications[1].body)}`);
  if (process.env.BUILD_PUSH_PROOF_DIR) await page.screenshot({ path: join(proofDir, "sealed-push-fixture.png") });
  expect(shown.notifications[0]).toMatchObject({
    title: shown.expected.title, body: shown.expected.body, data: { url: shown.expected.url },
  });
  expect(shown.notifications[1]).toMatchObject({ title: "Build", body: "An agent needs you" });
  await context.close();
}, 60_000);

it("shows a sealed push the real worker received, end to end, and a replay generically", async (test) => {
  // Push is refused in an incognito profile, which is what newContext() is.
  const profile = await mkdtemp(join(tmpdir(), "build-push-profile-"));
  const context = await chromium.launchPersistentContext(profile, {
    executablePath: await chromiumExecutable(), headless: true, args: ["--no-sandbox"],
  });
  test.onTestFinished(() => rm(profile, { recursive: true, force: true }));
  await context.grantPermissions(["notifications"], { origin });
  const page = await context.newPage();
  await page.goto(`${origin}/app/`);
  const cdp = await context.newCDPSession(page);
  const registrations = [];
  cdp.on("ServiceWorker.workerRegistrationUpdated", ({ registrations: updated }) => registrations.push(...updated));
  await cdp.send("ServiceWorker.enable");

  const subscribed = await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.register("/app/sw.js", { scope: "/app/" });
    await navigator.serviceWorker.ready;
    // Any valid P-256 point is an application server key to the push service.
    const pair = await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
    const serverKey = new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey));
    try {
      const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: serverKey });
      return { endpointHost: new URL(subscription.endpoint).host };
    } catch (error) {
      return { errorName: error.name, error: `${error.name}: ${error.message}` };
    }
  });
  if (subscribed.error) {
    // A real subscription needs the browser's push service over the network:
    // only an unreachable one (AbortError) skips. Anything else, a refused
    // permission grant included, is this test's setup breaking and fails it.
    await context.close();
    expect(subscribed.errorName, `push subscription failed: ${subscribed.error}`).toBe("AbortError");
    record(`end to end skipped: push service unreachable (${subscribed.error})`);
    test.skip();
    return;
  }

  await loadPushModules(page);
  const sealed = await page.evaluate(async () => {
    const { keys } = window.__push;
    const { b64uDecode, sealForTest } = window.__push.seal;
    const registration = await navigator.serviceWorker.getRegistration("/app/");
    const subscription = await registration.pushManager.getSubscription();
    const sid = await keys.subscriptionIdOf(subscription.endpoint);
    const key = await keys.ensurePushKey(sid);
    const stored = await keys.readPushKey(sid);
    const message = {
      v: 1,
      title: "Banner fade fixer",
      body: "Fixed on build/banner-fade, ready for review — ünïcode ✓",
      url: "/app/#/device/dev-1/project/proj-1/workspace/ws-1/changes?agent=agent-1",
      iat: Math.floor(Date.now() / 1000),
    };
    const blob = await sealForTest({
      recipientPublic: b64uDecode(key.publicKey), sid, kind: "agent", entityId: "run-e2e", message,
    });
    return {
      payload: { task_id: "run-e2e", kind: "agent", url: "/app/#/generic", sealed: blob },
      message,
      extractable: stored.privateKey.extractable,
    };
  });
  expect(sealed.extractable).toBe(false);

  await expect.poll(() => registrations.find((entry) => entry.scopeURL === `${origin}/app/` && !entry.isDeleted),
    { timeout: 10_000 }).toBeTruthy();
  const { registrationId } = registrations.find((entry) => entry.scopeURL === `${origin}/app/` && !entry.isDeleted);
  const shownNotifications = () => page.evaluate(async () => {
    const registration = await navigator.serviceWorker.getRegistration("/app/");
    return (await registration.getNotifications()).map((n) => ({ title: n.title, body: n.body, tag: n.tag, url: n.data?.url }));
  });

  await cdp.send("ServiceWorker.deliverPushMessage", { origin, registrationId, data: JSON.stringify(sealed.payload) });
  await expect.poll(async () => (await shownNotifications())[0]?.title, { timeout: 15_000 }).toBe(sealed.message.title);
  const [opened] = await shownNotifications();
  record(`end to end push service: ${subscribed.endpointHost}; private key extractable=${sealed.extractable}`);
  record(`end to end opened: title=${JSON.stringify(opened.title)} body=${JSON.stringify(opened.body)} tag=${opened.tag} url=${opened.url}`);
  expect(opened).toEqual({
    title: sealed.message.title, body: sealed.message.body, tag: "build-task-run-e2e", url: sealed.message.url,
  });

  // The same blob again is a replay: the same tag, now the generic copy.
  await cdp.send("ServiceWorker.deliverPushMessage", { origin, registrationId, data: JSON.stringify(sealed.payload) });
  await expect.poll(async () => (await shownNotifications())[0]?.title, { timeout: 15_000 }).toBe("Build");
  const [replayed] = await shownNotifications();
  record(`end to end replay: title=${JSON.stringify(replayed.title)} body=${JSON.stringify(replayed.body)} tag=${replayed.tag}`);
  expect(replayed).toMatchObject({ title: "Build", body: "An agent needs you", url: "/app/#/generic" });

  await page.evaluate((shown) => { document.querySelector("#shown").textContent = shown; },
    JSON.stringify({ opened, replayed }, null, 2));
  if (process.env.BUILD_PUSH_PROOF_DIR) await page.screenshot({ path: join(proofDir, "sealed-push-e2e.png") });
  await context.close();
}, 90_000);
