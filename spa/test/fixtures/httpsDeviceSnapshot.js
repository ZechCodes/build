import manifest from "../../../fixtures/api/http/resource-manifest.json";
import { createBrowserHttpEndpoint } from "../../src/core/httpBrowserEndpoint.js";
import { openHttpSession } from "../../src/core/httpSession.js";
import { createHttpRoutes } from "../../src/core/httpRoutes.js";

// Only the bridge and its clock are simulated. Each snapshot request costs
// nine seconds on the lease clock, below the 12 s HTTPS request deadline.
export function httpsDeviceSnapshot(size = 3500, { afterPage } = {}) {
  const origin = "https://127.0.0.1:8443";
  const routes = createHttpRoutes(manifest);
  const scope = { kind: "device" };
  const base = { scope, incarnation: "incarnation-paint", bridge_epoch: "epoch-paint" };
  const greeting = { api_version: "3.34.0", capabilities: ["bridge.epoch", "changes.subscriptions", "sync.device", "sync.task", "sync.projectTasks"],
    bridge_epoch: base.bridge_epoch, journal_incarnation: base.incarnation };
  const counts = { pages: 0, returned: 0, expiries: 0, overBudget: 0 };
  let now = 0, head = 1, expires = 0;
  const elapse = (ms) => { now += ms; };
  const records = Array.from({ length: size }, (_, n) => ({
    kind: "identity", key: `agent-${n}`, revision: String(n + 1),
    row: { agent_id: `agent-${n}`, name: `Agent ${n}`, provider: "codex", workspace_id: null },
  }));
  function query(path, params) {
    if (path === "/v1/sync/change-queries") {
      if (params.cursor !== `cursor-${head}`) {
        counts.overBudget++;
        return { ...base, status: "resync_required", reason: "over_budget" };
      }
      return { ...base, status: "ok", cursor: params.cursor, head: String(head), changes: [] };
    }
    if (path !== "/v1/sync/snapshot-queries") throw new Error("Unexpected resource");
    if (!params.page) expires = now + 300_000;
    counts.pages++;
    now += 9_000;
    if (now >= expires) {
      counts.expiries++;
      return { ...base, status: "resync_required", reason: "lease_expired" };
    }
    const offset = Number(params.page || 0), size = head === 1 ? 1 : records.length;
    const end = Math.min(size, offset + (params.limit ?? 100));
    counts.returned += end - offset;
    afterPage?.({ offset, end, elapse });
    return { ...base, status: "ok", watermark: String(head), cursor: `cursor-${head}`, consistency: "leased",
      lease: { id: "lease", expires_at_ms: expires }, records: records.slice(offset, end),
      ...(end < size && { next_page: String(end) }) };
  }
  async function openSession() {
    const sessionId = crypto.randomUUID();
    const grant = { identity: { origin, deviceId: "device-paint-997" }, token: () => "fixture",
      expiresAt: () => Date.now() + 300_000, close() {} };
    const fetchImpl = async (url, { method, body }) => {
      const target = new URL(url);
      if (target.origin !== origin) throw new Error("Unexpected origin");
      const path = target.pathname;
      if (method === "DELETE") return new Response(null, { status: 204 });
      if (path === "/v1/sessions" || method === "PATCH") return Response.json({
        session_id: sessionId, opening_generation: sessionId, grant_expires_at: new Date(Date.now() + 30_000).toISOString(),
        greeting, transport: manifest.transport,
      });
      const binding = `binding-${sessionId}`, events = `/v1/subscriptions/${binding}/events`;
      if (path === "/v1/subscriptions") return Response.json({ subscription_id: binding, opening_generation: sessionId, events_path: events });
      if (path === events) return eventStream({ subscription_id: binding, opening_generation: sessionId, bridge_epoch: base.bridge_epoch,
        durable: false, recovery: "current_state_refresh" });
      let result = {};
      if (path === "/v1/session-greetings") result = greeting;
      if (path === "/v1/sync/head-queries") result = { ...base, status: "ok", head: String(head), retention_floor: "0" };
      else if (path.startsWith("/v1/sync/")) result = query(path, JSON.parse(body));
      return Response.json({ ok: true, result, bridge_epoch: base.bridge_epoch });
    };
    return openHttpSession({ endpoint: createBrowserHttpEndpoint({ grant, fetchImpl }), routes });
  }
  return { openSession, advanceThreeDays() { now += 3 * 86_400_000; head += 3600; }, counts };
}

function eventStream(hello) {
  return new Response(new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode(`event: hello\ndata: ${JSON.stringify(hello)}\n\n`));
    },
  }), { headers: { "content-type": "text/event-stream" } });
}
