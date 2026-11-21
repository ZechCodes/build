import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { EventEmitter, once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getCACertificates, setDefaultCACertificates } from "node:tls";
import test, { after, before } from "node:test";
import { WebSocketServer } from "ws";
import { createBridgeHttp } from "../src/bridge-http.mjs";
import { installBridgeHttpIpc } from "../src/bridge-http-ipc.mjs";
import { APP_URL } from "../src/security-policy.mjs";

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url)));
const vectors = read("../../fixtures/api/http/terminal-websocket-v1.json");
const manifest = read("../src/bridge-http-resources.json");
const { endpoint, handshake, ticket, opening, messages, limits } = vectors;
const v1Opening = opening.cases.find(({ name }) => name === "v1");
const vectorBytes = (name) => Buffer.from(messages[name].utf8);
const envelope = (name) => JSON.parse(messages[name].utf8);
const bearer = "fixture-device-bearer";
const owner = { id: 1 };
let tls;
let originalCAs;

before(() => {
  const directory = mkdtempSync(join(tmpdir(), "desktop-terminal-tls-"));
  try {
    const key = join(directory, "key.pem");
    const cert = join(directory, "cert.pem");
    execFileSync("openssl", [
      "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key,
      "-out", cert, "-days", "1", "-subj", "/CN=localhost",
      "-addext", "subjectAltName=DNS:localhost",
    ], { stdio: "ignore" });
    tls = { key: readFileSync(key), cert: readFileSync(cert, "utf8") };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
  originalCAs = getCACertificates("default");
  setDefaultCACertificates([...originalCAs, tls.cert]);
});
after(() => setDefaultCACertificates(originalCAs));

function issueTicket(response, body = ticketBody(), status = ticket.response.status) {
  response.writeHead(status, ticket.response.headers);
  response.end(JSON.stringify(body));
}

function ticketBody() {
  return { ...ticket.response.body, expires_at: new Date(Date.now() + 10_000).toISOString() };
}

async function recordHttp(request, response, received, respond) {
  const parts = [];
  for await (const part of request) parts.push(part);
  const recorded = {
    method: request.method, url: request.url, headers: request.headers,
    body: Buffer.concat(parts).toString("utf8"),
  };
  received.push(recorded);
  if (respond) respond(response, recorded);
  else if (request.url === ticket.path) issueTicket(response);
  else response.writeHead(404).end();
}

async function peer(t, { respond, upgrade } = {}) {
  const requests = [];
  const upgrades = [];
  const server = createServer(tls, (req, res) => void recordHttp(req, res, requests, respond));
  const wss = new WebSocketServer({ noServer: true, perMessageDeflate: false });
  const sockets = new Set();
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (request, socket, head) => {
    upgrades.push({ url: request.url, headers: request.headers, rawHeaders: request.rawHeaders });
    if (upgrade) upgrade(request, socket);
    else wss.handleUpgrade(request, socket, head, (ws) => wss.emit("connection", ws));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  t.after(() => {
    for (const socket of sockets) socket.destroy();
    server.close();
    wss.close();
  });
  return { origin: "https://localhost:" + server.address().port, requests, upgrades, wss };
}

function fakeOpening(vector) {
  const result = structuredClone(opening.base);
  result.transport.operation_ids = manifest.operations.map(({ operationId }) => operationId);
  if (Object.hasOwn(vector, "descriptor")) result.transport.terminal_websocket = vector.descriptor;
  if (vector.capability) result.greeting.capabilities.push(opening.capability);
  result.grant_expires_at = new Date(Date.now() + 30_000).toISOString();
  return result;
}

function mint(origin, scope) {
  return {
    origin, issuer: endpoint.caller_origin, account_id: "account", device_id: "device",
    endpoint_epoch: "epoch", family_id: "family", access_token: bearer,
    expires_at: Math.floor(Date.now() / 1000) + 300, scope,
  };
}

async function desktop(t, origin, {
  vector = v1Opening,
  scope = "bridge:read bridge:write", fetchTicket = fetch,
  getMint = () => mint(origin, scope),
} = {}) {
  const auth = { accessToken: "fixture-hosted-bearer", generation: 1 };
  const adapter = createBridgeHttp({
    fetch(url, options) {
      if (url.startsWith(endpoint.caller_origin + "/api/"))
        return new Response(JSON.stringify(getMint()));
      if (url === origin + "/v1/sessions" && options.method === "POST")
        return new Response(JSON.stringify(fakeOpening(vector)));
      return fetchTicket(url, options);
    },
    getAuth: () => auth, isOwnerCurrent: (value) => value === owner,
    manifest, hostedOrigin: endpoint.caller_origin,
  });
  t.after(() => adapter.clear());
  const { handle } = await adapter.resume(owner, {
    deviceId: "device", endpointEpoch: "epoch", familyId: "family", origin,
  });
  await adapter.open(owner, handle, "1");
  return { adapter, handle, auth };
}

async function connect(t, options, onEvent) {
  const remote = await peer(t, options);
  const local = await desktop(t, remote.origin);
  return openConnection(local, remote, onEvent);
}

async function openConnection(local, remote, onEvent) {
  const connected = once(remote.wss, "connection");
  const delivery = [];
  const opened = await local.adapter.terminalOpen(owner, local.handle, onEvent ?? ((value) => delivery.push(value)));
  const [socket] = await connected;
  return { ...local, ...opened, remote, socket, delivery };
}

function controlRefreshTimer(t) {
  let scheduled = Promise.withResolvers();
  const schedule = globalThis.setTimeout;
  t.mock.method(globalThis, "setTimeout", (callback, delay, ...args) => {
    const timer = schedule(callback, delay, ...args);
    // The 30 s Opening renews about 20 s in, beyond the network timers.
    if (delay > 10_000 && delay <= 20_000) scheduled.resolve(() => {
      clearTimeout(timer);
      callback(...args);
    });
    return timer;
  });
  return async () => {
    const run = await scheduled.promise;
    scheduled = Promise.withResolvers();
    return run;
  };
}

async function renewableConnection(t, renewedBearer = bearer) {
  const nextRefresh = controlRefreshTimer(t);
  const patch = Promise.withResolvers();
  const remote = await peer(t, {
    respond(response, request) {
      if (request.method === "PATCH") patch.resolve({ response, request });
      else if (request.url === ticket.path) issueTicket(response);
      else response.writeHead(404).end();
    },
  });
  let mints = 0;
  const scope = "bridge:read bridge:write";
  const getMint = () => mints++ === 0
    ? { ...mint(remote.origin, scope), expires_at: Math.floor(Date.now() / 1000) + 50 }
    : { ...mint(remote.origin, scope), access_token: renewedBearer };
  const local = await desktop(t, remote.origin, { getMint });
  const ctx = await openConnection(local, remote);
  const refresh = await nextRefresh();
  return {
    ...ctx,
    async beginRenewal() {
      refresh();
      const { request } = await patch.promise;
      assert.equal(request.url, "/v1/sessions/" + opening.base.session_id);
      assert.equal(request.headers.authorization, "Bearer " + renewedBearer);
    },
    async finishRenewal(value = fakeOpening(v1Opening)) {
      const { response } = await patch.promise;
      response.writeHead(200, { "content-type": "application/json" });
      response.end(JSON.stringify(value));
      await nextRefresh();
    },
  };
}

function call(ctx, vector, params = vector.params) {
  return ctx.adapter.terminalCall(owner, ctx.terminal, vector.method, params, vector.id);
}

async function readCall(ctx, name) {
  const received = once(ctx.socket, "message");
  const result = call(ctx, envelope(name));
  const [bytes, binary] = await received;
  assert.equal(binary, true);
  assert.deepEqual(bytes, vectorBytes(name));
  return { result };
}

function checkTicket(request) {
  assert.equal(request.method, ticket.method);
  assert.equal(request.url, ticket.path);
  assert.equal(request.body, ticket.request.utf8);
  assert.deepEqual({
    authorization: request.headers.authorization,
    origin: request.headers.origin,
    session: request.headers["x-build-session"],
    cookie: request.headers.cookie,
  }, {
    authorization: "Bearer " + bearer, origin: endpoint.caller_origin,
    session: ticket.request.headers["x-build-session"], cookie: undefined,
  });
}

function checkUpgrade(remote) {
  const [upgrade] = remote.upgrades;
  const { headers, rawHeaders } = upgrade;
  assert.equal(upgrade.url, handshake.path);
  assert.equal(headers.host, new URL(remote.origin).host);
  assert.equal(headers.origin, endpoint.caller_origin);
  const protocols = (value) => value.split(",").map((token) => token.trim());
  assert.deepEqual(protocols(headers["sec-websocket-protocol"]),
    protocols(handshake.request.headers["sec-websocket-protocol"]));
  assert.equal(rawHeaders.filter((value) => /^sec-websocket-protocol$/i.test(value)).length, 1);
  for (const name of ["authorization", "cookie", "x-build-session", "sec-websocket-extensions"])
    assert.equal(headers[name], undefined, name);
}

test("desktop mints in main and sends only the fixed WSS upgrade and binary envelopes", async (t) => {
  const ctx = await connect(t);
  checkTicket(ctx.remote.requests[0]);
  checkUpgrade(ctx.remote);
  const { result } = await readCall(ctx, "attach");
  ctx.socket.send(vectorBytes("attach_ok"));
  assert.deepEqual(await result, envelope("attach_ok"));
  const detach = once(ctx.socket, "message");
  ctx.adapter.terminalDetach(owner, ctx.terminal, envelope("attach").id);
  assert.deepEqual(await detach, [vectorBytes("detach"), true]);
});

function answerUpgrade(request, socket, { protocols = [handshake.protocol], headers = {} } = {}) {
  const accept = createHash("sha1")
    .update(request.headers["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
  const response = {
    Upgrade: "websocket", Connection: "Upgrade", "Sec-WebSocket-Accept": accept, ...headers,
  };
  const fields = Object.entries(response).map(([name, value]) => name + ": " + value);
  fields.push(...protocols.map((value) => "Sec-WebSocket-Protocol: " + value));
  socket.write(["HTTP/1.1 101 Switching Protocols", ...fields, "", ""].join("\r\n"));
}

function openTerminal(local) {
  return local.adapter.terminalOpen(owner, local.handle, () => {});
}

function bypassTicketFetch() {
  return new Response(JSON.stringify(ticketBody()), {
    status: ticket.response.status, headers: ticket.response.headers,
  });
}

for (const selection of handshake.selections.filter(({ accepted }) => !accepted)) {
  test("native refuses selected protocol " + selection.name, async (t) => {
    const remote = await peer(t, {
      upgrade: (request, socket) => answerUpgrade(request, socket, { protocols: selection.header_values }),
    });
    const local = await desktop(t, remote.origin);
    await assert.rejects(openTerminal(local), { code: "terminal_protocol_error" });
  });
}

for (const { name, headers } of handshake.invalid_responses.filter(({ status }) => status === 101)) {
  test("native refuses invalid HTTP 101 " + name, async (t) => {
    const remote = await peer(t, { upgrade: (request, socket) => answerUpgrade(request, socket, { headers }) });
    await assert.rejects(openTerminal(await desktop(t, remote.origin)), { code: "terminal_protocol_error" });
  });
}

test("native verifies the WSS certificate hostname", async (t) => {
  const remote = await peer(t);
  const local = await desktop(t, remote.origin.replace("localhost", "127.0.0.1"), {
    fetchTicket: bypassTicketFetch,
  });
  await assert.rejects(openTerminal(local), { code: "terminal_network_error" });
  assert.deepEqual(remote.upgrades, []);
});

test("native refuses a WSS certificate from an untrusted issuer", async (t) => {
  const remote = await peer(t);
  const local = await desktop(t, remote.origin, { fetchTicket: bypassTicketFetch });
  setDefaultCACertificates(originalCAs);
  try {
    await assert.rejects(openTerminal(local), { code: "terminal_network_error" });
    assert.deepEqual(remote.upgrades, []);
  } finally {
    setDefaultCACertificates([...originalCAs, tls.cert]);
  }
});

test("native refuses the WSS redirect without a second request", async (t) => {
  const remote = await peer(t, { upgrade: (_request, socket) => socket.end(
    "HTTP/1.1 307 Temporary Redirect\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n",
  ) });
  await assert.rejects(openTerminal(await desktop(t, remote.origin)), { code: "terminal_upgrade_refused", status: 307 });
  assert.deepEqual(remote.upgrades.map(({ url }) => url), [handshake.path]);
  assert.deepEqual(remote.requests.map(({ url }) => url), [ticket.path]);
});

test("native refuses a ticket redirect before trying WSS", async (t) => {
  const remote = await peer(t, {
    respond: (response) => response.writeHead(307, { Location: "/redirected" }).end(),
  });
  await assert.rejects(openTerminal(await desktop(t, remote.origin)), { code: "network_error" });
  assert.deepEqual(remote.upgrades, []);
  assert.deepEqual(remote.requests.map(({ url }) => url), [ticket.path]);
});

for (const [name, alter, status] of [
  ["noncanonical", (body) => { body.ticket = body.ticket.slice(0, -1) + "B"; }],
  ["wrong generation", (body) => { body.opening_generation = "different"; }],
  ["expired", (body) => { body.expires_at = new Date(0).toISOString(); }],
  ["unexpected status", () => {}, 200],
]) {
  test("native refuses a " + name + " ticket before trying WSS", async (t) => {
    const remote = await peer(t, { respond(response) {
      const body = ticketBody();
      alter(body);
      issueTicket(response, body, status);
    } });
    await assert.rejects(openTerminal(await desktop(t, remote.origin)), { code: "invalid_response" });
    assert.deepEqual(remote.upgrades, []);
  });
}

for (const vector of opening.cases.filter(({ desktop, native_terminal_ipc }) =>
  desktop !== "wss" && native_terminal_ipc === "complete")) {
  test("native keeps the HTTP Opening and handles " + vector.name + " without a ticket", async (t) => {
    const remote = await peer(t);
    const local = await desktop(t, remote.origin, { vector });
    const code = vector.desktop === "webrtc" ? "terminal_unsupported" : "terminal_protocol_error";
    await assert.rejects(openTerminal(local), { code });
    assert.deepEqual(remote.requests, []);
  });
}

for (const [status, code] of [[401, "unauthorized"], [403, "terminal_forbidden"], [429, "terminal_capacity"]]) {
  test("ticket refusal " + status + " keeps the Opening valid for a fresh ticket", async (t) => {
    let refuse = true;
    const remote = await peer(t, { respond(response) {
      if (refuse) response.writeHead(status).end("peer error text");
      else issueTicket(response);
      refuse = false;
    } });
    const local = await desktop(t, remote.origin);
    await assert.rejects(openTerminal(local), { code, status });
    assert.deepEqual(remote.upgrades, []);
    const { terminal } = await openTerminal(local);
    assert.equal(remote.requests.length, 2);
    local.adapter.terminalClose(owner, terminal);
  });
}

function payloadOfSize(value, bytes, target, key) {
  const space = bytes - Buffer.byteLength(JSON.stringify(value));
  if (!target) return Buffer.from(JSON.stringify(value) + " ".repeat(space));
  const grow = Math.floor(space / 4) * 4;
  target[key] += "A".repeat(grow);
  const padded = JSON.stringify(value) + " ".repeat(space - grow);
  assert.equal(Buffer.byteLength(padded), bytes);
  return Buffer.from(padded);
}

function sizedReply(vector) {
  let value;
  if (vector.ok && /attach$/.test(vector.reply_to ?? "")) {
    value = envelope(vector.reply_to === "term.attach" ? "attach_ok" : "agent_attach_ok");
    return payloadOfSize(value, vector.bytes, value.result, "snapshot");
  }
  value = vector.ok ? { ...envelope("attach_ok"), result: {} } : envelope("oversized_attach");
  return payloadOfSize(value, vector.bytes);
}

function sizedPush(vector) {
  if (vector.push === "attachment_error") return payloadOfSize(envelope(vector.push), vector.bytes);
  const value = envelope(vector.push.slice(5));
  value.data = "";
  if (value.type === "term.output") value.cursor = vector.bytes;
  return payloadOfSize(value, vector.bytes, value, "data");
}

async function requestForVector(ctx, vector, id) {
  if (!vector.reply_to) return;
  const received = once(ctx.socket, "message");
  const result = call(ctx, { id, method: vector.reply_to, params: {} });
  void result.catch(() => {});
  await received;
  return { result };
}

function sendFragments(socket, bytes) {
  const split = Math.floor(bytes.length / 2);
  socket.send(bytes.subarray(0, split), { fin: false });
  socket.send(bytes.subarray(split));
}

async function verifySize(ctx, vector) {
  const bytes = vector.push ? sizedPush(vector) : sizedReply(vector);
  const id = vector.push ? undefined : JSON.parse(bytes).id;
  const request = await requestForVector(ctx, vector, id);
  const closed = once(ctx.socket, "close");
  sendFragments(ctx.socket, bytes);
  if (!vector.accepted) {
    assert.equal((await closed)[0], vector.close_code);
    if (request) await assert.rejects(request.result, { code: "terminal_message_too_large" });
    return;
  }
  if (request) assert.deepEqual(await request.result, JSON.parse(bytes));
  else await waitForDelivery(ctx, JSON.parse(bytes));
}

async function waitForDelivery(ctx, expected) {
  // A correlated barrier proves the preceding push was delivered in wire order.
  const { result } = await readCall(ctx, "ping");
  ctx.socket.send(reply(envelope("ping").id));
  await result;
  assert.deepEqual(ctx.delivery, [{ terminal: ctx.terminal, type: "message", data: expected }]);
}

for (const name of ["output", "reset", "closed", "agent_session_ended", "attachment_error"]) {
  test("native delivers the frozen push " + name, async (t) => {
    const ctx = await connect(t);
    ctx.socket.send(vectorBytes(name));
    await waitForDelivery(ctx, envelope(name));
  });
}

for (const length of [0, limits.message_bytes.ordinary]) {
  test("native rejects excess snapshot metadata with " + length + " bytes of data", async (t) => {
    const delivery = Promise.withResolvers();
    const ctx = await connect(t, undefined, delivery.resolve);
    const reset = envelope("reset");
    reset.data = "A".repeat(length);
    const padding = limits.snapshot_bound.json_metadata_bytes + 1;
    const closed = once(ctx.socket, "close");
    ctx.socket.send(Buffer.from(JSON.stringify(reset) + " ".repeat(padding)));
    assert.equal((await delivery.promise).code, "terminal_message_too_large");
    assert.equal((await closed)[0], 1009);
  });
}

for (const vector of limits.message_cases.filter(({ budget }) => budget !== "client")) {
  test("native applies the frozen receive budget to " + vector.name, async (t) => {
    await verifySize(await connect(t), vector);
  });
}

function sizedCall(vector) {
  const value = { id: "create-sized", method: "term.create", params: { kind: "" } };
  const space = vector.bytes - Buffer.byteLength(JSON.stringify(value));
  value.params.kind = "é".repeat(Math.floor(space / 2)) + "x".repeat(space % 2);
  return value;
}

for (const vector of limits.message_cases.filter(({ budget }) => budget === "client")) {
  test("native counts UTF-8 bytes for " + vector.name, async (t) => {
    const ctx = await connect(t);
    const value = sizedCall(vector);
    if (!vector.accepted) {
      await assert.rejects(call(ctx, value), { code: "request_too_large" });
      return;
    }
    const received = once(ctx.socket, "message");
    const result = call(ctx, value);
    const [raw, binary] = await received;
    assert.equal(binary, true);
    assert.equal(raw.length, vector.bytes);
    ctx.socket.send(reply(value.id));
    await result;
  });
}

function reply(id) {
  return Buffer.from(JSON.stringify({
    bridge_epoch: opening.base.greeting.bridge_epoch, ok: true, id, result: {},
  }));
}

for (const { bytes, accepted } of limits.decoded_input_cases) {
  test("native bounds a single input envelope at " + bytes + " decoded bytes", async (t) => {
    const ctx = await connect(t);
    const value = envelope("input");
    value.params.data = Buffer.alloc(bytes).toString("base64");
    if (!accepted) {
      await assert.rejects(call(ctx, value), { code: "invalid_params" });
      return;
    }
    const received = once(ctx.socket, "message");
    const result = call(ctx, value);
    assert.deepEqual(JSON.parse((await received)[0]), value);
    ctx.socket.send(reply(value.id));
    await result;
  });
}

for (const name of ["invalid_utf8", "utf8_bom", "invalid_json", "non_object_envelope", "text_frame"]) {
  test("native rejects " + name + " at the wire boundary", async (t) => {
    const invalid = vectors.invalid_messages.find((item) => item.name === name);
    const ctx = await connect(t);
    const closed = once(ctx.socket, "close");
    const bytes = invalid.hex ? Buffer.from(invalid.hex, "hex") : Buffer.from(invalid.utf8);
    ctx.socket.send(bytes, { binary: invalid.opcode === vectors.wire.binary_opcode });
    assert.equal((await closed)[0], invalid.close_code);
  });
}

test("owner teardown drops the socket and fences outstanding effects", async (t) => {
  const ctx = await connect(t);
  const { result } = await readCall(ctx, "input");
  const closed = once(ctx.socket, "close");
  ctx.adapter.clear(owner);
  await assert.rejects(result, { code: "stale_owner", uncertain: true });
  assert.equal((await closed)[0], 1006);
  assert.deepEqual(ctx.delivery, []);
  await assert.rejects(call(ctx, envelope("input")), { code: "invalid_terminal_handle" });
});

test("owner teardown during ticket issuance prevents a late upgrade", async (t) => {
  const remote = await peer(t);
  const posted = Promise.withResolvers();
  const issued = Promise.withResolvers();
  const local = await desktop(t, remote.origin, {
    fetchTicket(_url, { signal }) {
      posted.resolve(signal);
      return issued.promise;
    },
  });
  const pending = openTerminal(local);
  const signal = await posted.promise;
  local.adapter.clear(owner);
  issued.resolve(bypassTicketFetch());
  await assert.rejects(pending, { code: "canceled" });
  assert.equal(signal.aborted, true);
  assert.deepEqual(remote.upgrades, []);
});

test("owner teardown cancels a pending 101", async (t) => {
  const upgraded = Promise.withResolvers();
  const remote = await peer(t, { upgrade(_request, socket) {
    upgraded.resolve({ closed: once(socket, "close") });
  } });
  const local = await desktop(t, remote.origin);
  const pending = openTerminal(local);
  const { closed } = await upgraded.promise;
  local.adapter.clear(owner);
  await assert.rejects(pending, { code: "stale_owner" });
  await closed;
});

test("a terminal handle cannot be used from another owner or after an explicit close", async (t) => {
  const ctx = await connect(t);
  await assert.rejects(ctx.adapter.terminalCall({ ...owner }, ctx.terminal, "ping", {}),
    { code: "invalid_terminal_handle" });
  ctx.adapter.terminalClose(owner, ctx.terminal);
  const next = await openTerminal(ctx);
  assert.notEqual(next.terminal, ctx.terminal);
  assert.equal(ctx.remote.requests.length, 2);
  await assert.rejects(call(ctx, envelope("input")), { code: "invalid_terminal_handle" });
});

test("a stale reply with a current ID retires the socket before correlation", async (t) => {
  const ctx = await connect(t);
  const { result } = await readCall(ctx, "attach");
  const stale = vectors.epochs.find(({ name }) => name === "stale_matching_id");
  const closed = once(ctx.socket, "close");
  ctx.socket.send(Buffer.from(JSON.stringify({
    ...envelope(stale.message), bridge_epoch: stale.bridge_epoch,
  })));
  await assert.rejects(result, { code: "terminal_epoch_mismatch" });
  assert.equal((await closed)[0], 1002);
});

test("a retired main-process receiver can fail delivery without escaping the socket handler", async (t) => {
  const ctx = await connect(t, undefined, () => { throw new Error("receiver retired"); });
  const closed = once(ctx.socket, "close");
  ctx.socket.send(vectorBytes("output"));
  assert.equal((await closed)[0], 1006);
});

function expectRenewalClose(ctx, code) {
  assert.deepEqual(ctx.delivery, [{
    terminal: ctx.terminal, type: "close", code, closeCode: undefined,
  }]);
}

const rotatedBearer = "fixture-rotated-device-bearer";

test("renewal token rotation closes the socket and marks handed-off effects uncertain", { timeout: 5000 }, async (t) => {
  const ctx = await renewableConnection(t, rotatedBearer);
  const { result } = await readCall(ctx, "input");
  void result.catch(() => {});
  const closed = once(ctx.socket, "close");
  await ctx.beginRenewal();
  expectRenewalClose(ctx, "terminal_credentials_rotated");
  await assert.rejects(result, { code: "terminal_credentials_rotated", uncertain: true });
  assert.equal((await closed)[0], 1006);
  await ctx.finishRenewal();
});

test("renewal refuses terminal open while the rotated credential awaits PATCH", { timeout: 5000 }, async (t) => {
  const ctx = await renewableConnection(t, rotatedBearer);
  await ctx.beginRenewal();
  expectRenewalClose(ctx, "terminal_credentials_rotated");
  await assert.rejects(openTerminal(ctx), { code: "terminal_busy" });
  assert.equal(ctx.remote.requests.filter(({ url }) => url === ticket.path).length, 1);
  await ctx.finishRenewal();
  const next = await openTerminal(ctx);
  assert.notEqual(next.terminal, ctx.terminal);
  assert.equal(ctx.remote.requests.at(-1).headers.authorization, "Bearer " + rotatedBearer);
});

function changedEpoch() {
  const value = fakeOpening(v1Opening);
  value.greeting.bridge_epoch = vectors.epochs.find(({ name }) => name === "stale_matching_id").bridge_epoch;
  return value;
}

for (const [name, changedOpening] of [
  ["bridge epoch", changedEpoch],
  ["terminal mode", () => fakeOpening(opening.cases.find(({ name }) => name === "old_bridge"))],
]) {
  test("renewal PATCH changing the " + name + " closes the socket", { timeout: 5000 }, async (t) => {
    const ctx = await renewableConnection(t);
    const closed = once(ctx.socket, "close");
    await ctx.beginRenewal();
    assert.deepEqual(ctx.delivery, []);
    await ctx.finishRenewal(changedOpening());
    expectRenewalClose(ctx, "terminal_binding_changed");
    assert.equal((await closed)[0], 1006);
  });
}

test("renewal with the same epoch and token keeps the same socket", { timeout: 5000 }, async (t) => {
  const ctx = await renewableConnection(t);
  await ctx.beginRenewal();
  await ctx.finishRenewal();
  const { result } = await readCall(ctx, "ping");
  const response = reply(envelope("ping").id);
  ctx.socket.send(response);
  assert.deepEqual(await result, JSON.parse(response));
  assert.deepEqual(ctx.delivery, []);
  assert.equal(ctx.remote.upgrades.length, 1);
});

async function terminalIpc(t, remote) {
  const contents = new EventEmitter();
  const events = [];
  const frame = { url: APP_URL, send: (channel, event) => events.push({ channel, ...event }) };
  contents.mainFrame = frame;
  contents.send = frame.send;
  let handler;
  const auth = { accessToken: "fixture-hosted-bearer", generation: 1 };
  const ipc = installBridgeHttpIpc({
    ipcMain: { handle(_channel, callback) { handler = callback; } },
    getAuth: () => auth,
    getWindow: () => ({ webContents: contents }),
    manifest,
    fetch(url, options) {
      if (url.startsWith(new URL(APP_URL).origin + "/api/"))
        return new Response(JSON.stringify({ ...mint(remote.origin, "bridge:read bridge:write"), issuer: new URL(APP_URL).origin }));
      if (url === remote.origin + "/v1/sessions")
        return new Response(JSON.stringify(fakeOpening(v1Opening)));
      return fetch(url, options);
    },
  });
  ipc.attach(contents);
  t.after(() => ipc.clear());
  const invoke = (method, args, senderFrame = contents.mainFrame) =>
    handler({ sender: contents, senderFrame }, method, args);
  const resumed = await invoke("resume", [{
    deviceId: "device", endpointEpoch: "epoch", familyId: "family", origin: remote.origin,
  }]);
  assert.equal(resumed.ok, true, JSON.stringify(resumed));
  const { handle } = resumed.value;
  const opened = await invoke("open", [handle, "1"]);
  assert.equal(opened.ok, true);
  const ticketBody = { opening_generation: opened.value.opening_generation };
  return {
    contents, frame, events, auth, invoke, handle, ticketBody,
    open(requestId, body = ticketBody) { return invoke("terminalOpen", [handle, requestId, body]); },
  };
}

const ipcAttempt = "00000000-0000-4000-8000-000000000011";
const secondIpcAttempt = "00000000-0000-4000-8000-000000000012";

async function openIpc(t) {
  const remote = await peer(t);
  const h = await terminalIpc(t, remote);
  const connected = once(remote.wss, "connection");
  const result = await h.open(ipcAttempt);
  assert.equal(result.ok, true, JSON.stringify(result));
  const [socket] = await connected;
  return { ...h, socket, terminal: result.value.terminal, remote };
}

async function ipcCall(h, method, params, id) {
  const next = once(h.socket, "message");
  assert.equal((await h.invoke("terminalCall", [h.terminal, method, params, id])).ok, true);
  const [data, binary] = await next;
  assert.equal(binary, true);
  return JSON.parse(data.toString());
}

const flushIpc = () => new Promise((resolve) => setTimeout(resolve, 20));

test("hosted IPC carries a shell and agent on one socket and rejects reuse of a settled attach ID", async (t) => {
  const h = await openIpc(t);
  const attach = envelope("attach");
  const agent = envelope("agent_attach");
  assert.deepEqual(await ipcCall(h, attach.method, attach.params, "1"), { ...attach, id: "1" });
  h.socket.send(Buffer.from(JSON.stringify({ ...envelope("attach_ok"), id: "1" })));
  await flushIpc();
  const [first] = h.events;
  assert.equal(first.data.id, "1");
  assert.equal(first.requestId, ipcAttempt);
  assert.equal(first.terminal, h.terminal);
  assert.equal(first.channel, "build:bridge-http-terminal-event");
  assert.equal((await h.invoke("terminalConsume", [h.terminal, first.sequence])).ok, true);
  assert.equal((await h.invoke("terminalCall", [h.terminal, agent.method, agent.params, "1"])).error.code, "invalid_params");
  assert.deepEqual(await ipcCall(h, agent.method, agent.params, "2"), { ...agent, id: "2" });
  h.socket.send(Buffer.from(JSON.stringify({ ...envelope("agent_attach_ok"), id: "2" })));
  await flushIpc();
  const reply = h.events.at(-1);
  assert.equal(reply.data.id, "2");
  assert.equal(reply.data.result.term_id, envelope("agent_attach_ok").result.term_id);
  assert.equal(h.remote.upgrades.length, 1);
  const input = envelope("input");
  const resize = envelope("resize");
  const detach = once(h.socket, "message");
  assert.equal((await h.invoke("terminalDetach", [h.terminal, "1"])).ok, true);
  assert.deepEqual(JSON.parse((await detach)[0]), { type: "detach", attach_id: "1" });
  assert.deepEqual(await ipcCall(h, input.method, input.params, "3"), { ...input, id: "3" });
  assert.deepEqual(await ipcCall(h, resize.method, resize.params, "4"), { ...resize, id: "4" });
});

test("hosted IPC preserves D2 ticket status, retry body, pane controls and close codes", async (t) => {
  const remote = await peer(t, { respond(response, request) {
    if (!JSON.parse(request.body).retry) {
      issueTicket(response, { error: "snapshot_write_timeout", error_code: "snapshot_write_timeout", retryable: false }, 409);
      return;
    }
    issueTicket(response);
  } });
  const h = await terminalIpc(t, remote);
  const refused = await h.open(ipcAttempt);
  assert.equal(refused.error.code, "snapshot_write_timeout");
  assert.equal(refused.error.status, 409);
  assert.equal(refused.error.retryable, false);
  const connected = once(remote.wss, "connection");
  const body = { ...h.ticketBody, retry: true };
  const result = await h.open(secondIpcAttempt, body);
  assert.equal(result.ok, true, JSON.stringify(result));
  const [socket] = await connected;
  assert.deepEqual(remote.requests.map(({ body }) => JSON.parse(body)), [h.ticketBody, body]);
  const terminal = result.value.terminal;
  const attach = envelope("attach");
  await ipcCall({ ...h, terminal, socket }, attach.method, attach.params, "1");
  socket.send(Buffer.from(JSON.stringify({
    id: "1", ok: false, error: "Terminal snapshot is busy",
    error_code: "snapshot_busy", retryable: true, bridge_epoch: opening.base.greeting.bridge_epoch,
  })));
  const message = { ...envelope("attachment_error"), reason: "snapshot_busy", attach_id: "1" };
  socket.send(Buffer.from(JSON.stringify(message)));
  await flushIpc();
  assert.equal(h.events[0].data.error_code, "snapshot_busy");
  assert.equal(h.events[0].data.retryable, true);
  assert.deepEqual(h.events[1].data, message);
  socket.close(4008, "snapshot_write_timeout");
  await flushIpc();
  assert.deepEqual(h.events.at(-1), {
    channel: "build:bridge-http-terminal-event", terminal, requestId: secondIpcAttempt,
    type: "close", code: "terminal_closed", closeCode: 4008, reason: "snapshot_write_timeout",
  });
});

test("terminal IPC delivery window bounds unconsumed frames independently of emulator ACK", async (t) => {
  const h = await openIpc(t);
  const reset = { ...envelope("reset"), data: Buffer.alloc(2 * 1024 * 1024).toString("base64") };
  h.socket.send(Buffer.from(JSON.stringify(reset)));
  h.socket.send(Buffer.from(JSON.stringify(envelope("output"))));
  await flushIpc();
  assert.equal(h.events[0].data.data, reset.data);
  assert.equal(h.events[1].sequence, 2);
  assert.equal((await h.invoke("terminalConsume", [h.terminal, 2])).error.code, "invalid_params");
  assert.equal((await h.invoke("terminalConsume", [h.terminal, 1])).ok, true);
  assert.equal((await h.invoke("terminalConsume", [h.terminal, 2])).ok, true);
  const closed = once(h.socket, "close");
  for (let i = 0; i < 129; i += 1) h.socket.send(Buffer.from(messages.output.utf8));
  await closed;
  assert.equal(h.events.filter(({ type }) => type === "message").length, 130);
  assert.equal(h.events.at(-1).code, "terminal_delivery_overflow");
});

test("hosted IPC retires exact-frame handles and drops late terminal events on navigation", async (t) => {
  const h = await openIpc(t);
  const subframe = { url: APP_URL };
  assert.equal((await h.invoke("terminalConsume", [h.terminal, 1], subframe)).error.code, "untrusted_caller");
  assert.equal((await h.invoke("terminalClose", ["00000000-0000-4000-8000-000000000099"])).error.code, "invalid_terminal_handle");
  const closed = once(h.socket, "close");
  h.contents.emit("did-start-navigation", {}, APP_URL, false, true);
  const stale = await h.invoke("terminalCall", [h.terminal, "ping", {}, "1"], h.frame);
  assert.equal(stale.error.code, "untrusted_caller");
  h.contents.mainFrame = { ...h.frame };
  h.contents.emit("did-navigate", {}, APP_URL);
  assert.equal((await h.invoke("terminalClose", [h.terminal])).error.code, "invalid_terminal_handle");
  await closed;
  assert.deepEqual(h.events, []);
});
