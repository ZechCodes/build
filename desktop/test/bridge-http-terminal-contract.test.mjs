import assert from "node:assert/strict";
import { isUtf8 } from "node:buffer";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";
import { compatibleTransport } from "../src/bridge-http-contract.mjs";

function readFixture(name) {
  return JSON.parse(readFileSync(new URL(`../../fixtures/api/http/${name}.json`, import.meta.url), "utf8"));
}

const contract = readFixture("terminal-websocket-v1");
const manifest = readFixture("resource-manifest");
const bytes = (vector) => Buffer.from(vector.utf8, "utf8");
const payload = (vector) => JSON.parse(bytes(vector).toString("utf8"));

test("native main can still read every Opening's legacy transport", () => {
  const { base, cases } = contract.opening;
  for (const vector of cases) {
    const transport = { ...base.transport, operation_ids: manifest.operations.map(({ operationId }) => operationId) };
    if (Object.hasOwn(vector, "descriptor")) transport.terminal_websocket = vector.descriptor;
    assert.doesNotThrow(() => compatibleTransport(manifest, transport), vector.name);
  }
  assert.equal(cases.find(({ name }) => name === "v1").desktop, "wss");
  assert.equal(cases.find(({ name }) => name === "partial_native_ipc").desktop, "webrtc");
  assert.equal(cases.find(({ name }) => name === "future_version").desktop, "webrtc");
  assert.equal(cases.find(({ name }) => name === "wrong_v1_protocol").desktop, "protocol_error");
});

test("native uses the exact trusted Origin and only accepts one selected protocol", () => {
  const { endpoint, handshake, ticket } = contract;
  const canonical = new URL(endpoint.bridge_https_origin);
  assert.equal(new URL(endpoint.websocket_url).host, canonical.host);
  assert.equal(new URL(endpoint.websocket_url).pathname, handshake.path);
  assert.equal(ticket.request.headers.origin, endpoint.caller_origin);
  assert.equal(handshake.request.headers.origin, endpoint.caller_origin);
  assert.equal(handshake.response.status, 101);
  const key = handshake.request.headers["sec-websocket-key"];
  const accept = createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
  assert.equal(handshake.response.headers["sec-websocket-accept"], accept);
  const token = ticket.response.body.ticket;
  assert.equal(Buffer.from(token, "base64url").byteLength, 32);
  assert.equal(Buffer.from(token, "base64url").toString("base64url"), token);
  for (const { header_values: values, accepted, name } of handshake.selections) {
    assert.equal(values.length === 1 && values[0] === handshake.protocol, accepted, name);
  }
});

test("native receives the exact epoch-stamped binary envelopes", () => {
  const { messages, opening, epochs } = contract;
  for (const vector of Object.values(messages)) {
    assert.equal(vector.opcode, 2);
    assert.equal(isUtf8(bytes(vector)), true);
    assert.deepEqual(Buffer.from(JSON.stringify(payload(vector))), bytes(vector));
  }
  const attach = payload(messages.attach_ok);
  const output = payload(messages.output);
  const reset = payload(messages.reset);
  assert.deepEqual(Buffer.from(payload(messages.input).params.data, "base64"), Buffer.from([0, 255, 97, 10]));
  assert.deepEqual(Buffer.from(reset.data, "base64"), Buffer.concat([Buffer.from(attach.result.snapshot, "base64"), Buffer.from(output.data, "base64")]));
  assert.equal(payload(messages.closed).reason, "closed");
  assert.equal(payload(messages.oversized_attach).retryable, false);
  assert.equal(payload(messages.oversized_attach).error_code, "snapshot_too_large");
  assert.deepEqual(payload(messages.attachment_error), {
    attach_id: attach.id,
    bridge_epoch: attach.bridge_epoch,
    reason: "snapshot_too_large",
    type: "attachment_error",
  });
  for (const vector of epochs) {
    assert.equal(vector.bridge_epoch === opening.base.greeting.bridge_epoch, vector.accepted, vector.name);
  }
});

test("native has separate bounded receive classes and byte-level invalid vectors", () => {
  const { limits, invalid_messages: invalid } = contract;
  assert.deepEqual(limits.message_bytes, { client: 128 * 1024, ordinary: 1024 ** 2, snapshot: 12 * 1024 ** 2 });
  for (const vector of limits.message_cases) {
    assert.equal(vector.bytes <= limits.message_bytes[vector.budget], vector.accepted, vector.name);
  }
  assert.deepEqual(limits.snapshot_eligible, { successful_reply_to: ["term.attach", "agent.attach"], push: "term.reset" });
  const malformed = Object.fromEntries(invalid.map((vector) => [vector.name, vector]));
  assert.equal(isUtf8(Buffer.from(malformed.invalid_utf8.hex, "hex")), false);
  assert.throws(() => payload(malformed.invalid_json), SyntaxError);
  assert.throws(() => payload(malformed.utf8_bom), SyntaxError);
  assert.equal(malformed.text_frame.opcode, 1);
  assert.equal(malformed.text_frame.close_code, 1003);
});
