import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

function run(scenario, mode = "normal") {
  const directory = mkdtempSync(join(tmpdir(), "build-pi-default-"));
  try {
    const command = join(directory, "fake-mcp");
    const extension = join(directory, "build-tools.mjs");
    copyFileSync(new URL("pi-mcp-child.mjs", import.meta.url), command);
    chmodSync(command, 0o700);
    copyFileSync(new URL("../../src/harness/build-tools.ts", import.meta.url), extension);
    const policy = { tokens: 150000, enabled: true, owner: scenario === "owner_mismatch" ? "wrong-owner" : "agent-a", thread_id: "thread-a", launch_id: "launch-a" };
    // Coordinator-authorized fixture-only exemption: no inherited credentials,
    // only a scratch fake endpoint and a fixed dummy token for the fake child.
    const env = {
      HOME: directory, PATH: process.env.PATH,
      BUILD_PI_EXTENSION_PATH: extension,
      BUILD_PI_MCP_COMMAND: command, BUILD_PI_MCP_OWNER: "agent-a", BUILD_PI_MCP_TIMEOUT_MS: "3000",
      BRIDGE_MCP_SOCKET: join(directory, "fake-mcp.sock"), BRIDGE_MCP_TOKEN: "fixture-dummy-token",
      BUILD_PI_COMPACTION_POLICY: JSON.stringify(policy),
      FAKE_MCP_MODE: mode, FAKE_MCP_PID: join(directory, "mcp.pid"),
      PI_DEFAULT_SCENARIO: scenario,
    };
    if (scenario !== "missing_log") env.BUILD_PI_TELEMETRY_LOG = join(directory, "telemetry.jsonl");
    const result = spawnSync(process.execPath, [new URL("pi-default-extension-driver.mjs", import.meta.url).pathname], { env, timeout: 10000, encoding: "utf8" });
    assert.equal(result.status, 0, JSON.stringify({ status: result.status, signal: result.signal, stderr: result.stderr }));
    return JSON.parse(result.stdout);
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

test("default export initializes policy telemetry before MCP and qualifies ready", () => {
  const result = run("normal");
  assert.equal(result.initializationError, undefined);
  assert.equal(result.records[0].type, "starting");
  assert.equal(result.records[0].sequence, 1);
  assert.equal(result.records[0].generation, 1);
  assert.equal(result.records.some((record) => record.type === "ready"), true);
  assert.deepEqual(result.threshold, [{ cancel: true }]);
  assert.equal(result.records.at(-1).type, "shutdown");
  assert.deepEqual(result.signals, []);
  assert.equal(result.childRunning, false);
});

for (const [scenario, mode] of [["owner_mismatch", "normal"], ["initialization_failure", "invalid_initialize_object"], ["registration_failure", "normal"], ["mcp_failure", "child_exit"]]) {
  test(`default export ${scenario} withdraws support and only closes its MCP child`, () => {
    const result = run(scenario, mode);
    assert.equal(result.initializationError, undefined);
    assert.equal(result.records.some((record) => record.type === "unsupported" && record.reason === "extension_failed"), true);
    assert.equal(result.records.every((record) => record.owner === "agent-a"), true);
    assert.equal(result.threshold.some((value) => value?.cancel), false);
    assert.deepEqual(result.signals, []);
    assert.equal(result.childRunning, false);
  });
}

test("missing telemetry path leaves Pi usable and native compaction uncancelled", () => {
  const result = run("missing_log");
  assert.equal(result.initializationError, undefined);
  assert.equal(result.threshold.some((value) => value?.cancel), false);
  assert.deepEqual(result.signals, []);
  assert.equal(result.childRunning, false);
});
