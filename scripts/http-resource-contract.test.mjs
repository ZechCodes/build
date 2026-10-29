// Cross-check generated OpenAPI and portable clients without a second catalogue.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const root = new URL("../", import.meta.url);
const read = (path) => JSON.parse(readFileSync(new URL(path, root), "utf8"));
const manifest = read("fixtures/api/http/resource-manifest.json");
const openapi = read("fixtures/api/http/openapi.json");

function reference(ref) {
  assert.ok(ref.startsWith("#/"), `Unexpected external schema reference ${ref}`);
  return ref.slice(2).split("/").reduce((value, key) => value[key], openapi);
}

test("portable operation IDs match every generated resource and no lifecycle claims", () => {
  assert.equal(manifest.version, 1);
  assert.equal(openapi["x-build-resource-manifest-version"], 1);
  assert.deepEqual(manifest.transport.operation_ids, manifest.operations.map((op) => op.operationId));
  assert.equal(manifest.transport.terminal, "webrtc");
  assert.equal(manifest.transport.durable_events, false);
  const actual = Object.entries(openapi.paths).flatMap(([path, verbs]) => Object.entries(verbs).map(([verb, op]) => `${verb.toUpperCase()} ${path} ${op.operationId}`));
  const expected = manifest.operations.map((op) => `${op.verb} ${op.path} ${op.operationId}`);
  assert.deepEqual(actual.sort(), expected.sort());
  assert.ok(!openapi.paths["/v1/sessions"]);
  assert.ok(!openapi.paths["/v1/rpc"]);
});

test("all refs resolve and all shared fixtures remain unchanged transport examples", () => {
  function walk(value) {
    if (!value || typeof value !== "object") return;
    if (value.$ref) assert.ok(reference(value.$ref), value.$ref);
    for (const child of Object.values(value)) walk(child);
  }
  walk(openapi);
  for (const op of manifest.operations) {
    const spec = openapi.paths[op.path][op.verb.toLowerCase()];
    assert.equal(spec["x-build-method"], op.method);
    assert.equal(spec["x-build-request-max-bytes"], op.requestMaxBytes);
    assert.equal(spec["x-build-response-max-bytes"], op.responseMaxBytes);
    const contract = spec["x-build-contract"];
    if (!contract.fixture) continue;
    const fixture = read(contract.fixture);
    assert.deepEqual(spec.responses["200"].content["application/json"].examples.wire.value.result, fixture.result);
    if (op.verb === "GET") continue;
    const expected = {...fixture.params};
    for (const name of op.pathParams) delete expected[name];
    assert.deepEqual(spec.requestBody.content["application/json"].example, expected);
    assert.equal(spec.requestBody.content["application/json"].schema["x-build-schema-complete"], false);
  }
  assert.match(manifest.schemaLimitation, /incomplete/);
});

test("the sole event header is required exactly for actual binding-owned resources", () => {
  assert.equal(manifest.headers.subscription, "X-Build-Subscription");
  assert.ok(!JSON.stringify(manifest).includes("X-Build-Event-Binding"));
  const required = manifest.operations.filter((op) => op.requires_events).map((op) => op.method).sort();
  assert.deepEqual(required, ["changes.subscribe", "changes.unsubscribe", "fs.uploadAbort", "fs.uploadBegin", "fs.uploadChunk", "fs.uploadFinish", "session.hello", "writes.subscribe"].sort());
  for (const op of manifest.operations) {
    const spec = openapi.paths[op.path][op.verb.toLowerCase()];
    const headers = spec.parameters.filter((param) => param.$ref).map((param) => reference(param.$ref));
    assert.equal(headers.find((param) => param.name === "X-Build-Session").required, true);
    assert.equal(headers.find((param) => param.name === "X-Build-Subscription").required, op.requires_events);
  }
});

test("only the four reviewed operations exceed the default encoded request budget", () => {
  const actual = manifest.operations.filter((op) => op.requestMaxBytes > 1048576).map((op) => [op.method, op.requestMaxBytes]);
  assert.deepEqual(actual.sort(), [["fs.uploadChunk",5657944],["fs.write",1463640],["tasks.attach",7056044],["thread.attach",7056044]].sort());
  for (const op of manifest.operations) assert.equal(op.responseMaxBytes, 8388608);
});

// Legacy normalization is a separate shared-execution handoff, not v1 evidence.
test("legacy routes do not claim typed unknown-field validation before extraction", () => {
  for (const op of manifest.operations.filter((op) => !op.contract.source.startsWith("bridge/src/api/v1/"))) {
    const description = openapi.paths[op.path][op.verb.toLowerCase()].description;
    assert.match(description, /legacy normalization.*#908/i);
    assert.ok(!description.includes("unknown fields are validated by the typed registry"));
  }
});
