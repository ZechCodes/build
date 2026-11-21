import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createBrowserHttpEndpoint } from "../src/core/httpBrowserEndpoint.js";

const fixture = JSON.parse(readFileSync(new URL("../../fixtures/api/http/terminal-websocket-v1.json", import.meta.url), "utf8"));
const { endpoint: policy, ticket, opening, handshake, limits, messages } = fixture;
const now = Date.parse(ticket.lifetime_cases[0].issued_at);
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const json = (value, status = 200) => new Response(JSON.stringify(value), {
  status, headers: ticket.response.headers,
});
const payload = (name) => JSON.parse(messages[name].utf8);

class FakePeer extends EventTarget {
  readyState = 0;
  bufferedAmount = 0;
  extensions = "";
  sent = [];
  close = vi.fn(() => { this.readyState = 3; });

  constructor(url, protocols) {
    super();
    this.url = url;
    this.protocols = protocols;
  }
  open(protocol = handshake.selected) {
    this.protocol = protocol;
    this.readyState = 1;
    this.dispatchEvent(new Event("open"));
  }
  drop(code = 1006, reason = "") {
    this.readyState = 3;
    this.dispatchEvent(Object.assign(new Event("close"), { code, reason }));
  }
  receive(utf8) { this.receiveRaw(encoder.encode(utf8).buffer); }
  receiveRaw(data) { this.dispatchEvent(new MessageEvent("message", { data })); }
  reply(frame, id = this.calls().at(-1).id) {
    this.receive(JSON.stringify({ ...frame, id }));
  }
  send(bytes) {
    expect(bytes).toBeInstanceOf(Uint8Array);
    this.sent.push(decoder.decode(bytes));
  }
  calls() { return this.sent.map((message) => JSON.parse(message)); }
}

const endpoints = [];
function harness({ origin = policy.bridge_https_origin, answer = () => json(ticket.response.body, 201), greeting = opening.base.greeting, renewToken, callerOrigin = policy.caller_origin, clockSkew = 0 } = {}) {
  const peers = [];
  const heldOpening = { ...opening.base, greeting };
  let token = "device-bearer";
  let current = true;
  const fetchImpl = vi.fn(async (url) => {
    if (url.endsWith(ticket.path)) return answer();
    if (url.includes("/v1/sessions")) return json({ ...heldOpening, grant_expires_at: new Date(now + 60000).toISOString() });
    return new Response(null, { status: 204 });
  });
  const endpoint = createBrowserHttpEndpoint({
    grant: {
      identity: { origin }, token: () => token, expiresAt: () => now + clockSkew + 30000,
      renew: async () => { if (renewToken) token = renewToken; }, close: vi.fn(),
    },
    fetchImpl, now: () => now + clockSkew, isCurrent: () => current, callerOrigin,
    createWebSocket: (url, protocols) => {
      const peer = new FakePeer(url, protocols);
      peers.push(peer);
      return peer;
    },
  });
  endpoints.push(endpoint);
  return { endpoint, fetchImpl, peers, replace: () => { current = false; } };
}

afterEach(() => {
  endpoints.splice(0).forEach((endpoint) => endpoint.close());
  vi.useRealTimers();
});

async function connected(h = harness()) {
  const held = h.held || await h.endpoint.open("range");
  const attempt = h.endpoint.openTerminal(held);
  await vi.waitFor(() => expect(h.peers.at(-1)?.readyState).toBe(0));
  const peer = h.peers.at(-1);
  peer.open();
  return { ...h, held, peer, carrier: await attempt };
}

function request(carrier, name, options) {
  const { method, params } = payload(name);
  return carrier.call(method, params, options);
}

function fixtureReply(peer, name, index = -1) {
  peer.reply(payload(name), peer.calls().at(index).id);
}

describe("browser terminal admission", () => {
  it.each(policy.browser_host_cases)("checks the cookie hostname boundary at $name", async ({ bridge_https_origin: origin, allowed }) => {
    const h = harness({ origin });
    const held = await h.endpoint.open("range");
    if (!allowed) {
      await expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({ error_code: "origin_forbidden" });
      expect(h.fetchImpl).toHaveBeenCalledOnce();
      expect(h.peers).toHaveLength(0);
      return;
    }
    const { peer } = await connected({ ...h, held });
    expect(peer.url).toBe(origin.replace("https:", "wss:") + handshake.path);
    expect(peer.protocols).toEqual(handshake.offers[0].tokens);
    expect(peer.binaryType).toBe("arraybuffer");
    const [url, options] = h.fetchImpl.mock.calls[1];
    expect(url).toBe(origin + ticket.path);
    expect(options).toMatchObject({
      method: ticket.method, credentials: "omit", redirect: "error", cache: "no-store", body: ticket.request.utf8,
    });
    const headers = new Headers(options.headers);
    expect(headers.get("authorization")).toBe("Bearer device-bearer");
    expect(headers.get("x-build-session")).toBe(ticket.request.headers["x-build-session"]);
    expect(headers.has("origin")).toBe(false); // Supplied by the browser.
  });

  it("fences the Opening and validates the returned generation, token and expiry format before constructing WSS", async () => {
    const good = ticket.response.body;
    for (const body of [
      { ...good, opening_generation: "other" },
      { ...good, ticket: handshake.offers.find(({ name }) => name === "noncanonical_pad_bits").tokens[1].slice(7) },
      { ...good, expires_at: "not a timestamp" },
    ]) {
      const h = harness({ answer: () => json(body, 201) });
      const held = await h.endpoint.open("range");
      await expect(h.endpoint.openTerminal({ ...held, opening_generation: "other" })).rejects.toMatchObject({ uncertain: false });
      expect(h.fetchImpl).toHaveBeenCalledOnce();
      await expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({ error_code: "protocol" });
      expect(h.peers).toHaveLength(0);
    }
  });

  it("requires an Opening epoch before requesting a ticket", async () => {
    const h = harness({ greeting: {} });
    const held = await h.endpoint.open("range");
    await expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({ error_code: "protocol" });
    expect(h.fetchImpl).toHaveBeenCalledOnce();
  });

  it.each(handshake.selections)("checks exact selected subprotocol for $name", async ({ browser_protocol, accepted }) => {
    const h = harness();
    const held = await h.endpoint.open("range");
    const attempt = h.endpoint.openTerminal(held);
    const outcome = accepted ? attempt : expect(attempt).rejects.toMatchObject({ error_code: "protocol" });
    await vi.waitFor(() => expect(h.peers).toHaveLength(1));
    h.peers[0].open(browser_protocol);
    await outcome;
    expect(h.peers[0].close).toHaveBeenCalledTimes(accepted ? 0 : 1);
  });

  it("lets the bridge decide expiry when the browser clock runs ahead", async () => {
    const { peer } = await connected(harness({ clockSkew: 16000 }));
    expect(peer.readyState).toBe(1);
  });

  it("explains that a non-HTTPS caller origin requires HTTPS", async () => {
    const h = harness({ callerOrigin: "http://localhost:5173" });
    const held = await h.endpoint.open("range");
    await expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({
      error_code: "origin_forbidden",
      message: "Browser terminals require an HTTPS app origin.",
    });
    expect(h.fetchImpl).toHaveBeenCalledOnce();
  });

  it("rejects negotiated extensions even with the expected subprotocol", async () => {
    const h = harness();
    const held = await h.endpoint.open("range");
    const outcome = expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({ error_code: "protocol" });
    await vi.waitFor(() => expect(h.peers).toHaveLength(1));
    h.peers[0].extensions = Object.values(handshake.invalid_responses.find(({ name }) => name === "extension_negotiated").headers)[0];
    h.peers[0].open();
    await outcome;
    expect(h.peers[0].close).toHaveBeenCalledOnce();
  });

  it("reports a network ticket failure as retryable before sending any terminal effect", async () => {
    const h = harness({ answer: () => { throw new TypeError("internal fetch diagnostics"); } });
    const held = await h.endpoint.open("range");
    await expect(h.endpoint.openTerminal(held)).rejects.toMatchObject({ error_code: "network", retryable: true, uncertain: false });
    expect(h.peers).toHaveLength(0);
  });

  it.each(Object.values(ticket.refusals))("reports a $status ticket POST refusal while REST stays usable", async ({ status, body }) => {
    const h = harness({ answer: () => json(body, status) });
    h.fetchImpl.mockResolvedValueOnce(json(opening.base));
    const held = await h.endpoint.open("range");
    const refused = expect(h.endpoint.openTerminal(held)).rejects;
    const capacity = opening.selected_wss_failures.find(({ cause }) => cause === "capacity");
    await refused.toMatchObject({
      error_code: body.error_code, httpStatus: status, retryable: body.retryable,
      ...(status === capacity.http_status ? { message: capacity.pane_message } : {}),
    });
    expect(h.peers).toHaveLength(0);
    h.fetchImpl.mockResolvedValueOnce(json({ ok: true }));
    await expect(h.endpoint.request(held, { path: "/v1/health", method: "GET" }, {})).resolves.toEqual({ ok: true });
  });

  it("retires the old socket after credential rotation and mints with the new bearer", async () => {
    vi.useFakeTimers();
    const { carrier, peer, ...h } = await connected(harness({ renewToken: "renewed-bearer" }));
    const close = vi.fn();
    carrier.onClose(close);
    await vi.advanceTimersByTimeAsync(20000);
    expect(close).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ error_code: "credentials_rotated", retryable: true }));
    expect(peer.close).toHaveBeenCalledOnce();
    await connected(h);
    const posts = h.fetchImpl.mock.calls.filter(([url]) => url.endsWith(ticket.path));
    expect(new Headers(posts.at(-1)[1].headers).get("authorization")).toBe("Bearer renewed-bearer");
  });

  it("fences the socket to one owner signal per Opening", async () => {
    const h = harness();
    const held = await h.endpoint.open("range");
    const first = new AbortController();
    const second = new AbortController();
    const opening = h.endpoint.openTerminal(held, { signal: first.signal });
    const joining = h.endpoint.openTerminal(held, { signal: first.signal });
    await expect(h.endpoint.openTerminal(held, { signal: second.signal })).rejects.toMatchObject({ error_code: "busy" });
    await vi.waitFor(() => expect(h.peers).toHaveLength(1));
    h.peers[0].open();
    const [carrier, joined] = await Promise.all([opening, joining]);
    expect(joined).toBe(carrier);
    second.abort();
    expect(h.peers[0].close).not.toHaveBeenCalled();
    first.abort();
    expect(h.peers[0].close).toHaveBeenCalledOnce();
  });

  it("abandons a pending ticket on owner retirement, including a late fetch response", async () => {
    let release;
    const h = harness({ answer: () => new Promise((resolve) => { release = resolve; }) });
    const held = await h.endpoint.open("range");
    const controller = new AbortController();
    const pending = h.endpoint.openTerminal(held, { signal: controller.signal });
    const ended = expect(pending).rejects.toMatchObject({ error_code: "scope_ended" });
    await vi.waitFor(() => expect(release).toBeTypeOf("function"));
    controller.abort();
    await ended;
    release(json(ticket.response.body, 201));
    await Promise.resolve();
    expect(h.peers).toHaveLength(0);
    await connected({ ...h, held, endpoint: h.endpoint, fetchImpl: h.fetchImpl.mockResolvedValueOnce(json(ticket.response.body, 201)) });
  });
});

describe("browser terminal wire", () => {
  it("correlates shell and agent attaches and carries ordered output, reset, credit and pane closure", async () => {
    const { peer, carrier } = await connected();
    const attachId = vi.fn();
    const push = vi.fn();
    carrier.onPush(push);
    const shell = request(carrier, "attach", { onAttachId: attachId });
    const agent = request(carrier, "agent_attach");
    expect(peer.calls()[0]).toMatchObject({ method: "term.attach", params: payload("attach").params });
    expect(attachId).toHaveBeenCalledExactlyOnceWith(peer.calls()[0].id);
    fixtureReply(peer, "agent_attach_ok");
    await expect(agent).resolves.toEqual(payload("agent_attach_ok").result);
    fixtureReply(peer, "attach_ok", 0);
    await expect(shell).resolves.toEqual(payload("attach_ok").result);
    expect(peer.sent).toHaveLength(2); // Credit waits for the consumer.
    const ack = request(carrier, "ack");
    expect(peer.calls().at(-1).params).toEqual(payload("ack").params);
    peer.reply({ ...payload("attach_ok"), result: {} });
    await ack;
    for (const name of ["output", "reset", "agent_session_ended", "closed"]) peer.receive(messages[name].utf8);
    expect(push.mock.calls.flat()).toEqual(["output", "reset", "agent_session_ended", "closed"].map(payload));
  });

  it("releases a pending interest by its attach ID and preserves pane-local refusals", async () => {
    const { peer, carrier } = await connected();
    const push = vi.fn();
    carrier.onPush(push);
    let attachId;
    const attach = request(carrier, "attach", { onAttachId: (id) => { attachId = id; } });
    carrier.detach(attachId);
    expect(peer.calls().at(-1)).toEqual({ ...payload("detach"), attach_id: attachId });
    peer.reply(payload("oversized_attach"), attachId);
    await expect(attach).rejects.toMatchObject({ error_code: "snapshot_too_large", retryable: false, uncertain: false });
    peer.receive(messages.attachment_error.utf8);
    expect(push).toHaveBeenCalledExactlyOnceWith(payload("attachment_error"));
    expect(peer.close).not.toHaveBeenCalled();
  });

  it("splits a paste without delaying its first write and keeps later input behind its remaining bytes", async () => {
    const { peer, carrier } = await connected();
    const { term_id } = payload("input").params;
    const cap = limits.decoded_input_bytes;
    const data = btoa("\x00".repeat(cap) + "\xff\x01");
    const paste = carrier.call("term.input", { term_id, data });
    expect(atob(peer.calls()[0].params.data)).toBe("\x00".repeat(cap));
    const next = request(carrier, "input");
    const resize = request(carrier, "resize");
    expect(peer.calls().at(-1).method).toBe("term.resize");
    peer.reply({ ...payload("attach_ok"), result: {} });
    await resize;
    peer.reply({ ...payload("attach_ok"), result: {} }, peer.calls()[0].id);
    await vi.waitFor(() => expect(peer.calls()).toHaveLength(3));
    expect(atob(peer.calls()[2].params.data)).toBe("\xff\x01");
    peer.reply({ ...payload("attach_ok"), result: {} });
    await paste;
    await vi.waitFor(() => expect(peer.calls()).toHaveLength(4));
    expect(peer.calls()[3].params).toEqual(payload("input").params);
    peer.reply({ ...payload("attach_ok"), result: {} });
    await next;
  });

  it("caps outstanding calls through settlement and can still detach a pending interest", async () => {
    const { peer, carrier } = await connected();
    const calls = Array.from({ length: limits.outstanding_calls.socket }, () => request(carrier, "ping"));
    for (const call of calls) void call.catch(() => {});
    await expect(request(carrier, "resize")).rejects.toMatchObject({ error_code: "busy", uncertain: false });
    expect(peer.sent).toHaveLength(limits.outstanding_calls.socket);
    peer.reply({ ...payload("attach_ok"), result: {} }, peer.calls()[0].id);
    await calls[0];
    const controller = new AbortController();
    const attach = request(carrier, "attach", { signal: controller.signal });
    const attachId = peer.calls().at(-1).id;
    controller.abort();
    await expect(attach).rejects.toMatchObject({ uncertain: true });
    carrier.detach(attachId);
    expect(peer.calls().at(-1)).toEqual({ type: "detach", attach_id: attachId });
    await expect(request(carrier, "ping")).rejects.toMatchObject({ error_code: "busy" });
    peer.reply(payload("oversized_attach"), attachId);
    const next = request(carrier, "ping");
    peer.reply({ ...payload("attach_ok"), result: {} });
    await next;
  });

  it("honors a liveness deadline without releasing unsettled wire credit", async () => {
    vi.useFakeTimers();
    const { peer, carrier } = await connected();
    const pending = request(carrier, "ping", { timeoutMs: 3000 });
    const expired = expect(pending).rejects.toMatchObject({ error_code: "timeout", timedOut: true, uncertain: true });
    expect(peer.sent).toHaveLength(1);
    const active = Array.from({ length: limits.outstanding_calls.socket - 1 }, () => request(carrier, "ping"));
    for (const call of active) void call.catch(() => {});
    await vi.advanceTimersByTimeAsync(3000);
    await expired;
    await expect(request(carrier, "ping")).rejects.toMatchObject({ error_code: "busy" });
    peer.reply({ ...payload("attach_ok"), result: {} }, peer.calls()[0].id);
    const next = request(carrier, "ping");
    peer.reply({ ...payload("attach_ok"), result: {} });
    await next;
  });

  it("admits only the pinned methods and original grid dimensions", async () => {
    const { peer, carrier } = await connected();
    for (const { name, method, params, accepted } of limits.grid.cases) {
      const call = carrier.call(method, params);
      if (accepted) {
        const reply = method === "agent.attach" ? "agent_attach_ok" : "attach_ok";
        peer.reply(payload(reply));
        await call;
      } else {
        await expect(call, name).rejects.toMatchObject({ error_code: "invalid_params", uncertain: false });
      }
    }
    expect(peer.sent).toHaveLength(limits.grid.cases.filter(({ accepted }) => accepted).length);
    await expect(carrier.call("term.list", {})).rejects.toMatchObject({ error_code: "unknown_method" });
    await expect(carrier.call("term.attach", { session_id: "other" })).rejects.toMatchObject({ error_code: "invalid_params" });
  });

  it("retires a physically stalled send without treating absent screen ACK as a stall", async () => {
    vi.useFakeTimers();
    const { peer, carrier } = await connected();
    const originalSend = peer.send.bind(peer);
    peer.send = (bytes) => { originalSend(bytes); peer.bufferedAmount += bytes.length; };
    const close = vi.fn();
    carrier.onClose(close);
    peer.receive(messages.output.utf8);
    await vi.advanceTimersByTimeAsync(limits.sockets.physical_stall_ms);
    expect(close).not.toHaveBeenCalled();
    const pending = request(carrier, "input");
    const lost = expect(pending).rejects.toMatchObject({ uncertain: true });
    await vi.advanceTimersByTimeAsync(limits.sockets.physical_stall_ms - 1);
    expect(close).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(close).toHaveBeenCalledOnce();
    await lost;
  });

  it("marks handed-off input uncertain and drops unsent chunks on an opaque loss, then mints again", async () => {
    const { peer, carrier, ...h } = await connected();
    const paste = carrier.call("term.input", { term_id: "term-1", data: btoa("x".repeat(limits.decoded_input_bytes + 1)) });
    const next = request(carrier, "input");
    const uncertain = expect(paste).rejects.toMatchObject({ error_code: "uncertain_delivery", uncertain: true });
    const unsent = expect(next).rejects.toMatchObject({ error_code: "scope_ended", uncertain: false });
    const closed = vi.fn();
    carrier.onClose(closed);
    peer.drop(1006, "must not expose a peer's close text");
    await uncertain;
    await unsent;
    expect(closed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ closeCode: 1006, retryable: true }));
    expect(closed.mock.calls[0][0].message).not.toContain("must not expose");
    const recovered = await connected(h);
    expect(h.fetchImpl.mock.calls.filter(([url]) => url.endsWith(ticket.path))).toHaveLength(2);
    expect(recovered.peer.sent).toEqual([]);
    expect(peer.sent).toHaveLength(1);
  });

  it("fences frames and unsent writes when the owner is replaced", async () => {
    const { peer, carrier, replace } = await connected();
    const push = vi.fn();
    carrier.onPush(push);
    const input = request(carrier, "input");
    const uncertain = expect(input).rejects.toMatchObject({ uncertain: true });
    replace();
    peer.receive(messages.output.utf8);
    await uncertain;
    expect(push).not.toHaveBeenCalled();
    await expect(request(carrier, "input")).rejects.toMatchObject({ uncertain: false });
    expect(peer.sent).toHaveLength(1);
  });
});

function boundaryFrame(vector, peer) {
  const attach = ["term.attach", "agent.attach"].includes(vector.reply_to) && vector.ok;
  if (attach) {
    const result = payload(vector.reply_to === "agent.attach" ? "agent_attach_ok" : "attach_ok").result;
    return { bridge_epoch: opening.base.greeting.bridge_epoch, id: peer.calls().at(-1).id, ok: true, result: { ...result, snapshot: "" } };
  }
  if (vector.push === "term.reset" || vector.push === "term.output") return { ...payload(vector.push.slice(5)), data: "", cursor: limits.max_cursor };
  if (vector.push) return { ...payload(vector.push), extra: "" };
  if (!vector.ok) return { ...payload("oversized_attach"), id: peer.calls().at(-1).id, extra: "" };
  return { ...payload("attach_ok"), id: vector.reply_to ? peer.calls().at(-1).id : "unknown", result: { extra: "" } };
}

function fillBoundary(frame, bytes) {
  const serialized = JSON.stringify(frame);
  const filler = "A".repeat(Math.floor((bytes - serialized.length) / 4) * 4);
  const utf8 = serialized.replace(':""', `:"${filler}"`);
  return utf8.padEnd(bytes, " ");
}

describe("terminal receive budgets", () => {
  it.each(limits.message_cases.filter(({ budget }) => budget !== "client"))("enforces the inclusive budget for $name", async (vector) => {
    const { peer, carrier } = await connected();
    const push = vi.fn();
    const close = vi.fn();
    carrier.onPush(push);
    carrier.onClose(close);
    const pending = vector.reply_to ? carrier.call(vector.reply_to, {}) : null;
    void pending?.catch(() => {});
    peer.receive(fillBoundary(boundaryFrame(vector, peer), vector.bytes));
    if (!vector.accepted) {
      expect(close).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ closeCode: vector.close_code }));
    } else if (pending) {
      const result = await pending;
      expect(result.snapshot.length).toBeGreaterThan(limits.message_bytes.ordinary - 1024);
    } else {
      expect(push).toHaveBeenCalledOnce();
      expect(close).not.toHaveBeenCalled();
    }
  });

  it("bounds snapshot metadata by its encoded byte length even below the ordinary limit", async () => {
    const { peer, carrier } = await connected();
    const closed = vi.fn();
    carrier.onClose(closed);
    const reset = { ...payload("reset"), extra: "é".repeat(limits.snapshot_bound.json_metadata_bytes / 2) };
    const encoded = JSON.stringify(reset);
    expect(encoded.length).toBeLessThan(limits.snapshot_bound.json_metadata_bytes);
    peer.receive(encoded);
    expect(closed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ closeCode: 1009 }));
  });

  it.each(fixture.invalid_messages.filter(({ name }) => [
    "text_frame", "invalid_utf8", "utf8_bom", "invalid_json", "non_object_envelope", "excess_nesting",
  ].includes(name)))("classifies malformed content for $name", async (vector) => {
    const { peer, carrier } = await connected();
    const closed = vi.fn();
    carrier.onClose(closed);
    if (vector.opcode === 1) peer.receiveRaw(vector.utf8);
    else if (vector.hex) peer.receiveRaw(Uint8Array.from(Buffer.from(vector.hex, "hex")).buffer);
    else peer.receive(vector.utf8);
    expect(closed).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ closeCode: vector.close_code }));
  });

  it.each(fixture.epochs)("checks epoch before correlation or delivery for $name", async (vector) => {
    const { peer, carrier } = await connected();
    const frame = payload(vector.message);
    const push = vi.fn();
    const close = vi.fn();
    carrier.onPush(push);
    carrier.onClose(close);
    if (frame.id) {
      const pending = request(carrier, "attach");
      void pending.catch(() => {});
      frame.id = peer.calls()[0].id;
    }
    if (vector.bridge_epoch === null) delete frame.bridge_epoch;
    else frame.bridge_epoch = vector.bridge_epoch;
    peer.receive(JSON.stringify(frame));
    expect(close).toHaveBeenCalledTimes(vector.accepted ? 0 : 1);
    expect(push).toHaveBeenCalledTimes(vector.accepted && frame.type ? 1 : 0);
  });
});
