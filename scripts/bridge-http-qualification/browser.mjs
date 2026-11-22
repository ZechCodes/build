// Drives the shipped SPA and the real local issuer/bridge. Negative cases
// alter a discovery proof or ticket request. Responses and events stay real.
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
const mountedTestOrigin = process.env.TERMINAL_WSS_TEST_ORIGIN;
const fixture = mountedTestOrigin ? {} : JSON.parse(await readFile(process.env.BUILD_HTTP_QUALIFICATION_FIXTURE, "utf8"));
const { hostedOrigin, bridgeOrigin, deviceId, email } = fixture;
const terminalGate = process.env.BUILD_HTTP_QUALIFICATION_TERMINAL;
for (const origin of mountedTestOrigin ? [] : [hostedOrigin, bridgeOrigin]) {
  assert.equal(new URL(origin).protocol, "https:");
  assert.equal(new URL(origin).hostname, terminalGate && origin === bridgeOrigin ? "127.0.0.1" : "localhost");
}
const output = fixture.logsDir || resolve(process.env.TMPDIR, "qualification-browser");
await mkdir(output, { recursive: true });
if (mountedTestOrigin) {
  await qualifyBrowserTerminalWitness();
} else if (terminalGate) {
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
    await qualifyBrowserTicketRefusal(page);
  } finally {
    writer.clear();
  }
}

async function qualifyBrowserTicketRefusal(page) {
  const ticketUrl = bridgeOrigin + "/v1/terminal-tickets";
  await page.route(ticketUrl, (route) => {
    const request = route.request();
    if (request.method() !== "POST") return route.continue();
    return route.continue({
      headers: { ...request.headers(), authorization: "Bearer qualification-invalid-device-bearer" },
    });
  });
  const [refusal] = await Promise.all([
    page.waitForResponse((response) => response.url() === ticketUrl && response.request().method() === "POST"),
    page.getByRole("button", { name: "New terminal", exact: true }).click({ timeout: 15_000 }),
  ]);
  assert.equal(refusal.status(), 401);
  assert.equal((await refusal.json()).error_code, "invalid_credential");
  const notice = page.getByRole("alert").filter({ hasText: "Could not open a terminal" });
  await notice.getByRole("button", { name: "Show details" }).click({ timeout: 10_000 });
  await notice.getByText("Bridge authorization expired or was refused. Verify this device with your account again.", {
    exact: true,
  }).waitFor({ timeout: 10_000 });
  console.log("PASS browser handled an actual 401 terminal ticket refusal");
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

async function qualifyBrowserTerminalWitness() {
  const bridge = new URL(mountedTestOrigin);
  assert.equal(bridge.origin, mountedTestOrigin);
  assert.equal(bridge.protocol, "https:");
  assert.equal(bridge.hostname, "localhost");
  const contract = JSON.parse(await readFile(resolve(root, "fixtures/api/http/terminal-websocket-v1.json"), "utf8"));
  const manifest = JSON.parse(await readFile(resolve(root, "fixtures/api/http/resource-manifest.json"), "utf8"));
  const useWebKit = process.env.TERMINAL_WSS_TEST_ENGINE === "webkit";
  const engine = useWebKit ? webkit : chromium;
  const options = useWebKit ? {
    executablePath: process.env.BUILD_HTTP_QUALIFICATION_WEBKIT,
    env: { ...process.env, QUALIFICATION_CA_FILE: process.env.NODE_EXTRA_CA_CERTS, QUALIFICATION_RUN_DIR: process.env.TMPDIR },
  } : chromiumOptions(resolve(output, "chromium-lost-close"));
  const browser = await engine.launch({ ...options, headless: true });
  try {
    // This fixture exercises the dropped Close and Retry using a generated
    // listener certificate. The hosted browser profile checks strict CA trust.
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const page = await context.newPage();
    const caller = contract.endpoint.caller_origin;
    if (!useWebKit) await context.grantPermissions(["local-network-access"], { origin: caller });
    await context.route("**/*", (route) => terminalWitnessRoute(route, caller, bridge.origin));
    await page.goto(caller);
    const profile = `${useWebKit ? "WebKit" : "Chromium"} ${browser.version()}`;
    if (process.env.TERMINAL_WSS_TEST_SCENARIO === "paint") {
      await qualifyBrowserMaximumGrid(page, { origin: bridge.origin, manifest, recipe: contract.recovery }, profile);
    } else {
      await qualifyBrowserLostClose(page, { origin: bridge.origin, manifest }, profile);
    }
  } finally {
    await browser.close();
  }
}

async function qualifyBrowserLostClose(page, options, profile) {
  await page.evaluate(startLostClosePane, options);
  const notice = page.getByText("Snapshot timed out. Use a smaller pane or a faster connection, then Retry.", { exact: true });
  await notice.waitFor({ timeout: 8000 });
  const evidence = () => page.evaluate(() => window.__lostCloseRun);
  const refused = await evidence();
  assert.deepEqual(refused.closes, [1006]);
  assert.deepEqual(refused.tickets, [
    { retry: false, status: 201 },
    { retry: false, status: 409, code: "snapshot_write_timeout" },
  ]);
  await new Promise((resolve) => setTimeout(resolve, 9000));
  assert.deepEqual((await evidence()).tickets, refused.tickets);
  await page.getByRole("button", { name: "Retry" }).click();
  await page.waitForFunction(() => window.__lostCloseRun.restored);
  await notice.waitFor({ state: "hidden" });
  const recovered = await evidence();
  assert.deepEqual(recovered.tickets, [...refused.tickets, { retry: true, status: 201 }]);
  console.log(`PASS ${profile} 1006, ticket 409, quiet retry window, explicit Retry 201`);
}

async function terminalWitnessRoute(route, caller, bridge) {
  const url = new URL(route.request().url());
  if (url.origin === bridge) return route.continue();
  if (url.origin !== caller) return route.abort();
  if (url.pathname === "/") {
    return route.fulfill({ contentType: "text/html", body: '<p id="notice"></p><button hidden>Retry</button>' });
  }
  const ghostty = resolve(root, "spa/node_modules/ghostty-web");
  if (url.pathname === "/qualification/ghostty-web.js") {
    return route.fulfill({ contentType: "text/javascript", path: resolve(ghostty, "dist/ghostty-web.js") });
  }
  if (url.pathname === "/qualification/ghostty-vt.wasm") {
    return route.fulfill({ contentType: "application/wasm", path: resolve(ghostty, "ghostty-vt.wasm") });
  }
  const source = resolve(root, "spa/src");
  const path = resolve(source, "." + url.pathname);
  return path.startsWith(source + "/") && path.endsWith(".js")
    ? route.fulfill({ contentType: "text/javascript", path })
    : route.abort();
}

async function qualifyBrowserMaximumGrid(page, options, profile) {
  await page.evaluate(startMaximumGridPane, options);
  const waitFor = (predicate, arg) => page.waitForFunction(predicate, arg, { timeout: 15000 });
  await waitFor(() => window.__grid.status !== undefined);
  assert.equal(await page.evaluate(() => window.__grid.status), "painted");
  await page.evaluate(() => window.__grid.input("r"));
  await waitFor(() => window.__grid.status !== "painted");
  assert.equal(await page.evaluate(() => window.__grid.status), "reset");
  await page.evaluate((next) => window.__grid.input(next), options.recipe.next_output);
  await waitFor(() => window.__grid.tail);
  await page.evaluate((encoded) => window.__grid.input(atob(encoded)), options.recipe.next_input_base64);
  await waitFor(() => window.__grid.echo);
  const evidence = await page.evaluate(() => ({
    attachments: window.__grid.attachments, tickets: window.__grid.tickets, sizes: window.__grid.sizes,
  }));
  assert.equal(evidence.attachments, 1);
  assert.equal(evidence.tickets, 1);
  assert.equal(evidence.sizes.length, 2);
  assert.ok(evidence.sizes.every((bytes) => bytes > 1024 * 1024 && bytes <= 12 * 1024 * 1024));
  const { cols, rows } = options.recipe;
  console.log(`PASS ${profile} ${cols}×${rows} Ghostty attach and reset, next live byte and input, frame bytes ${evidence.sizes.join(", ")}`);
}

async function startMaximumGridPane({ origin, manifest, recipe }) {
  const { createBrowserHttpEndpoint } = await import("/core/httpBrowserEndpoint.js");
  const { openHttpSession } = await import("/core/httpSession.js");
  const { createHttpRoutes } = await import("/core/httpRoutes.js");
  const { WebSocketTerminalSession } = await import("/terminal/webSocketSession.js");
  const { Ghostty } = await import("/qualification/ghostty-web.js");
  const wasm = await (await fetch("/qualification/ghostty-vt.wasm")).arrayBuffer();
  const { instance } = await WebAssembly.instantiate(wasm, { env: { log() {} } });
  const ghostty = new Ghostty(instance);
  const grid = { attachments: 0, tickets: 0, sizes: [], tail: false, echo: false };
  window.__grid = grid;
  const endpoint = createBrowserHttpEndpoint({
    grant: { identity: { origin, deviceId: "qualification" }, token: () => "writer",
      expiresAt: () => Date.now() + 60000, renew: async () => {}, close() {} },
    async fetchImpl(url, init) {
      if (new URL(url).pathname === "/v1/terminal-tickets") grid.tickets++;
      return fetch(url, init);
    },
    createWebSocket(url, protocols) {
      const socket = new WebSocket(url, protocols);
      socket.addEventListener("message", ({ data }) => {
        const frame = JSON.parse(new TextDecoder().decode(data));
        if (frame.type === "term.reset" || frame.result?.snapshot) grid.sizes.push(data.byteLength);
      });
      return socket;
    },
  });
  const selected = await openHttpSession({ endpoint, routes: createHttpRoutes(manifest) });
  selected.allowRefresh();
  const panes = new WebSocketTerminalSession({
    userTerminalDevice: () => "qualification",
    userTerminalRpc: (_device, method, params, call) => {
      if (method === "term.attach") grid.attachments++;
      return call(method, params);
    },
    rosterCall: (method, params) => selected.rawCall(method, params), panes: [],
  });
  panes.follow("qualification", { open: (signal, options) => selected.openTerminal({ signal, ...options }), isCurrent: () => true });
  grid.input = (input) => panes.input("painted", input);
  const { cols, rows, cells } = recipe;
  const lines = cells.map(({ sgr, text }) => sgr + text);
  const paint = recipe.prefix + (lines.join("").repeat(cols / 2) + lines.toReversed().join("").repeat(cols / 2))
    .repeat(rows / 2) + recipe.suffix;
  let terminal;
  void panes.attachTerminal("painted", {}, {
    cols, rows, afterRender: true,
    onFailure(failure) { if (failure) grid.status = failure.message; },
    onSnapshot(bytes, applied) {
      const afterReset = Boolean(terminal);
      terminal?.free();
      terminal = ghostty.createTerminal(cols, rows);
      terminal.write(bytes);
      const expected = ghostty.createTerminal(cols, rows);
      expected.write(paint + (afterReset ? "\x1b[2;5H?\x1b[6;8H" : ""));
      const mismatch = compareGrid(terminal, expected);
      expected.free();
      grid.status = mismatch || (afterReset ? "reset" : "painted");
      applied();
    },
    onOutput(bytes, applied) {
      terminal?.write(bytes);
      const text = new TextDecoder().decode(bytes);
      if (text === recipe.next_output) {
        terminal.update();
        grid.tail = terminal.getGraphemeString(5, 7) === recipe.next_output;
      } else if (grid.tail && text === atob(recipe.next_input_base64)) {
        terminal.update();
        grid.echo = terminal.getGraphemeString(5, 8) === text;
      }
      applied();
    },
  });

  function compareGrid(actual, expected) {
    actual.update();
    expected.update();
    const actualCursor = actual.getCursor();
    const expectedCursor = expected.getCursor();
    if (actualCursor.x !== expectedCursor.x || actualCursor.y !== expectedCursor.y) return "cursor mismatch";
    const left = actual.getViewport();
    const right = expected.getViewport();
    if (left.length !== cols * rows || right.length !== left.length) return "viewport size mismatch";
    for (let i = 0; i < left.length; i++) {
      if (Object.keys(right[i]).some((field) => left[i][field] !== right[i][field])) return "cell mismatch at " + i;
    }
    for (const { row, col } of recipe.witness_cells) {
      if (actual.getGraphemeString(row, col) !== expected.getGraphemeString(row, col)) return "grapheme mismatch";
    }
    return null;
  }
}

async function startLostClosePane({ origin, manifest }) {
  const { createBrowserHttpEndpoint } = await import("/core/httpBrowserEndpoint.js");
  const { openHttpSession } = await import("/core/httpSession.js");
  const { createHttpRoutes } = await import("/core/httpRoutes.js");
  const { WebSocketTerminalSession } = await import("/terminal/webSocketSession.js");
  const evidence = { tickets: [], closes: [], restored: false };
  window.__lostCloseRun = evidence;
  const endpoint = createBrowserHttpEndpoint({
    grant: { identity: { origin, deviceId: "qualification" }, token: () => "writer",
      expiresAt: () => Date.now() + 60000, renew: async () => {}, close() {} },
    async fetchImpl(url, init) {
      const response = await fetch(url, init);
      if (new URL(url).pathname === "/v1/terminal-tickets") {
        const ticket = { retry: JSON.parse(init.body).retry === true, status: response.status };
        if (!response.ok) ticket.code = (await response.clone().json()).error_code;
        evidence.tickets.push(ticket);
      }
      return response;
    },
    createWebSocket(url, protocols) {
      const socket = new WebSocket(url, protocols);
      socket.addEventListener("close", ({ code }) => evidence.closes.push(code), { once: true });
      return socket;
    },
  });
  const session = await openHttpSession({ endpoint, routes: createHttpRoutes(manifest) });
  session.allowRefresh();
  const panes = new WebSocketTerminalSession({
    userTerminalDevice: () => "qualification",
    userTerminalRpc: (_device, method, params, call) => call(method, params),
    rosterCall: (method, params) => session.rawCall(method, params),
    panes: [],
  });
  panes.follow("qualification", { open: (signal, options) => session.openTerminal({ signal, ...options }), isCurrent: () => true });
  const button = document.querySelector("button");
  void panes.attachTerminal("term-1", {}, {
    deviceId: "qualification",
    afterRender: true,
    onFailure(failure) {
      document.querySelector("#notice").textContent = failure?.message || "";
      button.hidden = !failure?.retry;
      button.onclick = failure?.retry;
    },
    onSnapshot(bytes, applied) {
      evidence.restored = new TextDecoder().decode(bytes) === "ok";
      applied();
    },
  });
}
