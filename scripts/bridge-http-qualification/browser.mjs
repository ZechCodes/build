// Drives the shipped SPA and the real local issuer/bridge. Only the negative
// discovery proof is altered; responses and application events remain real.
import assert from "node:assert/strict";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = process.cwd();
const require = createRequire(resolve(root, "spa/package.json"));
const { chromium } = require("playwright-core");
const fixture = JSON.parse(await readFile(process.env.BUILD_HTTP_QUALIFICATION_FIXTURE, "utf8"));
const { hostedOrigin, bridgeOrigin, deviceId, email } = fixture;
for (const origin of [hostedOrigin, bridgeOrigin]) {
  assert.equal(new URL(origin).protocol, "https:");
  assert.equal(new URL(origin).hostname, "localhost");
}
const output = fixture.logsDir || resolve(process.env.TMPDIR, "qualification-browser");
await mkdir(output, { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true });
const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1440, height: 1100 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
// No public services participate in this qualification.
await context.route("**/*", (route) => ["localhost", "127.0.0.1"].includes(new URL(route.request().url()).hostname)
  ? route.continue() : route.abort());
await context.addInitScript(() => {
  const original = window.fetch.bind(window);
  const saved = () => JSON.parse(sessionStorage.getItem("qualification-evidence") || '{"requests":[],"events":[],"types":[],"heartbeats":0}');
  window.__qualification = saved();
  const save = () => sessionStorage.setItem("qualification-evidence", JSON.stringify(window.__qualification));
  window.fetch = async (input, options = {}) => {
    const url = new URL(typeof input === "string" ? input : input.url, location.href);
    if (url.pathname === "/api/bridge-discovery/complete" && sessionStorage.getItem("qualification-bad-proof")) {
      sessionStorage.removeItem("qualification-bad-proof");
      const body = JSON.parse(options.body);
      body.proof = "invalid.discovery.signature";
      options = { ...options, body: JSON.stringify(body) };
    }
    const response = await original(input, options);
    window.__qualification.requests.push({ method: options.method || "GET", path: url.pathname, status: response.status });
    save();
    if (!response.headers.get("content-type")?.startsWith("text/event-stream")) return response;
    let buffered = "";
    const decoder = new TextDecoder();
    const observer = new TransformStream({ transform(chunk, controller) {
      buffered += decoder.decode(chunk, { stream: true }).replaceAll("\r\n", "\n");
      let boundary;
      while ((boundary = buffered.indexOf("\n\n")) >= 0) {
        const frame = buffered.slice(0, boundary);
        buffered = buffered.slice(boundary + 2);
        if (frame.startsWith(":")) window.__qualification.heartbeats += 1;
        const event = frame.match(/^event: ?(.+)$/m)?.[1];
        if (event) window.__qualification.events.push(event);
        const data = frame.match(/^data: ?(.+)$/m)?.[1];
        if (data) { try { const type = JSON.parse(data).type; if (type) window.__qualification.types.push(type); } catch { /* Client parser owns malformed-event errors. */ } }
      }
      save();
      controller.enqueue(chunk);
    } });
    return new Response(response.body.pipeThrough(observer), { status: response.status, headers: response.headers });
  };
});

let writer;
let owner;
let writerHandle;
try {
  await page.goto(hostedOrigin + "/auth/dummy/login");
  await page.fill("#email", email);
  await page.fill("#name", "REST browser qualification");
  await page.click("button[type=submit]");
  await page.goto(hostedOrigin + "/app/");
  await page.waitForFunction(() => window.__qualification.requests.some((request) => request.path === "/api/devices" && request.status === 200));
  // A second document renders the device list the real first boot cached,
  // allowing settings even though no WebRTC relay runs in this fixture.
  await page.goto(`${hostedOrigin}/app/#/device/${deviceId}/settings`);
  await page.reload();
  await page.locator("[data-http-add]").waitFor();
  await page.evaluate(() => sessionStorage.setItem("qualification-bad-proof", "1"));
  await page.click("[data-http-add]");
  await page.fill("#bridge-url", bridgeOrigin);
  await page.click("#urlverify");
  await page.waitForFunction(() => document.querySelector("#urlerror")?.textContent.includes("authorization"));
  const refusal = await page.locator("#urlerror").innerText();
  assert.match(refusal, /Verify this device with your account again/);
  assert.equal(await page.locator("#urlconnect").count(), 0);
  await page.screenshot({ path: resolve(output, "spa-bad-discovery-proof.png"), fullPage: true });
  console.log("PASS bad signed discovery proof refused with visible account-verification guidance");
  await page.click("#urlcancel");
  await page.click("[data-http-add]");
  await page.fill("#bridge-url", bridgeOrigin);
  await page.click("#urlverify");
  await page.locator("#urlconnect").waitFor({ timeout: 15000 });
  await page.click("#urlconnect");
  await page.waitForFunction(() => document.querySelector("#device-transport")?.value === "http", undefined, { timeout: 20000 });
  await page.waitForFunction(() => !document.querySelector("#scrim")?.classList.contains("show"));
  // A settings page opened offline retains its explicit Retry until a settings
  // lease is acquired, even after the application transport has connected.
  if (await page.locator("#device-settings-retry").isVisible()) await page.click("#device-settings-retry");
  await page.locator("#projlist").waitFor({ timeout: 20000 });
  console.log("PASS Add by URL, hosted credential, HTTPS session and application subscription");

  const { createBridgeHttp } = await import(pathToFileURL(resolve(root, "desktop/src/bridge-http.mjs")));
  const manifest = JSON.parse(await readFile(resolve(root, "desktop/src/bridge-http-resources.json"), "utf8"));
  owner = { id: "browser-event-writer" };
  writer = createBridgeHttp({ hostedOrigin, manifest, getAuth: () => ({ generation: 1, accessToken: fixture.accessToken }),
    isOwnerCurrent: (candidate) => candidate === owner, send: () => {} });
  const discovered = await writer.discover(owner, bridgeOrigin);
  writerHandle = discovered.handle;
  await writer.open(owner, writerHandle, "^3.0.0");
  const name = `Browser live SSE ${Date.now()}`;
  await writer.request(owner, writerHandle, "project_create", { name, parent: fixture.projectsDir }, { write: { operation_id: randomUUID() } });
  await page.waitForFunction((expected) => document.querySelector("#projlist")?.textContent.includes(expected), name, { timeout: 20000 });
  await page.waitForFunction(() => window.__qualification.types.includes("changes"), { timeout: 20000 });
  console.log("PASS independent project write delivered through SSE and rendered from the SPA cache");
  await page.waitForFunction(() => window.__qualification.requests.some((request) => request.method === "PATCH" && request.path.startsWith("/v1/sessions/") && request.status === 200), { timeout: 35000 });
  console.log("PASS automatic PATCH session renewal against the real bridge");
  await page.screenshot({ path: resolve(output, "spa-live-rest-events.png"), fullPage: true });
  await page.selectOption("#device-transport", "webrtc");
  await page.waitForFunction(() => window.__qualification.requests.some((request) => request.method === "DELETE" && request.path.startsWith("/v1/sessions/") && request.status < 300), { timeout: 10000 });
  const evidence = await page.evaluate(() => window.__qualification);
  assert.ok(evidence.events.includes("hello"));
  assert.ok(evidence.types.includes("changes"));
  assert.ok(evidence.heartbeats > 0, "real SSE heartbeat observed before renewal");
  await writeFile(resolve(output, "browser-evidence.json"), JSON.stringify({ ...evidence, refusal, errors }, null, 2));
  console.log("PASS explicit transport change closed the HTTP session and subscription");
  assert.deepEqual(errors, [], "no uncaught browser errors");
} catch (error) {
  await page.screenshot({ path: resolve(output, "spa-failure.png"), fullPage: true }).catch(() => {});
  const text = await page.locator("body").innerText().catch(() => "");
  await writeFile(resolve(output, "browser-failure.txt"), `${error.stack}\n${text}\n${JSON.stringify(await page.evaluate(() => window.__qualification).catch(() => null), null, 2)}`);
  throw error;
} finally {
  if (writerHandle) await writer.close(owner, writerHandle);
  await browser.close();
}
