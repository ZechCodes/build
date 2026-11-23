import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { createBrowserHttpEndpoint } from "../../src/core/httpBrowserEndpoint.js";
import { createHttpRoutes } from "../../src/core/httpRoutes.js";
import { openHttpSession } from "../../src/core/httpSession.js";

const [origin, entityId, agentId] = process.argv.slice(2);
assert.equal(new URL(origin).hostname, "localhost");
const routes = createHttpRoutes(JSON.parse(readFileSync(
  new URL("../../../fixtures/api/http/resource-manifest.json", import.meta.url), "utf8",
)));
const grant = {
  identity: { origin, deviceId: "device-1" },
  token: () => "account-bound-token",
  expiresAt: () => Date.now() + 300_000,
  close() {},
};
const endpoint = createBrowserHttpEndpoint({ grant, fetchImpl(url, options) {
  return fetch(url, { ...options, headers: { ...options.headers, Origin: "https://app.example" } });
} });

const threadSubscription = "s-thread";
let threadTip = 0;
let inboxPushes = 0;
const losses = [];
const session = await openHttpSession({
  endpoint, routes, onLost: (error) => losses.push(error),
  onPush(frame) {
    if (frame.type !== "changes") return;
    if (frame.subscription_id === "s-inbox" && frame.items.some((item) => item.entity_id === entityId)) inboxPushes += 1;
    if (frame.subscription_id !== threadSubscription) return;
    const tips = frame.items.find((item) => item.entity_id === entityId)?.thread || [];
    threadTip = Math.max(threadTip, ...tips.filter((tip) => tip.agent_id === agentId).map((tip) => tip.last_sequence));
  },
});
const subscribe = (subscription_id, scope, kinds, mode = "realtime", priority = "foreground") =>
  session.rawCall("changes.subscribe", { subscription_id, scope, kinds, mode, priority });
const unsubscribe = (subscription_id) => session.rawCall("changes.unsubscribe", { subscription_id });
const watchThread = () => subscribe(threadSubscription, { kind: "entity", id: entityId }, ["thread"]);
const savedMessage = async (body) => {
  const until = performance.now() + 3_000;
  do {
    const page = await session.rawCall("thread.page", { entity_id: entityId, agent_id: agentId });
    const message = page.items.find((item) => item.data?.body === body);
    if (message) return message.data.sequence;
    await delay(25);
  } while (performance.now() < until);
  assert.fail(`The REST read never found "${body}"`);
};

async function publishAndWaitForPush(body) {
  console.log(`publish ${body}`);
  const sequence = await savedMessage(body);
  const until = performance.now() + 1_000;
  while (threadTip < sequence && performance.now() < until) await delay(25);
  return { sequence, delivered: threadTip >= sequence };
}

try {
  const greeting = await session.rawCall("session.hello", {
    client: { name: "spa", version: "fixture", api_range: ">=3.24.0 <4.0.0" }, changes: "subscriptions",
  });
  assert.equal(greeting.push_events, true);
  session.allowRefresh();
  const all = { kind: "all" };
  await subscribe("compaction-observations", all, ["compaction.changed"], "realtime", "background");
  await subscribe("s-inbox", all, ["state"]);
  await subscribe("s-background", all, ["terminals"], { batch_ms: 30_000 }, "background");

  await watchThread();
  const initial = await publishAndWaitForPush("before changing views");
  assert.equal(initial.delivered, true, "the thread push reaches this HTTPS client before the slots fill");
  console.log(`Thread push received before the slots fill at sequence ${initial.sequence}`);
  await unsubscribe(threadSubscription);

  // Device sync holds one terminal subscription per registered project source.
  const { projects } = await session.rawCall("project.list");
  const sources = projects.flatMap(({ project_id, sources }) => sources.map(({ id, path }) => ({
    project_id, source_id: id, source_path: path,
  })));
  assert.equal(sources.length, 5);
  for (const [index, source] of sources.entries()) {
    await subscribe(`s-project-term:${index}`, { kind: "project_source", ...source }, ["terminals"]);
  }
  const refused = await watchThread().then(() => null, (error) => error);
  const previousInboxPushes = inboxPushes;
  const stalled = await publishAndWaitForPush("with all project terminal subscriptions held");
  const inboxDelivered = inboxPushes > previousInboxPushes;
  const outcome = refused ? `${refused.error_code} (${refused.message})` : "accepted";
  console.log(`Thread subscribe returned ${outcome}`);
  console.log(`REST saw sequence ${stalled.sequence}. Thread push delivered = ${stalled.delivered}. Inbox push delivered = ${inboxDelivered}.`);
  assert.equal(inboxDelivered, true, "the same mutation still sends an inbox push over the same SSE stream");
  assert.deepEqual(losses, [], "the application event stream is still open");

  await unsubscribe("s-project-term:0");
  await watchThread();
  const restored = await publishAndWaitForPush("after releasing one slot");
  assert.equal(restored.delivered, true, "freeing one slot restores push on the same opening");
  console.log(`Thread push received after releasing one slot at sequence ${restored.sequence}`);

  assert.equal(stalled.delivered, true, "a mounted thread should receive live updates while project terminal subscriptions are held");
} finally {
  session.close();
}
