import assert from "node:assert/strict";
import test from "node:test";
import { createHash } from "node:crypto";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import {
  buildRoute,
  compatibleTransport,
} from "../src/bridge-http-contract.mjs";
import { networkOptions } from "../src/bridge-http-network.mjs";

const artifact = new URL("../src/bridge-http-resources.json", import.meta.url);
const manifest = () => JSON.parse(readFileSync(artifact));
const expectedHash =
  "bf75fca58d9ea75db89092d65621d99ca422bda7b164dfcabb3c40a1c713cd97";

test("packaged manifest is byte-identical to the coordinator-released committed artifact", () => {
  assert.equal(
    createHash("sha256").update(readFileSync(artifact)).digest("hex"),
    expectedHash,
  );
  const provenance = JSON.parse(
    readFileSync(
      new URL("../src/bridge-http-resources.provenance.json", import.meta.url),
    ),
  );
  assert.equal(
    provenance.predecessor.sourceCommit,
    "79d60d9526e6d2acbe8a49bf307395c3bb45c4be",
  );
  assert.equal(
    provenance.predecessor.sha256,
    "bcada2539a96e5784e864abf4e17564bb2d7260ac612f729db4dd5b7adbde616",
  );
  assert.equal(provenance.sourceTask, 913);
  assert.equal(provenance.sha256, expectedHash);
});

test("all 149 reconciled operations preserve method, path, body, event and byte-limit contracts", () => {
  const m = manifest();
  assert.equal(m.version, 1);
  assert.equal(m.operations.length, 149);
  assert.deepEqual(
    [...new Set(m.operations.map((o) => o.operationId))].sort(),
    m.transport.operation_ids,
  );
  for (const operation of m.operations) {
    assert.deepEqual(operation.queryParams, []);
    const pathParams = Object.fromEntries(
      operation.pathParams.map((key) => [key, "id & /value"]),
    );
    const params =
      operation.verb === "GET"
        ? pathParams
        : { ...pathParams, selector: { cursor: "a & b", kinds: ["open"] } };
    const route = buildRoute(m, operation.operationId, params);
    const expectedPath = operation.path.replace(
      /\{[^}]+\}/gu,
      encodeURIComponent("id & /value"),
    );
    assert.equal(route.path, expectedPath);
    assert.equal(route.method, operation.verb);
    assert.equal(route.maxBody, operation.requestMaxBytes);
    assert.equal(route.maxBytes, operation.responseMaxBytes);
    assert.equal(route.requiresEvents, operation.requires_events);
    const init = networkOptions(route);
    if (operation.verb === "GET") {
      assert.equal(init.body, undefined);
      assert.equal(init.headers["Content-Type"], undefined);
      assert.throws(
        () =>
          buildRoute(m, operation.operationId, {
            ...params,
            selector: "must not vanish",
          }),
        { code: "invalid_params" },
      );
    } else
      assert.deepEqual(JSON.parse(init.body), {
        selector: { cursor: "a & b", kinds: ["open"] },
      });
  }
  assert.doesNotThrow(() => compatibleTransport(m, m.transport));
  assert.throws(
    () =>
      compatibleTransport(m, {
        ...m.transport,
        operation_ids: m.transport.operation_ids.slice(1),
      }),
    { code: "incompatible_transport" },
  );
});

test("reviewed upload exceptions and stream requirements remain exact", () => {
  const m = manifest();
  const route = (method) => m.operations.find((o) => o.method === method);
  assert.equal(route("tasks.attach").requestMaxBytes, 7056044);
  assert.equal(route("thread.attach").requestMaxBytes, 7056044);
  assert.equal(route("fs.uploadChunk").requestMaxBytes, 5657944);
  assert.equal(route("fs.write").requestMaxBytes, 1463640);
  for (const operation of m.operations)
    assert.equal(operation.responseMaxBytes, 8388608);
  for (const method of [
    "changes.subscribe",
    "changes.unsubscribe",
    "writes.subscribe",
    "fs.uploadBegin",
    "fs.uploadChunk",
    "fs.uploadFinish",
    "fs.uploadAbort",
  ])
    assert.equal(route(method).requires_events, true, method);
});

test("desktop source archive includes real manifest at the main import path", async (t) => {
  const require = createRequire(import.meta.url);
  const asar = require("@electron/asar");
  const directory = mkdtempSync(join(tmpdir(), "desktop-manifest-asar-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const archive = join(directory, "app.asar");
  await asar.createPackage(
    new URL("../src/", import.meta.url).pathname,
    archive,
  );
  const content = asar.extractFile(archive, "bridge-http-resources.json");
  assert.equal(
    createHash("sha256").update(content).digest("hex"),
    expectedHash,
  );
  assert.match(
    asar.extractFile(archive, "main.mjs").toString(),
    /import resourceManifest from "\.\/bridge-http-resources\.json"/u,
  );
  assert.ok(asar.extractFile(archive, "preload.cjs").length);
  const config = JSON.parse(
    readFileSync(new URL("../package.json", import.meta.url)),
  );
  assert.ok(config.build.files.includes("src/**/*"));
  assert.equal(config.dependencies["eventsource-parser"], "4.1.2");
});
