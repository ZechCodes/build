import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { createBridgeHttp } from "../src/bridge-http.mjs";
import { createDesktopHttpEndpoint } from "../../spa/src/core/httpDesktopEndpoint.js";
import { openHttpSession } from "../../spa/src/core/httpSession.js";
import { createHttpRoutes } from "../../spa/src/core/httpRoutes.js";

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
function harness(t, fixture, { badProof = false, autoAck = true } = {}) {
  const trace = [];
  const output = [];
  const listeners = new Set();
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
      for (const listener of listeners) listener(event);
      if (autoAck && event.event !== "reset_required") bridge.ack(owner, event.stream, event.sequence);
    },
  });
  t.after(() => bridge.clear());
  return { bridge, owner, output, trace, listeners, heartbeat: () => heartbeat };
}

// The actual SPA desktop endpoint consumes the native module's IPC envelopes.
// Only Electron's process hop is local here; every network response is real.
function nativeEndpoint(h, identity) {
  const invoke = (name) => async (...args) => {
    try { return { ok: true, value: await h.bridge[name](h.owner, ...args) }; }
    catch (error) { return { ok: false, error: { code: error.code, uncertain: error.uncertain } }; }
  };
  const api = Object.fromEntries(["open", "subscribe", "request", "cancel", "ack", "unsubscribe", "close"].map((name) => [name, invoke(name)]));
  api.onEvent = (listener) => { h.listeners.add(listener); return () => h.listeners.delete(listener); };
  return createDesktopHttpEndpoint({ api, identity });
}
// The fixture is seeded by scripts/bridge-http-qualification/run.py using a disposable
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
  });
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

test("desktop refuses a genuinely revoked credential with account-verification guidance", qualification, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture);
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const revoked = await globalThis.fetch(`${fixture.hostedOrigin}/api/devices/${fixture.deviceId}/http-credentials/${identity.familyId}`, {
    method: "DELETE", headers: { Authorization: `Bearer ${fixture.accessToken}` },
  });
  assert.equal(revoked.status, 204);
  const endpoint = nativeEndpoint(h, identity);
  t.after(() => endpoint.close());
  await assert.rejects(endpoint.open("^3.0.0"), (error) => {
    assert.equal(error.code, "unauthorized");
    assert.match(error.message, /authorization was refused.*Verify this device again/);
    return true;
  });
  assert.ok(h.trace.some((entry) => entry.path === "/v1/sessions" && entry.status === 401));
  assert.equal(h.output.some((entry) => entry.event === "hello"), false);
  t.diagnostic("real issuer family DELETE=204 → real bridge POST /v1/sessions=401 → plain desktop authorization refusal");
});

test("desktop ninth family stream is refused by the real bridge and ends in a plain client error", qualification, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture, { autoAck: false });
  const first = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const { handle: _handle, ...hint } = first;
  for (let index = 0; index < 8; index++) {
    const identity = index ? await h.bridge.resume(h.owner, hint) : first;
    await h.bridge.open(h.owner, identity.handle, "^3.0.0");
    const subscription = await h.bridge.subscribe(h.owner, identity.handle, { kind: "application" }, { requestId: randomUUID() });
    await eventually(() => h.output.some((entry) => entry.stream === subscription.stream && entry.event === "hello"), "one of the eight real admitted streams");
    const hello = h.output.find((entry) => entry.stream === subscription.stream && entry.event === "hello");
    h.bridge.ack(h.owner, hello.stream, hello.sequence);
  }
  const ninth = await h.bridge.resume(h.owner, hint);
  const endpoint = nativeEndpoint(h, ninth);
  t.after(() => endpoint.close());
  let message;
  await assert.rejects(openHttpSession({ endpoint, routes: createHttpRoutes(fixture.manifest) }), (error) => {
    assert.equal(typeof error.code, "string");
    assert.ok(["scope_ended", "stream_reset", "protocol", "busy"].includes(error.code), error.code);
    assert.match(error.message, /bridge|HTTPS/i);
    assert.doesNotMatch(error.message, /[{}]|undefined|invalid_stream/);
    message = error.message;
    return true;
  });
  assert.equal(h.trace.filter((entry) => entry.path.endsWith("/events") && entry.status === 200).length, 8);
  assert.equal(h.trace.filter((entry) => entry.path.endsWith("/events") && entry.status === 429).length, 1);
  assert.equal(h.output.filter((entry) => entry.event === "hello").length, 8);
  t.diagnostic(JSON.stringify({ refusal: "ninth family stream", status: 429, message }));
});
