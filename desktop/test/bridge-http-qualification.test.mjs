import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdir, open, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";
import { createBridgeHttp } from "../src/bridge-http.mjs";
import { createDesktopHttpEndpoint } from "../../spa/src/core/httpDesktopEndpoint.js";
import { openHttpSession } from "../../spa/src/core/httpSession.js";
import { createHttpRoutes } from "../../spa/src/core/httpRoutes.js";

const fixturePath = process.env.BUILD_HTTP_QUALIFICATION_FIXTURE;
const qualification = { skip: !fixturePath, timeout: 75_000 };
const terminalGate = process.env.BUILD_HTTP_QUALIFICATION_TERMINAL;

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
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.fail(`Timed out waiting for ${description}`);
}
function harness(t, fixture, { badProof = false, autoAck = true, observeBearer = () => {}, observeTicket } = {}) {
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
    if (url.origin === fixture.hostedOrigin) assert.ok(authorization === `Bearer ${fixture.accessToken}`, "issuer receives its own credential");
    else if (url.pathname === "/.well-known/build-bridge") assert.equal(authorization, null);
    else {
      assert.ok(authorization?.startsWith("Bearer "));
      observeBearer(authorization);
      assert.ok(authorization !== `Bearer ${fixture.accessToken}`, "account credential never reaches bridge");
    }
    const response = await globalThis.fetch(input, options);
    trace.push({ path: url.pathname, method: options.method, status: response.status });
    if (observeTicket && url.pathname === "/v1/terminal-tickets" && response.status === 201) {
      observeTicket((await response.clone().json()).ticket);
    }
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

test("terminal qualification preserves WebRTC for older desktop IPC", {
  ...qualification, skip: !fixturePath || !terminalGate,
}, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture);
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const endpoint = nativeEndpoint(h, identity);
  let opening;
  const session = await openHttpSession({
    endpoint: {
      ...endpoint,
      async open(apiRange) {
        opening = await endpoint.open(apiRange);
        return opening;
      },
    },
    routes: createHttpRoutes(fixture.manifest),
  });
  t.after(() => session.close());
  assert.equal(opening.transport.terminal, "webrtc");
  assert.equal(Object.hasOwn(opening.transport, "terminal_websocket"), terminalGate !== "off");
  assert.equal(session.terminalTransport, "webrtc");
  assert.equal(h.trace.some((entry) => entry.path === "/v1/terminal-tickets"), false);
  t.diagnostic('Opening declares "terminal":"webrtc" and the legacy IPC surface selects it');
});

test("SSE redaction catches terminal output inside an ordinary state event", () => {
  const sentinel = "wss-native-qualification";
  const output = [{ event: "push", data: { type: "state", items: [{ text: sentinel }] } }];
  assert.throws(() => assertSseRedacts(output, new Set([sentinel])), {
    message: "SSE must redact credentials, tickets and terminal output",
  });
});

test("terminal qualification uses native WSS beside real REST and SSE", {
  ...qualification, skip: !fixturePath || terminalGate !== "on",
}, async (t) => {
  const fixture = await loadFixture();
  const secrets = new Set([fixture.accessToken, "wss-native-qualification"]);
  const h = harness(t, fixture, {
    observeBearer: (bearer) => secrets.add(bearer.slice("Bearer ".length)),
    observeTicket: (ticket) => secrets.add(ticket),
  });
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const opening = await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  assert.ok(opening.greeting.capabilities.includes("term.websocket"));
  assert.deepEqual(opening.transport.terminal_websocket, { version: 1, protocol: "build.term.v1" });
  await h.bridge.subscribe(h.owner, identity.handle, { kind: "application" }, { requestId: randomUUID() });
  await eventually(() => h.output.some((event) => event.event === "hello"), "native SSE hello");
  const changes = await h.bridge.request(h.owner, identity.handle, "changes_subscribe", {
    subscription_id: `native-${randomUUID()}`, scope: { kind: "all" },
    kinds: ["state"], mode: "realtime", priority: "foreground",
  });
  assert.equal(changes.ok, true);
  const events = [];
  const { terminal } = await h.bridge.terminalOpen(h.owner, identity.handle, (event) => events.push(event));
  const created = await h.bridge.request(h.owner, identity.handle, "project_create", {
    name: `native-wss-${randomUUID()}`, parent: fixture.projectsDir,
  });
  assert.equal(created.ok, true, created.error);
  const scope = { project_id: created.result.project_id };
  await eventually(() => h.output.some((event) => event.data?.items?.some((item) =>
    item.state?.projects?.some((row) => row.project_id === scope.project_id))), "SSE state while WSS is open");
  const stopAgent = await exerciseNativeAgent(h, identity.handle, terminal, scope, events);
  await exerciseNativeShell(h, terminal, scope, events);
  await exerciseNativeSource(h, fixture, identity.handle, terminal, scope);
  await stopAgent();
  assert.equal(h.trace.filter((entry) => entry.path === "/v1/terminal-tickets" && entry.status === 201).length, 1);
  assert.equal(h.output.some((event) => event.data?.type?.startsWith("term.")), false);
  assertSseRedacts(h.output, secrets);
  await assertShippedLogsRedact(fixture, secrets, h.trace);
  t.diagnostic("native TLS WSS agent attach and shell create, attach, resize, input, output and close beside REST and SSE");
});

function assertSseRedacts(output, secrets) {
  assertRedacts([JSON.stringify(output)], secrets, "SSE must redact credentials, tickets and terminal output");
}

async function assertShippedLogsRedact(fixture, secrets, trace) {
  assert.ok(secrets.size >= 4, "issuer bearer, bridge bearer, ticket and terminal output were observed");
  const logs = await Promise.all(["issuer.log", "bridge.log"].map((name) =>
    readFile(resolve(fixture.runDir, name), "utf8")));
  logs.push(JSON.stringify(trace));
  assertRedacts(logs, secrets, "diagnostics must redact credentials, tickets and terminal output");
}

function assertRedacts(sources, secrets, message) {
  for (const value of secrets) {
    assert.ok(typeof value === "string" && value.length > 0, "the fixture observed a nonempty sentinel");
    assert.ok(sources.every((source) => !source.includes(value)), message);
  }
}

async function exerciseNativeAgent(h, handle, terminal, scope, events) {
  const rest = async (operation, params) => {
    const reply = await h.bridge.request(h.owner, handle, operation, params);
    assert.equal(reply.ok, true, operation + ": " + reply.error);
    return reply.result;
  };
  const workspace = await rest("workspace_create", { ...scope, name: "native-qualification-agent", isolation: "worktree" });
  const conversation = await rest("workspace_ensure_conversation", {
    workspace_id: workspace.workspace_id, provider: "codex",
  });
  const params = { id: conversation.run_id };
  const agent = await rest("agent_start", params);
  await eventually(async () => {
    const result = await h.bridge.terminalCall(h.owner, terminal, "agent.attach", params);
    assert.equal(result.ok, true, result.error);
    assert.equal(result.result.term_id, agent.term_id);
    return result.result.live;
  }, "started QA agent to expose a live screen", 5000);
  const wrongClass = await h.bridge.terminalCall(h.owner, terminal, "term.close", { term_id: agent.term_id });
  assert.equal(wrongClass.ok, false);
  assert.match(wrongClass.error, /cannot close an agent terminal/i);
  return async () => {
    // The fixture's QA harness reads stdin until EOF.
    await terminalResult(h, terminal, "term.input", { term_id: agent.term_id, data: "BA==" });
    await eventually(() => events.some(({ data }) => data?.type === "term.closed" && data?.term_id === agent.term_id),
      "QA agent terminal to close on EOF", 5000);
  };
}

async function exerciseNativeShell(h, terminal, scope, events) {
  const call = async (method, params, id) => {
    const reply = await h.bridge.terminalCall(h.owner, terminal, method, params, id);
    assert.equal(reply.ok, true, `${method}: ${reply.error_code}`);
    return reply.result;
  };
  const shell = await call("term.create", scope);
  const scoped = { ...scope, term_id: shell.term_id };
  const attachment = randomUUID();
  const attached = await call("term.attach", scoped, attachment);
  assert.equal(attached.term_id, shell.term_id);
  assert.equal(typeof attached.snapshot, "string");
  await call("term.resize", { term_id: shell.term_id, cols: 100, rows: 30 });
  // Split the marker in the input so a tty echo cannot satisfy the output check.
  const input = Buffer.from("printf '%s%s\\n' wss-native- qualification\n").toString("base64");
  await call("term.input", { term_id: shell.term_id, data: input });
  await eventually(() => outputFor(events, shell.term_id).includes("wss-native-qualification"), "native shell output");
  const cursor = events.findLast((event) => event.data?.type === "term.output")?.data.cursor;
  await call("term.ack", { term_id: shell.term_id, cursor });
  h.bridge.terminalDetach(h.owner, terminal, attachment);
  const detached = await h.bridge.terminalCall(h.owner, terminal, "term.input", { term_id: shell.term_id, data: "eA==" });
  assert.equal(detached.error_code, "invalid_params");
  const restored = await call("term.attach", scoped);
  assert.equal(restored.term_id, shell.term_id);
  assert.ok(restored.cursor >= cursor);
  await call("term.close", scoped);
}

async function exerciseNativeSource(h, fixture, handle, terminal, scope) {
  async function addSource(name) {
    const sourcePath = resolve(fixture.projectsDir, name);
    await mkdir(sourcePath, { mode: 0o700 });
    const added = await h.bridge.request(h.owner, handle, "project_add_source", { ...scope, path: sourcePath });
    assert.equal(added.ok, true, added.error_code);
    const source = added.result.added_sources.find(({ path }) => path === sourcePath);
    assert.ok(source, "the REST project row returned the added source");
    return source;
  }
  const source = await addSource("qualification-source");
  const sibling = await addSource("qualification-sibling");
  const scoped = { ...scope, source_id: source.id, source_path: source.path };
  const { term_id } = await terminalResult(h, terminal, "term.create", scoped);
  for (const method of ["term.attach", "term.close"]) {
    const wrongSource = await h.bridge.terminalCall(h.owner, terminal, method,
      { ...scope, term_id, source_id: sibling.id, source_path: sibling.path });
    assert.equal(wrongSource.error_code, "not_found", method);
  }
  const attached = await terminalResult(h, terminal, "term.attach", { ...scoped, term_id });
  assert.equal(attached.term_id, term_id);
  await terminalResult(h, terminal, "term.close", { ...scoped, term_id });
}

function outputFor(events, term) {
  return Buffer.concat(events.filter(({ data }) => data?.type === "term.output" && data.term_id === term)
    .map(({ data }) => Buffer.from(data.data, "base64"))).toString("utf8");
}

test("terminal qualification refuses a read-only grant at the real verifier", {
  ...qualification, skip: !fixturePath || terminalGate !== "on",
}, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture);
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  await restrictFixtureFamily(fixture, identity.familyId);
  await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  await assert.rejects(h.bridge.terminalOpen(h.owner, identity.handle, () => {}), {
    code: "forbidden_scope", status: 403,
  });
  assert.deepEqual(h.trace.filter(({ path }) => path === "/v1/terminal-tickets").map(({ status }) => status), [403]);
});

async function restrictFixtureFamily(fixture, family) {
  const database = await fixtureDatabase(fixture);
  try {
    const update = database.prepare("UPDATE bridge_http_families SET scope = ? WHERE id = ?");
    assert.equal(update.run("bridge:read", fixtureUuid(family)).changes, 1);
  } finally {
    database.close();
  }
}

async function fixtureDatabase(fixture) {
  assert.equal(resolve(fixture.runDir, "runtime.json"), resolve(fixturePath));
  const { DatabaseSync } = await import("node:sqlite");
  return new DatabaseSync(resolve(fixture.runDir, "app.db"));
}

function fixtureUuid(id) {
  // Skrift's SQLite UUID adapter stores the 16-byte value.
  return Buffer.from(id.replaceAll("-", ""), "hex");
}

test("terminal qualification binds tickets to the exact family and generation", {
  ...qualification, skip: !fixturePath || terminalGate !== "on",
}, async (t) => {
  const fixture = await loadFixture();
  let bearer;
  const h = harness(t, fixture, { observeBearer: (value) => { bearer = value; } });
  const first = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const opened = await h.bridge.open(h.owner, first.handle, "^3.0.0");
  const firstBearer = bearer;
  const second = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  await h.bridge.open(h.owner, second.handle, "^3.0.0");
  assert.ok(first.familyId !== second.familyId, "separately approved family");
  for (const [credential, generation] of [[bearer, opened.opening_generation], [firstBearer, randomUUID()]]) {
    await assertDeniedTicket(fixture, opened, credential, generation);
  }
  await assertHostedBindingRefusals(fixture, first, opened);
  const { terminal } = await h.bridge.terminalOpen(h.owner, first.handle, () => {});
  await terminalResult(h, terminal, "ping", {});
});

async function assertHostedBindingRefusals(fixture, identity, opening) {
  const database = await fixtureDatabase(fixture);
  // Vary one persisted claim at a time after a real issuer mint and before its
  // first introspection. Each case uses its own token digest.
  const variants = [
    ["user_id", fixtureUuid(fixture.unrelatedAccount)],
    ["device_id", fixtureUuid(fixture.unrelatedDevice)],
    ["issuer", "https://unrelated.invalid"],
    ["endpoint_epoch", randomUUID()],
    ["authorization_epoch", -1],
  ];
  try {
    for (const [field, invalid] of variants) {
      const family = fixtureUuid(identity.familyId);
      const read = database.prepare("SELECT " + field + " AS claim FROM bridge_http_families WHERE id = ?");
      const write = database.prepare("UPDATE bridge_http_families SET " + field + " = ? WHERE id = ?");
      const original = read.get(family).claim;
      const fresh = await renewFixtureCredential(fixture, identity);
      assert.equal(write.run(invalid, family).changes, 1);
      try {
        await assertDeniedTicket(fixture, opening, "Bearer " + fresh.access_token, opening.opening_generation);
      } finally {
        assert.equal(write.run(original, family).changes, 1);
      }
    }
  } finally {
    database.close();
  }
}

async function assertDeniedTicket(fixture, opening, credential, generation) {
  const response = await fetch(fixture.bridgeOrigin + "/v1/terminal-tickets", {
    method: "POST", headers: {
      Authorization: credential, Origin: fixture.hostedOrigin, "X-Build-Session": opening.session_id,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ opening_generation: generation }),
  });
  assert.equal(response.status, 401);
  const body = await response.text();
  assert.equal(JSON.parse(body).error_code, "invalid_credential");
  assert.ok(!body.includes(credential.slice("Bearer ".length)), "ticket refusal redacts the device credential");
}

test("terminal qualification preserves fresh write renewal and fences read-only renewal", {
  ...qualification, skip: !fixturePath || terminalGate !== "on",
}, async (t) => {
  const fixture = await loadFixture();
  let bearer;
  const h = harness(t, fixture, { observeBearer: (value) => { bearer = value; } });
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const opening = await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  const events = [];
  const { terminal } = await h.bridge.terminalOpen(h.owner, identity.handle, (event) => events.push(event));
  assert.equal(await patchOpening(fixture, opening, bearer), 200);
  await terminalResult(h, terminal, "ping", {});
  await restrictFixtureFamily(fixture, identity.familyId);
  const readOnly = await renewFixtureCredential(fixture, identity);
  assert.equal(readOnly.scope, "bridge:read");
  assert.equal(await patchOpening(fixture, opening, "Bearer " + readOnly.access_token), 200);
  await eventually(() => events.some((event) => event.type === "close"), "scoped-down socket to close", 7000);
  await assert.rejects(h.bridge.terminalCall(h.owner, terminal, "ping", {}), { code: "invalid_terminal_handle" });
  const projects = await fetch(fixture.bridgeOrigin + "/v1/projects", {
    headers: { Authorization: "Bearer " + readOnly.access_token, "X-Build-Session": opening.session_id },
  });
  assert.equal(projects.status, 200);
  assert.equal((await projects.json()).ok, true);
});

async function renewFixtureCredential(fixture, identity) {
  const response = await requestFixtureRenewal(fixture, identity);
  assert.equal(response.status, 201);
  return response.json();
}

function requestFixtureRenewal(fixture, identity) {
  return fetch(fixture.hostedOrigin + "/api/devices/" + fixture.deviceId + "/http-credentials", {
    method: "POST",
    headers: { Authorization: "Bearer " + fixture.accessToken, "Content-Type": "application/json" },
    body: JSON.stringify({ family_id: identity.familyId, endpoint_epoch: identity.endpointEpoch }),
  });
}

test("terminal qualification disables and rebinds a mounted device with an active socket", {
  ...qualification, skip: !fixturePath || process.env.BUILD_HTTP_QUALIFICATION_REBIND !== "1",
}, async (t) => {
  const fixture = await loadFixture();
  let bearer;
  const h = harness(t, fixture, { observeBearer: (value) => { bearer = value; } });
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const opening = await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  const staleBearer = bearer;
  const events = [];
  const { terminal } = await h.bridge.terminalOpen(h.owner, identity.handle, (event) => events.push(event));
  await terminalResult(h, terminal, "ping", {});
  const marker = (name) => resolve(fixture.runDir, name);
  await writeFile(marker("disable-ready"), "");

  await eventually(() => hasFile(marker("disabled-epoch")), "issuer to accept the signed disable", 10_000);
  await eventually(() => events.some(({ type }) => type === "close"), "disabled listener to close WSS", 3000);
  await assert.rejects(h.bridge.terminalCall(h.owner, terminal, "ping", {}), { code: "invalid_terminal_handle" });
  const renewal = await requestFixtureRenewal(fixture, identity);
  assert.equal(renewal.status, 401);
  await renewal.body?.cancel();
  await writeFile(marker("rebind-ready"), "");

  await eventually(() => hasFile(marker("rebound")), "listener to rebind at the same authority", 15_000);
  assert.equal(await patchOpening(fixture, opening, staleBearer), 401);
  const rebound = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  assert.notEqual(rebound.endpointEpoch, identity.endpointEpoch);
  await h.bridge.open(h.owner, rebound.handle, "^3.0.0");
  const fresh = await h.bridge.terminalOpen(h.owner, rebound.handle, () => {});
  await terminalResult(h, fresh.terminal, "ping", {});
  t.diagnostic("signed disable closed WSS and refused renewal, then a new endpoint epoch required fresh discovery");
});

test("terminal qualification revokes the socket and retains a process already started", {
  ...qualification, skip: !fixturePath || terminalGate !== "on",
}, async (t) => {
  const fixture = await loadFixture();
  let deviceBearer;
  const h = harness(t, fixture, { observeBearer: (bearer) => { deviceBearer = bearer; } });
  const name = "wss-revocation-" + randomUUID();
  const projectPath = resolve(fixture.projectsDir, name);
  const events = [];
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const opening = await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  const created = await h.bridge.request(h.owner, identity.handle, "project_create", { name, parent: fixture.projectsDir });
  assert.equal(created.ok, true, created.error);
  const scope = { project_id: created.result.project_id };
  const first = await h.bridge.terminalOpen(h.owner, identity.handle, (event) => events.push(event));
  const term = await terminalResult(h, first.terminal, "term.create", scope);
  const params = { term_id: term.term_id, ...scope };
  await terminalResult(h, first.terminal, "term.attach", params);
  const release = resolve(projectPath, ".qualification-release");
  t.after(() => writeFile(release, "").catch(() => {}));
  await startWitnessProcess(h, first.terminal, term.term_id, projectPath);
  await revokeFixtureCredential(fixture, identity, opening, deviceBearer);
  await eventually(() => events.some((event) => event.type === "close"), "revoked socket to close", 7000);
  await assert.rejects(h.bridge.terminalCall(h.owner, first.terminal, "ping", {}), { code: "invalid_terminal_handle" });
  await writeFile(release, "");
  await eventually(() => hasFile(resolve(projectPath, ".qualification-finished")), "process still running after revocation");
  const next = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  const reopened = await h.bridge.open(h.owner, next.handle, "^3.0.0");
  const freshEvents = [];
  const second = await h.bridge.terminalOpen(h.owner, next.handle, (event) => freshEvents.push(event));
  const restored = await terminalResult(h, second.terminal, "term.attach", params);
  assert.equal(restored.term_id, term.term_id);
  await terminalResult(h, second.terminal, "term.close", params);
  t.diagnostic("real grant DELETE fenced the old WSS; the started process finished and a fresh grant reattached the same shell");

  await terminalResult(h, second.terminal, "ping", {});
  const logout = await fetch(fixture.hostedOrigin + "/api/bridge-http/logout", {
    method: "POST",
    headers: { Authorization: "Bearer " + fixture.accessToken, "Content-Type": "application/json" },
    body: "{}",
  });
  assert.equal(logout.status, 204);
  assert.equal(await patchOpening(fixture, reopened, deviceBearer), 401);
  await eventually(() => freshEvents.some((event) => event.type === "close"), "socket to close after hosted logout", 7000);
  await assert.rejects(h.bridge.terminalCall(h.owner, second.terminal, "ping", {}), { code: "invalid_terminal_handle" });
  t.diagnostic("real hosted client logout retired the replacement WSS after verification");
});

async function terminalResult(h, terminal, method, params) {
  const reply = await h.bridge.terminalCall(h.owner, terminal, method, params);
  assert.equal(reply.ok, true, method + ": " + reply.error_code);
  return reply.result;
}

async function startWitnessProcess(h, terminal, termId, projectPath) {
  const command = "(for i in $(seq 1 200); do " +
    "if test -f .qualification-release; then touch .qualification-finished; exit; fi; sleep 0.1; done) & " +
    "touch .qualification-started\n";
  await terminalResult(h, terminal, "term.input", { term_id: termId, data: Buffer.from(command).toString("base64") });
  await eventually(() => hasFile(resolve(projectPath, ".qualification-started")), "shell to start the witness process", 5000);
}

async function hasFile(path) {
  try { await readFile(path); return true; }
  catch (error) { if (error.code === "ENOENT") return false; throw error; }
}

async function revokeFixtureCredential(fixture, identity, opening, deviceBearer) {
  const revoked = await fetch(fixture.hostedOrigin + "/api/devices/" + fixture.deviceId + "/http-credentials/" + identity.familyId, {
    method: "DELETE", headers: { Authorization: "Bearer " + fixture.accessToken },
  });
  assert.equal(revoked.status, 204);
  // Force observation through the real introspection path before checking
  // the idle socket and before allowing the existing child to finish.
  assert.equal(await patchOpening(fixture, opening, deviceBearer), 401);
}

async function patchOpening(fixture, opening, bearer) {
  const response = await fetch(fixture.bridgeOrigin + "/v1/sessions/" + opening.session_id, {
    method: "PATCH", headers: { Authorization: bearer, Origin: fixture.hostedOrigin },
  });
  await response.body?.cancel();
  return response.status;
}

test("terminal qualification restores the maximum grid over native WSS", {
  ...qualification, skip: !fixturePath || process.env.BUILD_HTTP_QUALIFICATION_GRID !== "1",
}, async (t) => {
  const fixture = await loadFixture();
  const h = harness(t, fixture);
  const identity = await h.bridge.discover(h.owner, fixture.bridgeOrigin);
  await h.bridge.open(h.owner, identity.handle, "^3.0.0");
  const events = [];
  const { terminal } = await h.bridge.terminalOpen(h.owner, identity.handle, (event) => events.push(event));
  const { recovery: recipe } = JSON.parse(await readFile(new URL("../../fixtures/api/http/terminal-websocket-v1.json", import.meta.url), "utf8"));
  const ghostty = await loadFixtureGhostty();
  const { cols, rows } = recipe;
  const size = Buffer.byteLength;
  const attachment = await h.bridge.terminalCall(h.owner, terminal, "term.attach", { term_id: "painted", cols, rows });
  assert.equal(attachment.ok, true, attachment.error_code);
  verifyGrid(ghostty, recipe, attachment.result.snapshot).free();
  const attachedBytes = size(JSON.stringify(attachment));
  await terminalResult(h, terminal, "term.ack", { term_id: "painted", cursor: attachment.result.cursor });
  await terminalResult(h, terminal, "term.input", { term_id: "painted", data: "cg==" });
  await eventually(() => events.some(({ data }) => data?.type === "term.reset"), "native maximum-grid reset");
  const reset = events.find(({ data }) => data?.type === "term.reset").data;
  const emulator = verifyGrid(ghostty, recipe, reset.data, "\x1b[2;5H?\x1b[6;8H");
  t.after(() => emulator.free());
  const resetBytes = size(JSON.stringify(reset));
  await terminalResult(h, terminal, "term.ack", { term_id: "painted", cursor: reset.cursor });
  const next = recipe.next_output;
  await terminalResult(h, terminal, "term.input", { term_id: "painted", data: Buffer.from(next).toString("base64") });
  await eventually(() => outputFor(events, "painted") === next, "native next live byte");
  emulator.write(next);
  emulator.update();
  assert.equal(emulator.getGraphemeString(5, 7), next);
  await terminalResult(h, terminal, "term.input", { term_id: "painted", data: recipe.next_input_base64 });
  await eventually(() => outputFor(events, "painted") === next + "x", "native keystroke echo after reset");
  assert.equal(events.filter(({ data }) => data?.type === "term.reset").length, 1);
  assert.equal(h.trace.filter(({ path }) => path === "/v1/terminal-tickets").length, 1);
  assert.ok([attachedBytes, resetBytes].every((bytes) => bytes > 1024 * 1024 && bytes <= 12 * 1024 * 1024));
  t.diagnostic("Native WSS restored all 65,536 cells and the cursor, then received the next live byte. Frame bytes " + attachedBytes + ", " + resetBytes);
});

async function loadFixtureGhostty() {
  const packageRoot = new URL("../../spa/node_modules/ghostty-web/", import.meta.url);
  const { Ghostty } = await import(new URL("dist/ghostty-web.js", packageRoot));
  const wasm = await readFile(new URL("ghostty-vt.wasm", packageRoot));
  const { instance } = await WebAssembly.instantiate(wasm, { env: { log() {} } });
  return new Ghostty(instance);
}

function verifyGrid(ghostty, recipe, snapshot, tail = "") {
  const { cols, rows, cells } = recipe;
  const [odd, even] = cells.map(({ sgr, text }) => sgr + text);
  const paint = recipe.prefix + ((odd + even).repeat(cols / 2) + (even + odd).repeat(cols / 2))
    .repeat(rows / 2) + recipe.suffix + tail;
  const expected = ghostty.createTerminal(cols, rows);
  const actual = ghostty.createTerminal(cols, rows);
  try {
    actual.write(Buffer.from(snapshot, "base64"));
    expected.write(paint);
    actual.update();
    expected.update();
    assert.deepEqual(actual.getCursor(), expected.getCursor());
    const left = actual.getViewport();
    const right = expected.getViewport();
    assert.equal(left.length, cols * rows);
    assert.equal(right.length, left.length);
    const mismatch = left.findIndex((cell, i) => Object.keys(cell).some((field) => cell[field] !== right[i][field]));
    assert.equal(mismatch, -1, "all maximum-grid cells must match");
    for (const { row, col } of recipe.witness_cells) {
      assert.equal(actual.getGraphemeString(row, col), expected.getGraphemeString(row, col));
    }
    return actual;
  } catch (error) {
    actual.free();
    throw error;
  } finally {
    expected.free();
  }
}

const electron = process.env.BUILD_HTTP_QUALIFICATION_ELECTRON;
test("terminal qualification recovers actual Electron IPC after uncommitted navigations", {
  ...qualification, skip: !fixturePath || terminalGate !== "on" || !electron,
}, (t) => runElectronQualification(t, "navigation"));

test("terminal qualification fences delayed Electron WSS IPC during native sign-out", {
  ...qualification, skip: !fixturePath || terminalGate !== "on" || !electron ||
    process.env.BUILD_HTTP_QUALIFICATION_ELECTRON_SIGN_OUT !== "1",
}, (t) => runElectronQualification(t, "sign-out"));

async function runElectronQualification(t, witness) {
  const fixture = await loadFixture();
  // Electron's application profile and certificate database are disposable.
  for (const dir of [".local/share/pki/nssdb", ".pki/nssdb"]) {
    const path = resolve(fixture.runDir, "home", dir);
    await mkdir(path, { recursive: true, mode: 0o700 });
    execFileSync("certutil", ["-N", "--empty-password", "-d", "sql:" + path]);
    execFileSync("certutil", ["-A", "-d", "sql:" + path, "-t", "C,,", "-n", "Build qualification CA", "-i", fixture.caFile]);
  }
  const entry = resolve(fixture.runDir, "electron-qualification.mjs");
  const main = new URL("../src/main.mjs", import.meta.url).href;
  await writeFile(entry, "(" + qualifyElectron.toString() + ")(..." + JSON.stringify([main, witness]) + ")\n", { mode: 0o600 });
  const logPath = resolve(fixture.runDir, "electron.log");
  const log = await open(logPath, "w", 0o600);
  const child = spawn(electron, [entry], { detached: true, stdio: ["ignore", log.fd, log.fd] });
  await log.close();
  t.after(() => {
    try { if (child.pid) process.kill(-child.pid, "SIGKILL"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  });
  let childError;
  child.on("error", (error) => { childError = error; });
  let outcome;
  const resultPath = resolve(fixture.runDir, "electron-result.json");
  await eventually(async () => {
    if (childError) throw childError;
    try {
      outcome = JSON.parse(await readFile(resultPath, "utf8"));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    return outcome || child.exitCode !== null || child.signalCode !== null;
  }, "Electron navigation result", 50_000);
  const output = await readFile(logPath, "utf8");
  assert.equal(outcome?.ok, true, output);
  if (witness === "sign-out") {
    await eventually(() => child.exitCode !== null || child.signalCode !== null, "Electron quit after sign-out", 10_000);
    assert.equal(child.exitCode, 0, output);
    assert.match(output, /Electron sign-out revoked the grant and fenced a held terminal reply/);
  } else {
    assert.match(output, /Electron navigation witnesses passed/);
  }
  t.diagnostic(output.split("\n").filter((line) => line.startsWith("PASS") || line.startsWith("Electron navigation")).join("\n"));
}

async function qualifyElectron(main, witness) {
  const { app, BrowserWindow, Menu, session, shell } = await import("electron");
  const assert = (await import("node:assert/strict")).default;
  const { once } = await import("node:events");
  const { readFile, rename, writeFile } = await import("node:fs/promises");
  const { registerHooks } = await import("node:module");
  const { setTimeout: sleep } = await import("node:timers/promises");
  const fixture = JSON.parse(await readFile(process.env.BUILD_HTTP_QUALIFICATION_FIXTURE, "utf8"));
  const allowedOrigins = new Set([fixture.hostedOrigin, fixture.bridgeOrigin]);
  const fetch = globalThis.fetch;
  const terminalSockets = [];
  let signOutCheck;
  globalThis.fetch = async (input, options) => {
    const url = new URL(input);
    assert.ok(allowedOrigins.has(url.origin), "qualification main requires a loopback fixture URL");
    const response = await fetch(input, { ...options, redirect: "error" });
    if (signOutCheck && url.origin === fixture.hostedOrigin && options?.method === "DELETE" &&
        /^\/api\/devices\/[^/]+\/http-credentials\/[^/]+$/.test(url.pathname)) {
      signOutCheck.revoked.resolve(response.status);
      await signOutCheck.finishCleanup.promise;
    }
    return response;
  };
  globalThis.qualificationTerminalDelivery = (event, deliver) => {
    if (signOutCheck && event.data?.id === "3") signOutCheck.pending.resolve({ event, deliver });
    else deliver();
  };
  globalThis.qualificationTerminalSend = (event) => {
    if (signOutCheck && event.data?.id === "3") signOutCheck.deliveries++;
  };
  globalThis.qualificationTerminalSocket = (url, connect) => {
    assert.equal(url.origin, fixture.bridgeOrigin.replace(/^https:/, "wss:"));
    const socket = connect();
    const closed = new Promise((resolve) => socket.once("close", resolve));
    terminalSockets.push({ socket, closed });
    return socket;
  };
  globalThis.qualificationAccessToken = fixture.accessToken;
  const appUrl = fixture.hostedOrigin + "/app/desktop";
  app.setPath("userData", fixture.runDir + "/electron-profile");
  const external = [];
  shell.openExternal = async (url) => { external.push(url); };
  // Use the real main, policies, renderer preload and IPC. The test injects
  // the disposable signer token at startup and redirects the hosted origin.
  registerHooks({
    load(url, context, next) {
      const loaded = next(url, context);
      let source = Buffer.from(loaded.source || "").toString();
      if (url === new URL("./security-policy.mjs", main).href) {
        source = source.replace('export const APP_URL = "https://getbuild.ing/app/desktop";',
          "export const APP_URL = " + JSON.stringify(appUrl) + ";");
        assert.ok(source.includes(appUrl));
      } else if (url === new URL("./bridge-http-contract.mjs", main).href) {
        source = source.replace('export const HOSTED_ORIGIN = "https://getbuild.ing";',
          "export const HOSTED_ORIGIN = " + JSON.stringify(fixture.hostedOrigin) + ";");
        assert.ok(source.includes(fixture.hostedOrigin));
      } else if (witness === "sign-out" && url === new URL("./bridge-http-ipc.mjs", main).href) {
        // Hold a real reply just before the IPC boundary. The delivery and
        // authorization functions still run when the test releases it.
        const deliver = "deliver(binding, event, delivery)";
        const send = "binding.owner.frame.send(TERMINAL_EVENT,";
        assert.ok(source.includes(deliver + ", {") && source.includes(send));
        source = source.replace(deliver + ", {",
          "globalThis.qualificationTerminalDelivery(event, () => " + deliver + "), {")
          .replace(send, "globalThis.qualificationTerminalSend(event);\n      " + send);
      } else if (witness === "sign-out" && url === new URL("./bridge-http-terminal.mjs", main).href) {
        const constructor = "return new WebSocket(url,";
        const endOptions = "maxPendingAutoPongs: 128,\n  });";
        assert.ok(source.includes(constructor) && source.includes(endOptions));
        source = source.replace(constructor,
          "return globalThis.qualificationTerminalSocket(url, () => new WebSocket(url,")
          .replace(endOptions, "maxPendingAutoPongs: 128,\n  }));");
      } else if (url === main) {
        source = source.replace('app.setAsDefaultProtocolClient("getbuilding");', "")
          .replace("void startAuthentication().finally(() => { starting = false; });",
            "accessToken = globalThis.qualificationAccessToken; delete globalThis.qualificationAccessToken;" +
            "mainWindow = createWindow(); starting = false;");
        assert.ok(source.includes("mainWindow = createWindow(); starting = false;"));
        if (witness === "sign-out") {
          const retire = "bridgeHttp.retire(contents);";
          assert.ok(source.includes(retire));
          source = source.replace(retire, retire + " globalThis.qualificationTerminalRetired(contents);");
        }
      } else {
        return loaded;
      }
      return { ...loaded, source };
    },
  });
  try {
    await app.whenReady();
    session.defaultSession.webRequest.onBeforeRequest({ urls: ["<all_urls>"] }, ({ url }, callback) => {
      callback({ cancel: !allowedOrigins.has(new URL(url).origin) });
    });
    await import(main);
    await wait(() => BrowserWindow.getAllWindows().length > 0);
    const win = BrowserWindow.getAllWindows()[0];
    const contents = win.webContents;
    await wait(() => contents.getURL() === appUrl && !contents.isLoadingMainFrame());
    const first = await openTerminal(contents);
    await ping(contents, first);
    if (witness === "sign-out") {
      await verifySignOut(win, contents, first);
      return;
    }
    await assertNoSubframeBridge(contents);
    console.log("PASS initial terminal IPC");
    await cancelledNavigation(contents);
    console.log("PASS terminal IPC after cancellation");
    await uncommittedNavigation(contents, "download");
    await uncommittedNavigation(contents, "empty");
    const stale = await openTerminal(contents);
    await ping(contents, stale);
    const committed = once(contents, "did-navigate");
    await win.loadURL(appUrl + "?qualification_navigation=commit");
    await committed;
    await checkRetired(contents, stale);
    const beforeExit = await openTerminal(contents);
    await ping(contents, beforeExit);
    const gone = once(contents, "render-process-gone", { signal: AbortSignal.timeout(8000) });
    const renderer = contents.getOSProcessId();
    assert.ok(renderer > 0 && renderer !== process.pid);
    // Kill only this fixture's renderer. SIGKILL leaves no core dump.
    process.kill(renderer, "SIGKILL");
    const [, details] = await gone;
    assert.equal(details.reason, "killed");
    await win.loadURL(appUrl);
    await checkRetired(contents, beforeExit);
    await ping(contents, await openTerminal(contents));
    console.log("PASS renderer exit retired the handle and fresh IPC worked");
    console.log("Electron navigation witnesses passed on " + process.versions.electron);
    await report(true);
  } catch (error) {
    console.error(error);
    await report(false);
  }

  async function verifySignOut(win, contents, terminal) {
    signOutCheck = {
      pending: Promise.withResolvers(),
      revoked: Promise.withResolvers(),
      finishCleanup: Promise.withResolvers(),
      deliveries: 0,
    };
    const { pending, revoked, finishCleanup } = signOutCheck;
    assert.equal(terminalSockets.length, 1);
    const [{ socket, closed }] = terminalSockets;
    const lifecycle = [];
    const destroyed = Promise.withResolvers();
    contents.once("destroyed", () => {
      lifecycle.push("WebContents destroyed");
      destroyed.resolve();
    });
    globalThis.qualificationTerminalRetired = (retired) => {
      assert.equal(retired, contents);
      assert.equal(win.isDestroyed(), true);
      lifecycle.push("retire returned");
    };
    win.once("closed", () => lifecycle.push("closed handler completed"));
    const errors = [];
    // Capture Electron's exception dialog as a test failure and keep the
    // fixture running long enough to observe revocation and IPC fencing.
    const captureWindowError = (error) => { errors.push(error); console.error(error); };
    process.on("uncaughtException", captureWindowError);
    try {
      value(await within(invoke(contents, "terminalCall", terminal, "ping", {}, "3"), "terminal call before sign-out"));
      const { event, deliver } = await within(pending.promise, "terminal reply before sign-out");
      assert.equal(event.data.ok, true);
      const accountMenu = Menu.getApplicationMenu().items.find(({ label }) => label === "Account");
      const signOut = accountMenu?.submenu.items.find(({ label }) => label === "Sign Out");
      assert.ok(signOut);
      assert.equal(socket.readyState, socket.OPEN);
      signOut.click();
      assert.equal(await within(revoked.promise, "hosted credential revoke"), 204);
      assert.equal(win.isDestroyed(), true);
      await within(destroyed.promise, "WebContents destruction");
      assert.equal(contents.isDestroyed(), true);
      assert.equal(lifecycle.length, 3);
      assert.ok(lifecycle.indexOf("retire returned") < lifecycle.indexOf("closed handler completed"));
      const closeCode = await within(closed, "terminal connection close");
      assert.equal(socket.readyState, socket.CLOSED);
      deliver();
      assert.equal(signOutCheck.deliveries, 0, "the real reply must be fenced before renderer IPC");
      console.log("Electron sign-out revoked the grant and fenced a held terminal reply (DELETE 204, zero IPC sends)");
      console.log(`PASS Native sign-out lifecycle: ${lifecycle.join(", ")}. Terminal socket close code ${closeCode}.`);
      await report(errors.length === 0);
    } finally {
      process.off("uncaughtException", captureWindowError);
      finishCleanup.resolve();
    }
  }

  async function within(promise, description) {
    const timeout = new AbortController();
    try {
      return await Promise.race([
        promise, sleep(5000, undefined, { signal: timeout.signal }).then(() => assert.fail("Timed out waiting for " + description)),
      ]);
    } finally {
      timeout.abort();
    }
  }

  async function report(ok) {
    const path = fixture.runDir + "/electron-result.json";
    await writeFile(path + ".tmp", JSON.stringify({ ok }), { mode: 0o600 });
    await rename(path + ".tmp", path);
  }

  async function wait(check) {
    const limit = Date.now() + 10_000;
    while (!await check()) {
      if (Date.now() >= limit) assert.fail("Electron did not settle its navigation");
      await sleep(25);
    }
  }

  function invoke(contents, method, ...args) {
    return contents.executeJavaScript("window.buildBridgeHttp[" + JSON.stringify(method) + "](..." + JSON.stringify(args) + ")");
  }

  function value(envelope) {
    assert.equal(envelope.ok, true, envelope.error?.code);
    return envelope.value;
  }

  async function openTerminal(contents) {
    const identity = value(await invoke(contents, "discover", fixture.bridgeOrigin));
    const opening = value(await invoke(contents, "open", identity.handle, "^3.0.0"));
    return value(await invoke(contents, "terminalOpen", identity.handle, crypto.randomUUID(),
      { opening_generation: opening.opening_generation })).terminal;
  }

  async function ping(contents, terminal) {
    const reply = await contents.executeJavaScript("(" + rendererPing.toString() + ")(" + JSON.stringify(terminal) + ")");
    assert.equal(reply, true);
  }

  async function rendererPing(terminal) {
    const api = window.buildBridgeHttp;
    const reply = Promise.withResolvers();
    const stop = api.onTerminalEvent(async (message) => {
      if (message.terminal !== terminal || message.data?.id !== "1") return;
      await api.terminalConsume(terminal, message.sequence);
      reply.resolve(message.data.ok);
    });
    const timeout = setTimeout(() => reply.reject(new Error("IPC ping reply timed out")), 5000);
    try {
      const sent = await api.terminalCall(terminal, "ping", {}, "1");
      if (!sent.ok) throw new Error(sent.error?.code);
      return await reply.promise;
    } finally {
      clearTimeout(timeout);
      stop();
    }
  }

  async function cancelledNavigation(contents) {
    const url = fixture.hostedOrigin + "/external-qualification-link";
    await contents.executeJavaScript("location.href = " + JSON.stringify(url) + "; void 0;");
    await wait(() => external.includes(url) && !contents.isLoadingMainFrame());
    assert.equal(contents.getURL(), appUrl);
    await ping(contents, await openTerminal(contents));
  }

  async function uncommittedNavigation(contents, name) {
    const target = appUrl + "?qualification_navigation=" + name;
    const signal = AbortSignal.timeout(8000);
    const started = once(contents, "did-start-navigation", { signal });
    const downloaded = name === "download"
      ? once(contents.session, "will-download", { signal }).then(([, item]) => {
        assert.equal(item.getURL(), target);
        item.cancel();
      })
      : null;
    await contents.executeJavaScript("location.href = " + JSON.stringify(target) + "; void 0;");
    await started;
    if (downloaded) {
      await downloaded;
    } else {
      await wait(async () => (await readFile(fixture.runDir + "/issuer.log", "utf8"))
        .includes("Qualification navigation sent HTTP 204"));
    }
    await wait(() => !contents.isLoadingMainFrame() && !contents.isWaitingForResponse());
    assert.equal(contents.getURL(), appUrl);
    await ping(contents, await openTerminal(contents));
    console.log("PASS terminal IPC after " + name);
  }

  async function checkRetired(contents, terminal) {
    const reply = await invoke(contents, "terminalCall", terminal, "ping", {}, "2");
    assert.equal(reply.ok, false);
    assert.equal(reply.error.code, "invalid_terminal_handle");
  }

  async function assertNoSubframeBridge(contents) {
    const bridgeType = await contents.executeJavaScript(`new Promise((resolve) => {
      const frame = document.createElement("iframe");
      frame.srcdoc = "<p>Qualification subframe</p>";
      frame.onload = () => {
        resolve(typeof frame.contentWindow.buildBridgeHttp);
        frame.remove();
      };
      document.body.append(frame);
    })`);
    assert.equal(bridgeType, "undefined");
    console.log("PASS subframe cannot access the terminal preload");
  }

}
