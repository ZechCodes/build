import manifest from "../../../fixtures/api/http/resource-manifest.json";
import { createBrowserHttpEndpoint } from "../../src/core/httpBrowserEndpoint.js";
import { createHttpRoutes } from "../../src/core/httpRoutes.js";

// Only the bridge and its clock are simulated. Each snapshot request costs
// nine seconds on the lease clock, below the 12 s HTTPS request deadline.
export async function httpsDeviceSnapshot() {
  const origin = "https://127.0.0.1:8443";
  const routes = createHttpRoutes(manifest);
  const scope = { kind: "device" };
  const base = { scope, incarnation: "incarnation-paint", bridge_epoch: "epoch-paint" };
  const counts = { pages: 0, returned: 0, expiries: 0, overBudget: 0 };
  let now = 0, head = 1, expires = 0;
  const records = Array.from({ length: 3500 }, (_, n) => ({
    kind: "identity", key: `agent-${n}`, revision: "1",
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
    return { ...base, status: "ok", watermark: String(head), cursor: `cursor-${head}`, consistency: "leased",
      lease: { id: "lease", expires_at_ms: expires }, records: records.slice(offset, end),
      ...(end < size && { next_page: String(end) }) };
  }
  const endpoint = createBrowserHttpEndpoint({
    grant: { identity: { origin, deviceId: "device-paint-997" }, token: () => "fixture", expiresAt: () => Date.now() + 300_000, close() {} },
    fetchImpl: async (url, { method, body }) => {
      const target = new URL(url);
      if (target.origin !== origin) throw new Error("Unexpected origin");
      if (method === "DELETE") return new Response(null, { status: 204 });
      if (target.pathname === "/v1/sessions") return Response.json({
        session_id: "session", opening_generation: "opening", grant_expires_at: new Date(Date.now() + 30_000).toISOString(),
      });
      return Response.json({ ok: true, result: query(target.pathname, JSON.parse(body)) });
    },
  });
  const opening = await endpoint.open(">=3.34.0 <4.0.0");
  return {
    request: async (method, params) => (await endpoint.request(opening, routes.request(method, params), {})).result,
    advanceThreeDays() { now += 3 * 86_400_000; head += 3600; },
    counts, close: () => endpoint.close(),
  };
}
