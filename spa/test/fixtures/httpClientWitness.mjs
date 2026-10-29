import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { createBridgeDiscovery } from "../../src/core/httpDiscovery.js";
import { createBrowserHttpEndpoint } from "../../src/core/httpBrowserEndpoint.js";
import { createHttpRoutes } from "../../src/core/httpRoutes.js";
import { openHttpSession } from "../../src/core/httpSession.js";

const [origin, mode] = process.argv.slice(2);
if (mode === "untrusted") {
  const endpoint = createBrowserHttpEndpoint({ grant: {
    identity: { origin, deviceId: "device" }, token: () => "fixture-device-bearer", expiresAt: () => Date.now() + 300000, close() {},
  } });
  await assert.rejects(endpoint.open("range"), { code: "network" });
  endpoint.close();
  console.log("untrusted TLS refused");
  process.exit(0);
}
const routes = createHttpRoutes({ version: 1, operations: [
  { method: "ping", operationId: "ping", verb: "GET", path: "/v1/health", pathParams: [], queryParams: [], body: "remaining" },
  { method: "tasks.update", operationId: "update", verb: "PATCH", path: "/v1/tasks/{task_id}", pathParams: ["task_id"], queryParams: [], body: "remaining" },
] });
const discovery = createBridgeDiscovery({ issuer: origin, accountId: "account" });
let received = 0;
for (let attempt = 0; attempt < 4; attempt += 1) {
  assert.equal(await discovery.begin(origin), origin + "/app/?bridge_discovery=challenge");
  const grant = await discovery.complete("challenge");
  const endpoint = createBrowserHttpEndpoint({ grant });
  if (attempt === 1) {
    await assert.rejects(openHttpSession({ endpoint, routes }));
    await delay(75); // Bounded exact-opening DELETE is an independent request.
    continue;
  }
  const session = await openHttpSession({ endpoint, routes, onPush: () => { received += 1; } });
  session.allowRefresh();
  if (attempt === 0) {
    assert.equal((await session.rawCall("tasks.update", { task_id: "t", title: "success" })).effects, 1);
    await assert.rejects(session.rawCall("tasks.update", { task_id: "t", lose: true }), { uncertain: true });
    assert.equal((await session.rawCall("ping")).effects, 2);
  }
  session.close();
  await delay(75);
}
assert.equal(received, 1);
console.log("trusted HTTPS witness passed");
