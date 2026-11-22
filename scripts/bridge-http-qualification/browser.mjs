// Drives the shipped SPA and the real local issuer/bridge. Only the negative
// discovery proof is altered; responses and application events remain real.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { refusalFixture, qualifyBrowserRefusals } from "./refusals.mjs";

const root = process.cwd();
const require = createRequire(resolve(root, "spa/package.json"));
const { chromium, webkit } = require("playwright-core");
const fixture = JSON.parse(await readFile(process.env.BUILD_HTTP_QUALIFICATION_FIXTURE, "utf8"));
const { hostedOrigin, bridgeOrigin, deviceId, email } = fixture;
const terminalGate = process.env.BUILD_HTTP_QUALIFICATION_TERMINAL;
for (const origin of [hostedOrigin, bridgeOrigin]) {
  assert.equal(new URL(origin).protocol, "https:");
  assert.equal(new URL(origin).hostname, terminalGate && origin === bridgeOrigin ? "127.0.0.1" : "localhost");
}
const output = fixture.logsDir || resolve(process.env.TMPDIR, "qualification-browser");
await mkdir(output, { recursive: true });
if (terminalGate) {
  await qualifyTerminalBrowsers();
} else {
  await qualifyRestBrowser();
}

async function qualifyRestBrowser() {
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
const arrangedRefusals = await refusalFixture(context, fixture);
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
  const name = `browser-live-sse-${Date.now()}`;
  // No durable-write method is advertised by this bridge generation.
  const created = await writer.request(owner, writerHandle, "project_create", { name, parent: fixture.projectsDir });
  assert.equal(created.ok, true, JSON.stringify(created));
  await page.getByText(name, { exact: true }).first().waitFor({ timeout: 20000 });
  await page.waitForFunction(() => window.__qualification.types.includes("changes"), undefined, { timeout: 20000 });
  console.log("PASS independent project write delivered through SSE and rendered from the SPA cache");
  await page.waitForFunction(() => window.__qualification.requests.some((request) => request.method === "PATCH" && request.path.startsWith("/v1/sessions/") && request.status === 200), undefined, { timeout: 35000 });
  console.log("PASS automatic PATCH session renewal against the real bridge");
  await page.screenshot({ path: resolve(output, "spa-live-rest-events.png"), fullPage: true });
  await page.selectOption("#device-transport", "webrtc");
  await page.waitForFunction(() => window.__qualification.requests.some((request) => request.method === "DELETE" && request.path.startsWith("/v1/sessions/") && request.status < 300), undefined, { timeout: 10000 });
  const evidence = await page.evaluate(() => window.__qualification);
  assert.ok(evidence.events.includes("hello"));
  assert.ok(evidence.types.includes("changes"));
  assert.ok(evidence.heartbeats > 0, "real SSE heartbeat observed before renewal");
  await writeFile(resolve(output, "browser-evidence.json"), JSON.stringify({ ...evidence, refusal, errors }, null, 2));
  console.log("PASS explicit transport change closed the HTTP session and subscription");
  const refusals = await qualifyBrowserRefusals(page, arrangedRefusals, bridgeOrigin, output);
  await writeFile(resolve(output, "browser-refusals.json"), JSON.stringify(refusals, null, 2));
  assert.deepEqual(errors, [], "no uncaught browser errors");
} catch (error) {
  await page.screenshot({ path: resolve(output, "spa-failure.png"), fullPage: true }).catch(() => {});
  const text = await page.locator("body").innerText().catch(() => "");
  await writeFile(resolve(output, "browser-failure.txt"), `${error.stack}\n${text}\n${JSON.stringify(await page.evaluate(() => window.__qualification).catch(() => null), null, 2)}`);
  throw error;
} finally {
  await arrangedRefusals.close();
  if (writerHandle) await writer.close(owner, writerHandle);
  await browser.close();
}
}

function chromiumOptions(home) {
  return {
    executablePath: process.env.CHROMIUM_PATH || "/usr/bin/chromium", headless: true,
    env: { ...process.env, HOME: home, XDG_DATA_HOME: resolve(home, ".local/share") },
  };
}

async function trustChromiumCa(home) {
  // Isolated NSS databases cover both Chromium's current and legacy locations.
  for (const dir of [".local/share/pki/nssdb", ".pki/nssdb"]) {
    const path = resolve(home, dir);
    await mkdir(path, { recursive: true, mode: 0o700 });
    const db = `sql:${path}`;
    execFileSync("certutil", ["-N", "--empty-password", "-d", db]);
    execFileSync("certutil", ["-A", "-d", db, "-t", "C,,", "-n", "Build qualification CA", "-i", fixture.caFile]);
  }
}

async function qualifyTerminalBrowsers() {
  const trustHome = resolve(fixture.runDir, "chromium-trust");
  await trustChromiumCa(trustHome);
  const untrusted = await chromium.launch(chromiumOptions(resolve(fixture.runDir, "chromium-untrusted")));
  try {
    const page = await untrusted.newPage();
    await assert.rejects(page.goto(bridgeOrigin), /ERR_CERT_AUTHORITY_INVALID/);
  } finally {
    await untrusted.close();
  }
  console.log("PASS Chromium refused the bridge certificate without the fixture CA");
  const engines = [
    ["Chromium", chromium, chromiumOptions(trustHome)],
    ["WebKit", webkit, {
      headless: true, executablePath: process.env.BUILD_HTTP_QUALIFICATION_WEBKIT,
      env: { ...process.env, QUALIFICATION_CA_FILE: fixture.caFile, QUALIFICATION_RUN_DIR: fixture.runDir },
    }],
  ];
  for (const [name, engine, options] of engines) {
    const browser = await engine.launch(options);
    try {
      await qualifyTerminalDocument(browser, name);
      console.log(`PASS ${name} ${browser.version()} default transport ${terminalGate === "off" ? "WebRTC declaration" : "WSS shell"}`);
    } finally {
      await browser.close();
    }
  }
}

async function instrumentTerminalContext(context) {
  await context.route("**/*", (route) => {
    const { origin } = new URL(route.request().url());
    return [hostedOrigin, bridgeOrigin].includes(origin) ? route.continue() : route.abort();
  });
  await context.addInitScript(({ bridgeOrigin }) => {
    const evidence = { sockets: 0, otherSockets: 0, rtc: 0, sends: [], replies: [], pushes: {}, markers: [] };
    window.__terminalQualification = evidence;
    const native = window.WebSocket;
    const decode = new TextDecoder();
    const parse = (bytes) => JSON.parse(decode.decode(bytes));
    const markers = ["wss-browser-qualification", "wss-browser-reset", "wss-browser-recovered"];
    window.WebSocket = class extends native {
      calls = new Map();
      constructor(url, protocols) {
        if (url !== bridgeOrigin.replace("https:", "wss:") + "/v1/terminal") {
          evidence.otherSockets++;
          throw new Error("Qualification has no relay socket");
        }
        super(url, protocols);
        evidence.sockets++;
        window.__closeQualificationSocket = () => this.close(4001, "qualification transport loss");
        this.addEventListener("message", ({ data }) => qualificationMessage(this.calls, data));
      }
      send(bytes) {
        const { id, method } = parse(bytes);
        if (method) {
          this.calls.set(id, method);
          evidence.sends.push(method);
        }
        super.send(bytes);
      }
    };
    window.RTCPeerConnection = class {
      constructor() {
        evidence.rtc++;
        throw new Error("Qualification has no WebRTC");
      }
    };
    let tail = "";
    async function qualificationMessage(calls, data) {
      const frame = parse(data instanceof Blob ? await data.arrayBuffer() : data);
      const method = calls.get(frame.id);
      if (method) {
        calls.delete(frame.id);
        evidence.replies.push({ method, ok: frame.ok, code: frame.error_code });
      }
      if (frame.type) evidence.pushes[frame.type] = (evidence.pushes[frame.type] || 0) + 1;
      if (frame.type === "term.output" || frame.type === "term.reset") {
        const output = tail + atob(frame.data);
        for (const marker of markers) {
          if (!evidence.markers.includes(marker) && output.includes(marker)) evidence.markers.push(marker);
        }
        tail = output.slice(-32);
      }
    }
  }, { bridgeOrigin });
}

async function qualifyTerminalDocument(browser, name) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1100 }, serviceWorkers: "block" });
  const page = await context.newPage();
  const requests = [];
  const responses = [];
  const safePath = (url) => new URL(url).pathname;
  page.on("request", (request) => requests.push(safePath(request.url())));
  page.on("response", (response) => responses.push({ path: safePath(response.url()), status: response.status() }));
  await instrumentTerminalContext(context);
  try {
    await page.goto(hostedOrigin + "/auth/dummy/login");
    await page.fill("#email", email);
    await page.fill("#name", "Terminal browser qualification");
    await page.click("button[type=submit]");
    await page.goto(hostedOrigin + "/app/");
    await page.waitForResponse((response) => safePath(response.url()) === "/api/devices" && response.ok());
    await page.goto(`${hostedOrigin}/app/#/device/${deviceId}/settings`);
    await page.reload();
    await page.locator("[data-http-add]").waitFor({ timeout: 20000 });
    const [document] = await Promise.all([
      page.waitForResponse((response) => new URL(response.url()).searchParams.has("bridge_discovery") &&
        response.request().isNavigationRequest()),
      startBrowserDiscovery(page),
    ]);
    const headers = (await document.headersArray()).filter(({ name }) => name.toLowerCase() === "content-security-policy");
    assert.equal(headers.length, 1, "first-use response has one enforced CSP");
    const connect = headers[0].value.split(";").find((part) => part.trimStart().startsWith("connect-src ")).trim().split(/\s+/);
    assert.ok(connect.includes(bridgeOrigin), "first-use policy authorizes exact HTTPS port");
    assert.ok(connect.includes(bridgeOrigin.replace("https:", "wss:")), "first-use policy authorizes exact WSS port");
    await Promise.all([
      page.waitForResponse((response) => safePath(response.url()) === "/v1/sessions" &&
        response.request().method() === "POST" && response.status() === 201),
      page.locator("#urlconnect").click({ timeout: 20000 }),
    ]);
    await page.waitForFunction(() => document.querySelector("#device-transport")?.value === "http", undefined, { timeout: 20000 });
    assert.ok(responses.some(({ path, status }) => path === "/v1/sessions" && status === 201));
    await page.locator("[data-settings-close]").click();
    if (terminalGate === "on") await qualifyBrowserShell(page, name);
    const evidence = await page.evaluate(() => window.__terminalQualification);
    assert.equal(evidence.sockets, terminalGate === "on" ? 3 : 0);
    assert.equal(evidence.rtc, 0);
    assert.equal(evidence.otherSockets, 0);
    assert.equal(requests.filter((path) => ["/api/gateway-token", "/api/rtc/ice-servers"].includes(path)).length, 0);
  } catch (error) {
    const status = await page.evaluate(() => window.__terminalQualification).catch(() => null);
    if (status) console.error(JSON.stringify({
      sockets: status.sockets, methods: status.sends,
      replies: status.replies, pushes: status.pushes, rtc: status.rtc, otherSockets: status.otherSockets,
      markers: status.markers,
    }));
    await page.screenshot({ path: resolve(output, `${name}-terminal-failure.png`) }).catch(() => {});
    throw error;
  } finally {
    await context.close();
  }
}

async function startBrowserDiscovery(page) {
  await page.click("[data-http-add]");
  await page.fill("#bridge-url", bridgeOrigin);
  await page.click("#urlverify");
}

async function qualifyBrowserShell(page, name) {
  const { createBridgeHttp } = await import(pathToFileURL(resolve(root, "desktop/src/bridge-http.mjs")));
  const manifest = JSON.parse(await readFile(resolve(root, "desktop/src/bridge-http-resources.json"), "utf8"));
  const owner = {};
  const writer = createBridgeHttp({
    hostedOrigin, manifest, getAuth: () => ({ generation: 1, accessToken: fixture.accessToken }),
    isOwnerCurrent: (value) => value === owner, send: () => {},
  });
  try {
    const { handle } = await writer.discover(owner, bridgeOrigin);
    await writer.open(owner, handle, "^3.0.0");
    const project = `wss-browser-${name}-${Date.now()}`;
    const created = await writer.request(owner, handle, "project_create", { name: project, parent: fixture.projectsDir });
    assert.equal(created.ok, true, created.error);
    await page.getByText(project, { exact: true }).first().click({ timeout: 20000 });
    await page.locator("#console-toggle[aria-expanded=false]").click({ timeout: 10000 });
    await page.getByRole("button", { name: "New terminal", exact: true }).click({ timeout: 15000 });
    await waitForTerminalReply(page, "term.attach");
    const viewport = page.viewportSize();
    await page.setViewportSize({ width: viewport.width - 200, height: viewport.height - 100 });
    await waitForTerminalReply(page, "term.resize");
    await typeShellMarker(page, "qualification");
    await waitForTerminalReply(page, "term.ack");
    const resets = await page.evaluate(() => window.__terminalQualification.pushes["term.reset"] || 0);
    await typeShellCommand(page, "head -c 3000000 /dev/zero | tr '\\0' x");
    await page.waitForFunction((before) =>
      (window.__terminalQualification.pushes["term.reset"] || 0) > before, resets);
    await typeShellMarker(page, "reset");
    await page.evaluate(() => window.__closeQualificationSocket());
    await page.waitForFunction(() => window.__terminalQualification.replies
      .filter(({ method, ok }) => method === "term.attach" && ok).length === 2);
    await typeShellMarker(page, "recovered");
    await page.screenshot({ path: resolve(output, `${name}-terminal.png`) });
    await page.locator(".console-tab.active .tx").click();
    await page.waitForFunction(() => {
      const evidence = window.__terminalQualification;
      return evidence.sends.includes("term.close") && evidence.pushes["term.closed"] > 0;
    });
    await page.locator(".console-tab.active").waitFor({ state: "detached" });
  } finally {
    writer.clear();
  }
}

async function typeShellMarker(page, suffix) {
  await typeShellCommand(page, "printf '%s%s\\n' wss-browser- " + suffix);
  await page.waitForFunction((marker) => window.__terminalQualification.markers.includes(marker), "wss-browser-" + suffix);
}

async function typeShellCommand(page, command) {
  await page.locator(".term-screen").click();
  await page.keyboard.type(command, { delay: 40 });
  await page.keyboard.press("Enter");
}

async function waitForTerminalReply(page, method) {
  await page.waitForFunction((method) =>
    window.__terminalQualification.replies.some((reply) => reply.method === method && reply.ok), method);
}
