import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { expect, it } from "vitest";
import manifest from "../../../fixtures/api/http/resource-manifest.json";
import { loadChatOverviewModules } from "./chatOverviewSeed.mjs";
import { shellHtml, SHELL_STYLES } from "./chatMenuSeed.mjs";
import { mountLayout, openLayoutSession } from "./layoutHarness.mjs";

const execute = promisify(execFile);
const greeting = { api_version: "3.39.0", bridge_epoch: "epoch", journal_incarnation: "journal",
  capabilities: ["bridge.epoch", "agents.projectWorkers"] };
const prefix = "/v1/entities/project-owner";
const hello = (session) => `event: hello\ndata: ${JSON.stringify({ subscription_id: session,
  opening_generation: session, bridge_epoch: "epoch", durable: false, recovery: "current_state_refresh" })}\n\n`;

function projectResponses() {
  const seen = [];
  let cursor = 0;
  const agent = () => ({ id: "pin", conversation_id: "conversation", thread_id: "thread", thread_generation_revision: 0,
    ordinal: 1, provider: "codex", topic: "Project coordinator", project_role: "coordinator", watched: true,
    unread_count: 1 - cursor, read_through_sequence: cursor });
  const roster = () => ({ entity_id: "project-owner", pinned_agent_id: "pin", agents: [agent()] });
  const row = () => ({ ...roster(), kind: "project", project_id: "project" });
  const handlers = {
    "/v1/board": () => ({ items: [row()] }),
    "/v1/models": () => ({ providers: [] }),
    [`${prefix}/agent-queries`]: roster,
    [`${prefix}/message-queries`]: () => ({ id: "conversation", thread_id: "thread", thread_generation_revision: 0,
      oldest_sequence: 1, thread_last_sequence: 1, thread_total: 1, has_more: false, sessions: [], items: [
        { type: "message", data: { id: "message", role: "agent", sequence: 1, body: "Ready for review", created_at: "2026-11-22T20:00:00Z" } },
      ] }),
    [`${prefix}/read-cursor`]: (body, session) => {
      seen.push({ session, body });
      if (body.agent_id === "pin" && body.thread_id === "thread" && body.read_from_sequence === 1 && body.read_through_sequence === 1) cursor = 1;
      return { ok: true };
    },
  };
  return { handlers, seen };
}

// The bridge suite exercises the native CORS policy and persistence on real
// TLS. This fixture exercises the browser and cache consumer on the same wire.
async function serveProject(directory) {
  const key = join(directory, "key.pem");
  const cert = join(directory, "cert.pem");
  await execute("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=localhost",
    "-addext", "subjectAltName=IP:127.0.0.1", "-keyout", key, "-out", cert]);
  const { handlers, seen } = projectResponses();
  const requests = [];
  const openings = new Set();
  const streams = new Map();
  const allowed = [...new Set(manifest.operations.map((operation) => operation.verb))].join(", ");
  let appOrigin;
  const server = createServer({ key: await readFile(key), cert: await readFile(cert) }, async (request, response) => {
    const { method, url, headers } = request;
    requests.push({ method, url, session: headers["x-build-session"] });
    const json = (value, status = 200) => {
      response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value));
    };
    if (headers.origin !== appOrigin) return json({ error: "unexpected origin" }, 403);
    response.setHeader("access-control-allow-origin", appOrigin);
    if (method === "OPTIONS") {
      response.setHeader("access-control-allow-methods", allowed);
      response.setHeader("access-control-allow-headers", "authorization,content-type,x-build-priority,x-build-request-id,x-build-session,x-build-subscription");
      response.writeHead(204); return response.end();
    }
    if (headers.authorization !== "Bearer fixture-bearer") return json({ error: "unauthorized" }, 401);
    if (method === "POST" && url === "/v1/sessions") {
      const id = `session-${openings.size + 1}`;
      openings.add(id);
      return json({ session_id: id, opening_generation: id, greeting,
        grant_expires_at: new Date(Date.now() + 30_000).toISOString(), transport: { ...manifest.transport, terminal: "webrtc", durable_events: false } }, 201);
    }
    if (method === "DELETE") {
      streams.get(url.split("/").at(-1))?.end();
      response.writeHead(204); return response.end();
    }
    const session = headers["x-build-session"];
    if (!openings.has(session)) return json({ error: "unknown opening" }, 401);
    if (url === "/v1/subscriptions") return json({ subscription_id: session, opening_generation: session,
      events_path: `/v1/subscriptions/${session}/events` }, 201);
    if (url === `/v1/subscriptions/${session}/events`) {
      streams.set(session, response);
      response.writeHead(200, { "content-type": "text/event-stream" });
      return response.write(hello(session)); // No application pushes.
    }
    if (headers["x-build-subscription"] !== session) return json({ error: "missing binding" }, 409);
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
    if (url === `${prefix}/read-cursor` && method !== "PUT") return json({ error: "wrong method" }, 405);
    const result = url === "/v1/session-greetings" ? greeting : handlers[url]?.(body, session);
    if (!result) return json({ error: "unexpected fixture request" }, 404);
    return json({ ok: true, bridge_epoch: "epoch", result });
  });
  await new Promise((resolve, reject) => server.once("error", reject).listen(0, "127.0.0.1", resolve));
  return { origin: `https://127.0.0.1:${server.address().port}`, seen, requests, allowApp: (origin) => { appOrigin = origin; }, close: async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  } };
}

async function mountProject({ page, basePath }, fixture) {
  const appOrigin = new URL(page.url()).origin;
  fixture.allowApp(appOrigin);
  // Older Chromium releases can access loopback without this permission.
  await page.context().grantPermissions(["local-network-access"], { origin: appOrigin }).catch((error) => {
    if (!String(error).includes("Unknown permission")) throw error;
  });
  await mountLayout(page, shellHtml("desktop"), { basePath, styles: SHELL_STYLES });
  await loadChatOverviewModules(page, basePath, {
    endpoint: "src/core/httpBrowserEndpoint.js", routes: "src/core/httpRoutes.js", session: "src/core/httpSession.js",
    contexts: "src/core/deviceContexts.js", api: "src/core/bridgeApi/index.js", rosters: "src/core/projectAgentsCache.js",
    inbox: "src/core/inbox.js", projectAgent: "src/core/inboxProjectAgent.js",
  });
  await page.evaluate(async ({ origin, manifest }) => {
    const { app, cache, contexts, api, endpoint, routes, session, rosters, feed, inbox, projectAgent, rail } = window.__layoutModules;
    const deviceId = "https-device", projectId = "project", entityId = "project-owner";
    const address = (kind, entityId = "") => ({ deviceId, entityId, kind });
    app.App.devices = [{ id: deviceId, name: "Fixture computer" }];
    app.App.selectedDeviceId = deviceId;
    app.App.route = { name: "project", deviceId, projectId, tab: "changes" };
    const bridge = await session.openHttpSession({ endpoint: endpoint.createBrowserHttpEndpoint({ grant: {
      identity: { origin, deviceId }, token: () => "fixture-bearer", expiresAt: () => Date.now() + 300_000, close() {},
    } }), routes: routes.createHttpRoutes(manifest) });
    const context = contexts.adoptDeviceSession(bridge);
    const selection = api.selectAdapter(await bridge.rawCall("session.hello"));
    const adapter = bridge.installAdapter(selection);
    contexts.adoptBridgeSelection(context, selection, adapter);
    bridge.allowRefresh();
    await cache.writeCached(cache.DEVICES_ADDRESS, app.App.devices);
    await cache.writeCached(address("projects"), [{ id: projectId, name: "Build", entity_id: entityId, deviceId, projectKey: `${deviceId}/${projectId}` }]);
    await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
    await rosters.syncProjectAgents({ deviceId, entityId, capabilities: adapter.capabilities, call: context.rpc });
    feed.subscribeFeed((view) => {
      const entries = projectAgent.projectAgentEntries(view.projects, view.items, view.runs);
      document.querySelector("#inbox-rail").innerHTML = entries.map((entry) => inbox.inboxRowHtml(entry)).join("");
    });
    await feed.startFeed();
    rail.mountAgentRail(document.querySelector("#agent-rail"), {
      kind: "project", deviceId, projectId, entityId, cacheScope: context.cacheScope, chatRepository: context.chatRepository,
    });
  }, { origin: fixture.origin, manifest });
}

it("clears the HTTPS project tile from a viewed thread and a fresh pull without application pushes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "build-https-project-unread-"));
  let fixture;
  let layout;
  const browserErrors = [];
  try {
    fixture = await serveProject(directory);
    layout = await openLayoutSession();
    const badge = ".inbox-project-agent .inbox-status-unread";
    const pin = '[data-bubble="agent"][data-agent="pin"]';
    const count = `${pin} .rail-count:not([hidden])`;
    // The self-signed certificate belongs only to the loopback fixture.
    await layout.withPage(async (view) => {
      const { page } = view;
      page.on("console", (message) => { if (message.type() === "error") browserErrors.push(message.text()); });
      page.on("requestfailed", (request) => browserErrors.push(`${request.url()} ${request.failure()?.errorText}`));
      await mountProject(view, fixture);
      await page.locator(badge).waitFor({ timeout: 5000 });
      expect(await page.locator(count).textContent({ timeout: 5000 })).toBe("1");
      await page.locator(pin).click({ timeout: 5000 });
      await page.getByText("Ready for review", { exact: true }).waitFor({ timeout: 5000 });
      await page.waitForFunction(async () => {
        const roster = await window.__layoutModules.cache.readCached({ deviceId: "https-device", entityId: "project-owner", kind: "project-agents" });
        return roster?.value.agents[0].unread_count === 0;
      }, null, { timeout: 5000 });
      await page.locator(badge).waitFor({ state: "detached", timeout: 5000 });
      await page.locator(count).waitFor({ state: "detached", timeout: 5000 });
      expect(fixture.seen).toEqual([{ session: "session-1", body: {
        agent_id: "pin", thread_id: "thread", read_from_sequence: 1, read_through_sequence: 1,
      } }]);
    }, { ignoreHTTPSErrors: true });
    await layout.withPage(async (view) => {
      await mountProject(view, fixture);
      await view.page.locator(".inbox-project-agent").waitFor({ timeout: 5000 });
      await view.page.locator(pin).waitFor({ timeout: 5000 });
      expect(await view.page.locator(badge).count()).toBe(0);
      expect(await view.page.locator(count).count()).toBe(0);
      expect(fixture.requests).toContainEqual({ method: "POST", url: `${prefix}/agent-queries`, session: "session-2" });
      expect(fixture.seen).toHaveLength(1);
    }, { ignoreHTTPSErrors: true });
  } catch (error) {
    throw new Error(`${error}\nBrowser: ${browserErrors.join(" | ")}\nRequests: ${JSON.stringify(fixture?.requests)}`, { cause: error });
  } finally {
    await layout?.close();
    await fixture?.close();
    await rm(directory, { recursive: true, force: true });
  }
}, 45_000);
