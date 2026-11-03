import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { createBridgeHttp } from "../src/bridge-http.mjs";

const fixturePath = process.env.BUILD_HTTP_QUALIFICATION_FIXTURE;
const qualification = { skip: !fixturePath, timeout: 75_000 };

async function loadFixture() {
  const fixture = JSON.parse(await readFile(fixturePath, "utf8"));
  for (const origin of [fixture.hostedOrigin, fixture.bridgeOrigin]) {
    const url = new URL(origin);
    assert.equal(url.protocol, "https:");
    assert.ok(["localhost", "127.0.0.1", "[::1]"].includes(url.hostname), "qualification only permits loopback");
  }
  assert.equal(typeof fixture.accessToken, "string");
  fixture.manifest = JSON.parse(await readFile(new URL("../src/bridge-http-resources.json", import.meta.url), "utf8"));
  return fixture;
}
async function eventually(predicate, description, timeout = 35_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.fail(`Timed out waiting for ${description}`);
}
function harness(t, fixture, { badProof = false } = {}) {
  const trace = [];
  const output = [];
  const owner = { id: randomUUID() };
  let heartbeat = false;
  let bridge;
  const fetch = async (input, options) => {
    const url = new URL(input);
    assert.ok([fixture.hostedOrigin, fixture.bridgeOrigin].includes(url.origin), "every request remains on fixture origins");
    const authorization = new Headers(options.headers).get("Authorization");
    if (url.origin === fixture.hostedOrigin) assert.equal(authorization, `Bearer ${fixture.accessToken}`);
    else if (url.pathname === "/.well-known/build-bridge") assert.equal(authorization, null);
    else {
      assert.ok(authorization?.startsWith("Bearer "));
      assert.notEqual(authorization, `Bearer ${fixture.accessToken}`, "account credential never reaches bridge");
    }
    const response = await globalThis.fetch(input, options);
    trace.push({ path: url.pathname, method: options.method, status: response.status });
    if (url.pathname.endsWith("/.well-known/build-bridge") && badProof) {
      const value = await response.json();
      const parts = value.proof.split(".");
      parts[2] = (parts[2][0] === "A" ? "B" : "A") + parts[2].slice(1);
      return new Response(JSON.stringify({ ...value, proof: parts.join(".") }), {
        status: response.status, headers: response.headers,
      });
    }
    if (url.pathname.endsWith("/events")) {
      let text = "";
      const observer = new TransformStream({
        transform(chunk, controller) {
          text = (text + new TextDecoder().decode(chunk)).slice(-100);
          if (text.includes(": keepalive\n\n")) heartbeat = true;
          controller.enqueue(chunk);
        },
      });
      return new Response(response.body.pipeThrough(observer), {
        status: response.status, headers: response.headers,
      });
    }
    return response;
  };
  bridge = createBridgeHttp({
    hostedOrigin: fixture.hostedOrigin, fetch,
    getAuth: () => ({ generation: 1, accessToken: fixture.accessToken }),
    isOwnerCurrent: (candidate) => candidate === owner,
    manifest: fixture.manifest,
    send: (recipient, event) => {
      assert.equal(recipient, owner);
      output.push(event);
      if (event.event !== "reset_required") bridge.ack(owner, event.stream, event.sequence);
    },
  });
  t.after(() => bridge.clear());
  return { bridge, owner, output, trace, heartbeat: () => heartbeat };
}
// The fixture is seeded by scripts/qualify-http-clients.py using a disposable
// local Skrift account, paired identity and cargo-built loopback bridge.
test("desktop real REST lifecycle: signed discovery, credential, SSE, renewal and close", qualification, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture);
  const metadata = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  assert.equal(metadata.deviceId, fixture.deviceId);
  const opening = await h.bridge.open(h.owner, metadata.handle, "^3.0.0");
  assert.ok(opening.session_id);
  const subscription = await h.bridge.subscribe(h.owner, metadata.handle,
    { kind: "application" }, { requestId: randomUUID() });
  await eventually(() => h.output.some((event) => event.event === "hello"), "real SSE hello");
  const changed = await h.bridge.request(h.owner, metadata.handle, "changes_subscribe", {
    subscription_id: `desktop-${randomUUID()}`, scope: { kind: "all" },
    kinds: ["state"], mode: "realtime", priority: "foreground",
  });
  assert.equal(changed.ok, true, JSON.stringify(changed));
  const project = await h.bridge.request(h.owner, metadata.handle, "project_create", {
    name: `desktop-qualification-${randomUUID()}`, parent: fixture.projectsDir,
  }, { write: { operation_id: randomUUID() } });
  assert.equal(project.ok, true, JSON.stringify(project));
  await eventually(() => h.output.some((event) => event.event === "event" &&
    event.data.type === "changes" &&
    event.data.items?.some((item) => item.state?.projects?.some((row) => row.project_id === project.result.project_id))), "live SSE state containing the created project");
  await eventually(h.heartbeat, "real SSE heartbeat");
  await eventually(() => h.trace.some((request) => request.method === "PATCH" && request.path === `/v1/sessions/${opening.session_id}` && request.status === 200), "successful automatic PATCH renewal");
  await eventually(() => Date.now() > Date.parse(opening.grant_expires_at) + 100, "original grant deadline to pass after renewal");
  const projects = await h.bridge.request(h.owner, metadata.handle, "project_list");
  assert.equal(projects.ok, true, JSON.stringify(projects));
  assert.ok(JSON.stringify(projects).includes(project.result.project_id));
  await h.bridge.unsubscribe(h.owner, subscription.stream);
  await h.bridge.close(h.owner, metadata.handle);
  assert.ok(h.trace.some((request) => request.method === "DELETE" && request.path === `/v1/sessions/${opening.session_id}` && request.status === 204));
  await assert.rejects(h.bridge.request(h.owner, metadata.handle, "project_list"), { code: "invalid_handle" });
  t.diagnostic(JSON.stringify({ flow: "discovery → credential → open → live SSE → heartbeat → PATCH renewal → close", trace: h.trace, events: h.output.map((event) => ({ event: event.event, type: event.data?.type })) }));
});

test("desktop refuses a tampered real discovery proof with a plain error before opening", qualification, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture, { badProof: true });
  await assert.rejects(h.bridge.discover(h.owner, fixture.bridgeOrigin), {
    code: "discovery_failed",
    message: "Build could not verify this HTTPS address. Check the address and try again.",
  });
  assert.ok(h.trace.some((request) => request.path.endsWith("/complete") && request.status >= 400));
  assert.equal(h.trace.some((request) => request.path.startsWith("/v1/")), false);
});
