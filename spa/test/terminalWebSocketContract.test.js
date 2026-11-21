import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { createBridgeEpoch } from "../src/core/bridgeEpoch.js";
import { terminalWebSocketMode, validateHttpOpening } from "../src/core/httpSession.js";

function readFixture(name) {
  return JSON.parse(readFileSync(new URL(`../../fixtures/api/http/${name}.json`, import.meta.url), "utf8"));
}

const contract = readFixture("terminal-websocket-v1");
const manifest = readFixture(contract.opening.operation_ids_from.fixture.replace(/\.json$/u, ""));
const routes = { operationIds: () => manifest.operations.map(({ operationId }) => operationId) };
const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const encode = new TextEncoder();
const binary = (vector) => encode.encode(vector.utf8).buffer;
const payload = (vector) => JSON.parse(utf8.decode(binary(vector)));

describe("terminal WebSocket v1 browser vectors", () => {
  it("keeps the legacy Opening valid with and without the additive descriptor", () => {
    const { base, capability, descriptor, cases } = contract.opening;
    for (const vector of cases) {
      const opening = structuredClone(base);
      opening.transport.operation_ids = routes.operationIds();
      if (Object.hasOwn(vector, "descriptor")) opening.transport.terminal_websocket = vector.descriptor;
      if (vector.capability) opening.greeting.capabilities.push(capability);
      expect(() => validateHttpOpening(opening, routes), vector.name).not.toThrow();
      expect(terminalWebSocketMode(opening), vector.name).toBe(vector.browser);
    }
    expect(base.transport.terminal).toBe("webrtc");
    expect(cases.find(({ name }) => name === "old_bridge").browser).toBe("webrtc");
    expect(cases.find(({ name }) => name === "v1").descriptor).toEqual(descriptor);
    expect(cases.find(({ name }) => name === "v1").browser).toBe("wss");
  });

  it("uses the verified port, rejects the cookie hostname boundary and requires exact protocol selection", () => {
    const { endpoint, handshake, ticket } = contract;
    const wss = new URL(handshake.path, endpoint.bridge_https_origin);
    wss.protocol = "wss:";
    expect(wss.href).toBe(endpoint.websocket_url);
    for (const { bridge_https_origin: bridge, allowed } of endpoint.browser_host_cases) {
      expect(new URL(bridge).hostname !== new URL(endpoint.caller_origin).hostname).toBe(allowed);
    }
    const app = new URL(endpoint.caller_origin).hostname;
    const hostCase = (name) => new URL(endpoint.browser_host_cases.find((vector) => vector.name === name).bridge_https_origin).hostname;
    expect(hostCase("subdomain_of_app_host").endsWith(`.${app}`)).toBe(true);
    expect(hostCase("sibling_hostname").split(".").slice(1)).toEqual(app.split(".").slice(1));
    expect(handshake.offers[0].tokens).toEqual(["build.term.v1", `ticket.${ticket.response.body.ticket}`]);
    for (const { browser_protocol: selected, accepted } of handshake.selections) {
      expect(selected === handshake.protocol).toBe(accepted);
    }
    expect(endpoint.connect_src_sources).toEqual([endpoint.bridge_https_origin, wss.origin]);
    expect(ticket.request.body.opening_generation).toBe(contract.opening.base.opening_generation);
    expect(ticket.response.body.opening_generation).toBe(ticket.request.body.opening_generation);
    for (const sample of ticket.lifetime_cases) {
      const deadline = Math.min(Date.parse(sample.proof_deadline), Date.parse(sample.issued_at) + contract.limits.tickets.max_age_ms);
      expect(Date.parse(sample.expires_at)).toBe(deadline);
    }
  });

  it("keeps opaque browser upgrade failures on WSS and reports capacity from ticket POST", () => {
    const failures = contract.opening.selected_wss_failures;
    expect(failures.find(({ cause }) => cause === "opaque_browser_upgrade_failure")).toMatchObject({
      transport: "wss", http_status_visible: false, close_code: 1006, fresh_ticket: true, backoff: "bounded_jitter",
    });
    expect(failures.find(({ cause }) => cause === "capacity")).toMatchObject({
      transport: "wss", source: "ticket_post", http_status: 429, pane_message: "Terminal connection limit reached",
    });
  });

  it("reads an authority field in params as a refusal to the original call", () => {
    const vector = contract.invalid_messages.find(({ name }) => name === "caller_authority_in_params");
    const request = payload(vector);
    expect(request.params.session_id).toBe("other");
    expect(vector).toMatchObject({ dispatch: false, socket_open: true });
    expect(payload(contract.messages[vector.reply])).toMatchObject({
      id: request.id, ok: false, error_code: "invalid_params", retryable: false,
    });
  });

  it("keeps exact binary input bytes, ordered reply/push fixtures and pane-local controls", () => {
    const m = contract.messages;
    expect(binary(m.input)).toEqual(encode.encode('{"id":"input-1","method":"term.input","params":{"data":"AP9hCg==","term_id":"term-1"}}').buffer);
    expect(Array.from(atob(payload(m.input).params.data), (byte) => byte.charCodeAt(0))).toEqual([0, 255, 97, 10]);
    for (const vector of Object.values(m)) {
      expect(vector.opcode).toBe(2);
      expect(JSON.stringify(payload(vector))).toBe(vector.utf8);
    }
    const attach = payload(m.attach_ok);
    const output = payload(m.output);
    const reset = payload(m.reset);
    expect(attach.id).toBe(payload(m.attach).id);
    expect(output.cursor - atob(output.data).length).toBe(attach.result.cursor);
    expect(reset.cursor).toBe(output.cursor);
    expect(atob(reset.data)).toBe(atob(attach.result.snapshot) + atob(output.data));
    expect(payload(m.closed)).toMatchObject({ type: "term.closed", term_id: "term-1", reason: "closed" });
    expect(payload(m.agent_session_ended).reason).toBe("agent_session_ended");
    expect(payload(m.oversized_attach)).toMatchObject({ error_code: "snapshot_too_large", retryable: false });
    expect(payload(m.attachment_error)).toMatchObject({ type: "attachment_error", attach_id: attach.id, reason: "snapshot_too_large" });
    expect(payload(m.detach)).toEqual({ type: "detach", attach_id: attach.id });
  });

  it("requires the greeting epoch even for a matching reply ID and an attachment control", () => {
    const epoch = createBridgeEpoch();
    const greeting = contract.opening.base.greeting;
    epoch.acceptGreeting(greeting);
    for (const vector of contract.epochs) {
      const message = payload(contract.messages[vector.message]);
      if (vector.bridge_epoch === null) delete message.bridge_epoch;
      else message.bridge_epoch = vector.bridge_epoch;
      expect(epoch.accepts(message), vector.name).toBe(vector.accepted);
    }
    expect(epoch.current().bridge_epoch).toBe(greeting.bridge_epoch);
  });

  it("keeps the snapshot exception tied to the outstanding attach method or reset", () => {
    const limits = contract.limits;
    for (const vector of limits.message_cases) {
      expect(vector.accepted, vector.name).toBe(vector.bytes <= limits.message_bytes[vector.budget]);
    }
    expect(limits.snapshot_eligible).toEqual({ successful_reply_to: ["term.attach", "agent.attach"], push: "term.reset" });
    expect(limits.message_bytes).toMatchObject({ client: 128 * 1024, ordinary: 1024 ** 2, snapshot: 12 * 1024 ** 2 });
    const { raw_bytes: raw, json_metadata_bytes: metadata, encoded_bytes: encoded } = limits.snapshot_bound;
    expect(encoded).toBe(4 * Math.ceil(raw / 3) + metadata);
    expect(encoded).toBeLessThan(limits.message_bytes.snapshot);
  });

  it("distinguishes invalid UTF-8, invalid JSON and WebSocket text from binary RPC", () => {
    const invalid = Object.fromEntries(contract.invalid_messages.map((vector) => [vector.name, vector]));
    const badBytes = Uint8Array.from(invalid.invalid_utf8.hex.match(/../gu), (byte) => Number.parseInt(byte, 16));
    expect(() => utf8.decode(badBytes)).toThrow(TypeError);
    expect(invalid.invalid_utf8.close_code).toBe(1007);
    expect(() => payload(invalid.invalid_json)).toThrow(SyntaxError);
    expect(() => payload(invalid.utf8_bom)).toThrow(SyntaxError);
    expect(invalid.invalid_json.close_code).toBe(1002);
    expect(payload(invalid.text_frame)).toEqual(payload(contract.messages.input));
    expect(invalid.text_frame.opcode).toBe(1);
    expect(invalid.text_frame.close_code).toBe(1003);
    const idPattern = new RegExp(contract.wire.request_id_pattern, "u");
    for (const name of ["numeric_id", "empty_id", "unicode_id", "oversized_id"]) {
      const { id } = payload(invalid[name]);
      expect(typeof id === "string" && idPattern.test(id), name).toBe(false);
    }
  });
});
