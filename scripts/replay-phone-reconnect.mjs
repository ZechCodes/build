// Replay #134 against an isolated local compose stack; see the companion README.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createServer } from "../spa/node_modules/vite/dist/node/index.js";
import { loginWithDummy } from "../web/skrift-auth.mjs";

const repo = fileURLToPath(new URL("../", import.meta.url));
const root = resolve(repo, "spa");
const output = resolve(process.env.REPLAY_OUTPUT || "/tmp/phone134-replay");
const project = process.env.COMPOSE_PROJECT_NAME || "phone134";
const playwright = process.env.PLAYWRIGHT_MODULE
  ? pathToFileURL(resolve(process.env.PLAYWRIGHT_MODULE)).href
  : new URL("../spa/node_modules/playwright-core/index.mjs", import.meta.url).href;
const { chromium } = await import(playwright);
const compose = (...args) => execFileSync("docker", [
  "compose", "-p", project, "-f", "deploy/compose.real.yml", ...args,
], { cwd: repo, encoding: "utf8" });

await mkdir(output, { recursive: true });
const server = await createServer({
  root,
  server: {
    host: "127.0.0.1", port: 8134, strictPort: true, hmr: false,
    proxy: { "/api": "http://localhost:8090", "/auth": "http://localhost:8090" },
  },
  plugins: [{
    name: "observe-reconnect-rpcs",
    transform(code, id) {
      if (!id.endsWith("/src/core/session.js")) return;
      return code.replace(
        "const rawCall = (method, params = {}, options = {}) => {",
        'const rawCall = (method, params = {}, options = {}) => { console.log("REPLAY RPC", method);',
      );
    },
  }],
});

let browser;
try {
  await server.listen();
  browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true });
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const { cookie } = await loginWithDummy("http://localhost:8090");
  await context.addCookies(cookie.split("; ").map((value) => ({
    name: value.slice(0, value.indexOf("=")), value: value.slice(value.indexOf("=") + 1),
    url: "http://127.0.0.1:8134",
  })));
  await context.addInitScript(() => {
    window.replayDatabases = [];
    const open = indexedDB.open.bind(indexedDB);
    indexedDB.open = (...args) => {
      const request = open(...args);
      request.addEventListener("success", () => window.replayDatabases.push(request.result));
      return request;
    };
    window.replayPeers = [];
    const Peer = window.RTCPeerConnection;
    window.RTCPeerConnection = class extends Peer {
      constructor(...args) {
        super(...args);
        window.replayPeers.push(this);
      }
    };
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("console", (message) => console.log(new Date().toISOString(), message.type(), message.text()));
  page.on("pageerror", (error) => { pageErrors.push(error.message); console.error(error.stack); });
  await page.goto("http://127.0.0.1:8134/app/static/");
  await page.evaluate(async () => {
    window.replayModules = Object.fromEntries(await Promise.all([
      "app", "core/deviceContexts", "core/changeEvents", "core/connectionDiagnostics", "core/localCache", "core/cacheSync",
    ].map(async (name) => [name, await import(`/app/static/src/${name}.js`)])));
  });
  await page.waitForFunction(() => window.replayModules["core/deviceContexts"].knownContexts()
    .some((device) => device.apiVersion === "1.22.0"), null, { timeout: 60000 });

  // RPCs create real bridge records. The normal sync then fills the actual IDB.
  await page.evaluate(async () => {
    const modules = window.replayModules;
    const device = modules["core/deviceContexts"].knownContexts()[0];
    const projects = await device.rpc("project.list");
    const projectId = projects.projects[0].project_id;
    const workspace = await device.rpc("workspace.create", {
      project_id: projectId, name: `Phone replay ${Date.now()}`, isolation: "worktree",
    });
    const owner = await device.rpc("workspace.ensure_conversation", { workspace_id: workspace.workspace_id });
    await device.rpc("agent.add", { entity_id: owner.entity_id, name: "Phone replay agent" });
    await device.rpc("issues.create", { project_id: projectId, title: "Phone replay issue", status: "in_progress" });
    await new Promise((done) => setTimeout(done, 1500));
    await modules["core/cacheSync"].syncDevice(device.deviceId);
    window.replayOwner = owner;
    window.replayRoute = {
      name: "workspace", workspaceId: workspace.workspace_id, projectId, tab: "changes", deviceId: device.deviceId,
    };
    modules.app.go(window.replayRoute);
  });

  const populated = (count) => page.waitForFunction((minimum) =>
    document.querySelectorAll(".rail-bubble-agent").length >= minimum &&
    !document.querySelector("#root").innerText.includes("loading"), count, { timeout: 30000 });
  const snapshot = async (label) => {
    const state = await page.evaluate(() => ({
      devices: window.replayModules["core/deviceContexts"].knownContexts().map((device) => ({
        deviceId: device.deviceId, session: device.session?.sessionId, apiVersion: device.apiVersion,
        capabilities: device.adapter?.capabilities,
      })),
      agents: document.querySelectorAll(".rail-bubble-agent").length,
      body: document.querySelector("#root")?.innerText,
      diagnostics: window.replayModules["core/connectionDiagnostics"].connectionDiagnosticHistory().slice(-15),
    }));
    console.log("SNAPSHOT", label, JSON.stringify(state));
    await writeFile(resolve(output, `${label}.json`), JSON.stringify(state, null, 2));
    await page.screenshot({ path: resolve(output, `${label}.png`) });
    return state;
  };
  await populated(1);
  const before = await snapshot("before");
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await context.setOffline(true);
  await page.evaluate(() => {
    for (const db of window.replayDatabases) { db.close(); db.dispatchEvent(new Event("close")); }
    for (const peer of window.replayPeers) peer.close();
  });
  console.log("SLEEP: 35 seconds hidden/offline with closed IDB and peers");
  await page.waitForTimeout(35000);
  await populated(1);
  await snapshot("asleep");
  await context.setOffline(false);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    window.dispatchEvent(new Event("online"));
  });
  await page.waitForFunction(() => {
    const device = window.replayModules["core/deviceContexts"].knownContexts()[0];
    return device.session && !device.offline && device.apiVersion === "1.22.0" &&
      window.replayPeers.some((peer) => peer.connectionState === "connected");
  }, null, { timeout: 60000 });
  await populated(1);
  const after = await snapshot("awake");
  assert.deepEqual(after.devices[0].capabilities, before.devices[0].capabilities);
  await page.evaluate(async () => {
    const modules = window.replayModules;
    const device = modules["core/deviceContexts"].knownContexts()[0];
    const address = { deviceId: device.deviceId, entityId: "replay-probe", kind: "row" };
    await modules["core/localCache"].writeCached(address, { title: "write after wake" });
    const record = await modules["core/localCache"].readCached(address);
    if (record?.value?.title !== "write after wake") throw Error("cache did not recover");
    await device.rpc("agent.add", { entity_id: window.replayOwner.entity_id, name: "Push after wake" });
  });
  await page.waitForFunction(async () => {
    const modules = window.replayModules;
    const device = modules["core/deviceContexts"].knownContexts()[0];
    const row = await modules["core/localCache"].readCached({
      deviceId: device.deviceId, entityId: window.replayOwner.entity_id, kind: "row",
    });
    return row?.value?.agents?.some((agent) => agent.name === "Push after wake");
  }, null, { timeout: 30000 });
  await page.evaluate(() => window.replayModules.app.go({ name: "inbox" }));
  await page.waitForTimeout(300);
  await page.evaluate(() => window.replayModules.app.go(window.replayRoute));
  await populated(2);
  await snapshot("pushed");
  console.log(compose("restart", "relay"));
  await page.waitForTimeout(12000);
  await populated(2);
  await snapshot("relay-restarted");
  assert.deepEqual(pageErrors, []);
  console.log("PASS: cached view survives sleep; capabilities, writes, agent push and relay restart recover without refresh");
} finally {
  try {
    await writeFile(resolve(output, "bridge.log"), compose("logs", "--no-color", "bridge"));
  } finally {
    await browser?.close();
    await server.close();
  }
}
