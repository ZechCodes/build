import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

// Exercise the actual request/response path without spawning or signaling a process.
const source = readFileSync(new URL("../../src/harness/build-tools.ts", import.meta.url), "utf8");
const testSource = `${source}\nexport { McpStdioClient, DEFAULT_TIMEOUT_MS };\n//# sourceURL=build-tools-fixture.mjs`;
const { McpStdioClient, DEFAULT_TIMEOUT_MS } = await import(`data:text/javascript;base64,${Buffer.from(testSource).toString("base64")}`);

function requestFixture(t, timeoutMs = DEFAULT_TIMEOUT_MS, toolTimeoutMs) {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const client = new McpStdioClient("unused-fixture-command", "fixture-owner", timeoutMs, toolTimeoutMs);
  const writes = [];
  t.mock.method(client, "write", async (message) => { writes.push(message); });
  const stopped = t.mock.method(client, "stopChild", async () => {});
  const signaled = t.mock.method(process, "kill", () => true);
  t.after(() => client.close());
  return { client, writes, stopped, signaled };
}

for (const method of ["initialize", "tools/list"]) {
  test(`${method} retains its 30-second request deadline`, async (t) => {
    const { client, stopped } = requestFixture(t);
    const reply = client.request(method, {}).catch((error) => error);
    t.mock.timers.tick(29999);
    assert.equal(client.failure, undefined);
    t.mock.timers.tick(1);
    assert.match((await reply).message, /Build MCP request 1 timed out/);
    assert.equal(stopped.mock.callCount(), 1);
  });
}

test("tools/call keeps the same pending request beyond 30 seconds and delivers its reply", async (t) => {
  const { client, writes, stopped } = requestFixture(t);
  const reply = client.request("tools/call", { name: "comment_task", arguments: {} }).catch((error) => error);
  t.mock.timers.tick(31000);
  assert.equal(client.failure, undefined);
  assert.equal(client.pending.size, 1);
  assert.equal(stopped.mock.callCount(), 0);
  assert.equal(writes.length, 1);
  const result = { isError: false, content: [{ type: "text", text: "applied" }] };
  client.readLine(JSON.stringify({ jsonrpc: "2.0", id: writes[0].id, result }));
  assert.deepEqual(await reply, result);
  assert.equal(client.pending.size, 0);
  t.mock.timers.tick(310000);
  assert.equal(client.failure, undefined);
});

test("tools/call allows Rust's five-minute cap first and reports an uncertain outcome at 310 seconds", async (t) => {
  const { client, stopped, signaled } = requestFixture(t);
  const reply = client.request("tools/call", { name: "comment_task", arguments: {} }).catch((error) => error);
  t.mock.timers.tick(309999);
  assert.equal(client.failure, undefined);
  t.mock.timers.tick(1);
  const error = await reply;
  assert.match(error.message, /timed out/);
  assert.match(error.message, /call may have been applied; check its outcome before retrying/);
  await client.closePromise;
  assert.equal(client.pending.size, 0);
  assert.equal(stopped.mock.callCount(), 1);
  assert.deepEqual(signaled.mock.calls[0].arguments, [process.pid, "SIGTERM"]);
});

test("explicit fixture deadlines still bound tools/call without waiting minutes", async (t) => {
  const { client } = requestFixture(t, 30, 30);
  const reply = client.request("tools/call", {}).catch((error) => error);
  t.mock.timers.tick(30);
  assert.match((await reply).message, /call may have been applied; check its outcome before retrying/);
});

test("queued tools/call requests each receive their own reply budget after dispatch", async (t) => {
  const { client, writes, stopped } = requestFixture(t);
  const first = client.request("tools/call", { name: "first" }).catch((error) => error);
  const second = client.request("tools/call", { name: "second" }).catch((error) => error);
  t.mock.timers.tick(200000);
  client.readLine(JSON.stringify({ jsonrpc: "2.0", id: writes[0].id, result: "first applied" }));
  assert.equal(await first, "first applied");
  t.mock.timers.tick(200000);
  assert.equal(client.failure?.message, undefined);
  assert.equal(writes.length, 2);
  assert.deepEqual(writes.map((message) => message.params.name), ["first", "second"]);
  client.readLine(JSON.stringify({ jsonrpc: "2.0", id: writes[1].id, result: "second applied" }));
  assert.equal(await second, "second applied");
  t.mock.timers.tick(310000);
  assert.equal(client.failure, undefined);
  assert.equal(stopped.mock.callCount(), 0);
});

test("queued tools/call dispatches after a preceding JSON-RPC error", async (t) => {
  const { client, writes } = requestFixture(t);
  const first = client.request("tools/call", {}).catch((error) => error);
  const second = client.request("tools/call", {}).catch((error) => error);
  t.mock.timers.tick(200000);
  client.readLine(JSON.stringify({ jsonrpc: "2.0", id: writes[0].id, error: { code: -32000, message: "invalid arguments" } }));
  assert.equal((await first).message, "invalid arguments");
  t.mock.timers.tick(200000);
  assert.equal(client.failure?.message, undefined);
  assert.equal(writes.length, 2);
  client.readLine(JSON.stringify({ jsonrpc: "2.0", id: writes[1].id, result: "applied" }));
  assert.equal(await second, "applied");
});

test("terminal failure rejects queued tools/call without dispatching it", async (t) => {
  const { client, writes, stopped, signaled } = requestFixture(t);
  const first = client.request("tools/call", {}).catch((error) => error);
  const second = client.request("tools/call", {}).catch((error) => error);
  t.mock.timers.tick(310000);
  assert.equal(await first, client.failure);
  assert.equal(await second, client.failure);
  await client.closePromise;
  assert.equal(writes.length, 1);
  assert.equal(stopped.mock.callCount(), 1);
  assert.equal(signaled.mock.callCount(), 1);
});

test("close rejects queued tools/call without dispatching it", async (t) => {
  const { client, writes, signaled } = requestFixture(t);
  const first = client.request("tools/call", {}).catch((error) => error);
  const second = client.request("tools/call", {}).catch((error) => error);
  await client.close();
  for (const error of await Promise.all([first, second])) {
    assert.equal(error.message, "Build MCP session is shutting down");
  }
  t.mock.timers.tick(310000);
  assert.equal(writes.length, 1);
  assert.equal(client.failure, undefined);
  assert.equal(signaled.mock.callCount(), 0);
});

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
    if (scenario === "initial_sink_failure") mkdirSync(env.BUILD_PI_TELEMETRY_LOG);
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
  assert.deepEqual(result.threshold, [null]);
  assert.equal(result.records.at(-1).type, "shutdown");
  assert.deepEqual(result.signals, []);
  assert.equal(result.childRunning, false);
});

for (const [scenario, mode] of [["owner_mismatch", "normal"], ["initialization_failure", "invalid_initialize_object"], ["registration_failure", "normal"]]) {
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
  assert.deepEqual(result.toolResult, { content: [{ type: "text", text: "one" }], details: {} });
});

for (const scenario of ["initial_sink_failure", "sink_failure", "hook_failure", "lease_failure"]) {
  test(`${scenario} withdraws compaction while healthy MCP tools remain available`, () => {
    const result = run(scenario);
    assert.deepEqual(result.toolResult, { content: [{ type: "text", text: "one" }], details: {} });
    assert.equal(result.threshold.some((value) => value?.cancel), false);
    assert.deepEqual(result.signals, []);
    assert.equal(result.childRunning, false);
  });
}

test("actual MCP child death emits unsupported and restores Pi SIGTERM", () => {
  const result = run("mcp_failure", "child_exit");
  assert.equal(result.records.some((record) => record.type === "unsupported" && record.reason === "extension_failed"), true);
  assert.deepEqual(result.signals.map((signal) => signal.signal), ["SIGTERM"]);
  assert.equal(result.childRunning, false);
});

test("MCP framing failure during initialization is fatal rather than a semantic initialization error", () => {
  const result = run("transport_initialization_failure", "malformed_json");
  assert.deepEqual(result.signals.map((signal) => signal.signal), ["SIGTERM"]);
  assert.equal(result.childRunning, false);
});
