import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const source = readFileSync(new URL("../../src/harness/build-tools.ts", import.meta.url), "utf8");
const extension = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);
const policy = { tokens: 150000, enabled: true, owner: "agent-a", thread_id: "thread-a", launch_id: "launch-a" };

function fixture(configuration = policy, sink, start = true) {
  const handlers = new Map();
  const records = [];
  const failures = [];
  const pi = { on: (name, handler) => handlers.set(name, handler) };
  const telemetry = extension.registerCompactionTelemetry(pi, configuration, sink ?? ((record) => records.push(record)), (reason) => failures.push(reason));
  let usage = { tokens: 149999, contextWindow: 200000 };
  let idle = false;
  const ctx = { sessionManager: { getSessionId: () => configuration.owner }, hasPendingMessages: () => false, getContextUsage: () => usage, isIdle: () => idle, compact: () => assert.fail("manual compaction must remain wrapper-owned") };
  if (start) handlers.get("session_start")?.({}, ctx);
  return { handlers, records, failures, telemetry, ctx,
    usage: (value) => { usage = value; }, idle: (value) => { idle = value; },
    emit: async (name, event = {}) => handlers.get(name)?.(event, ctx),
  };
}

test("qualified policy cancels threshold only and preserves manual/overflow recovery", async () => {
  const f = fixture();
  assert.deepEqual(await f.emit("session_before_compact", { reason: "threshold" }), { cancel: true });
  for (const reason of ["manual", "overflow"]) {
    assert.equal(await f.emit("session_before_compact", { reason }), undefined);
  }
  assert.equal(f.records.filter((r) => r.type === "compaction").every((r) => !r.completed), true);
  const passive = fixture({ ...policy, enabled: false });
  assert.equal(await passive.emit("session_before_compact", { reason: "threshold" }), undefined);
});

test("long tool turn emits working/context and settles only at idle agent_settled", async () => {
  const f = fixture();
  assert.equal(f.records[0].type, "starting");
  await f.emit("agent_start");
  f.usage({ tokens: 180000, contextWindow: 200000 });
  await f.emit("tool_execution_start");
  await f.emit("tool_execution_end");
  await f.emit("turn_end");
  await f.emit("agent_end");
  assert.equal(f.records.some((r) => r.type === "settled"), false);
  assert.equal(f.records.some((r) => r.type === "context" && r.tokens === 180000), true);
  await f.emit("agent_settled");
  assert.equal(f.records.some((r) => r.type === "settled"), false);
  f.idle(true);
  await f.emit("agent_settled");
  assert.equal(f.records.at(-1).type, "settled");
  assert.equal(f.records.filter((r) => r.type === "working").length >= 2, true);
});

test("saved compaction success invalidates old usage before settling", async () => {
  const f = fixture();
  await f.emit("agent_start");
  f.usage({ tokens: null, contextWindow: 200000 });
  await f.emit("session_compact", { reason: "overflow", compactionEntry: { type: "compaction" } });
  assert.deepEqual(f.records.slice(-2).map((r) => [r.type, r.completed, r.tokens]), [
    ["compaction", true, undefined], ["context", undefined, null],
  ]);
  f.usage(undefined);
  f.idle(true);
  await f.emit("agent_settled");
  assert.deepEqual(withoutSequence(f.records.at(-2)), { ...identity(), type: "context", tokens: null, window: null });
});

function identity(configuration = policy) {
  return { owner: configuration.owner, thread_id: configuration.thread_id, launch_id: configuration.launch_id };
}

test("summary failure, abort and cancelled threshold never report success or raw error", async () => {
  const f = fixture();
  await f.emit("session_before_compact", { reason: "threshold" });
  for (const aborted of [true, false]) {
    await f.emit("session_compact_failed", { reason: "overflow", aborted, errorMessage: "secret provider payload" });
  }
  assert.equal(f.records.some((r) => r.completed === true), false);
  assert.equal(JSON.stringify(f.records).includes("secret"), false);
});

test("identity and policy remain isolated and immutable after registration", async () => {
  const mutable = { ...policy };
  const first = fixture(mutable);
  const second = fixture({ ...policy, owner: "agent-b", thread_id: "thread-b", launch_id: "launch-b", enabled: false });
  mutable.owner = "changed";
  mutable.enabled = false;
  await first.emit("agent_start");
  await second.emit("agent_start");
  assert.equal(await second.emit("session_before_compact", { reason: "threshold" }), undefined);
  assert.deepEqual(await first.emit("session_before_compact", { reason: "threshold" }), { cancel: true });
  assert.equal(first.records.every((r) => r.owner === "agent-a"), true);
  assert.equal(second.records.every((r) => r.owner === "agent-b"), true);
});

test("context hook failure reports fixed unsupported reason and withdraws cancellation", async () => {
  const f = fixture();
  f.ctx.getContextUsage = () => { throw new Error("secret provider payload"); };
  await f.emit("agent_start");
  await f.emit("agent_settled");
  assert.deepEqual(f.failures, ["telemetry_failed"]);
  assert.deepEqual(withoutSequence(f.records.at(-1)), { ...identity(), type: "unsupported", reason: "telemetry_failed" });
  assert.equal(JSON.stringify(f.records).includes("secret"), false);
});

test("telemetry write failure withdraws cancellation without stopping Pi", async () => {
  let writes = 0;
  const f = fixture(policy, () => { if (++writes > 1) throw new Error("private filesystem path"); });
  await f.emit("agent_start");
  assert.deepEqual(f.failures, ["telemetry_failed"]);
});

test("invalid policy and missing identity fail closed", () => {
  for (const patch of [{ tokens: 0 }, { tokens: -1 }, { tokens: 1.5 }, { tokens: Number.MAX_SAFE_INTEGER + 1 }, { enabled: "true" }, { owner: "" }, { thread_id: "" }, { launch_id: "" }]) {
    assert.throws(() => fixture({ ...policy, ...patch }), /invalid compaction policy/);
  }
});

test("default sink writes scoped JSONL with private permissions", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-compaction-extension-"));
  const path = join(dir, "telemetry.jsonl");
  try {
    const handlers = new Map();
    const telemetry = extension.registerCompactionTelemetry({ on: (name, fn) => handlers.set(name, fn) }, { ...policy, log: path });
    handlers.get("session_start")({}, { sessionManager: { getSessionId: () => policy.owner }, getContextUsage: () => undefined });
    telemetry.unsupported("extension_failed");
    const lines = readFileSync(path, "utf8").trim().split("\n").map(JSON.parse);
    assert.deepEqual(lines.map(withoutSequence).filter((r) => !["context", "starting"].includes(r.type)), [{ ...identity(), type: "ready" }, { ...identity(), type: "unsupported", reason: "extension_failed" }]);
    assert.equal(statSync(path).mode & 0o777, 0o600);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

function withoutSequence({ sequence: _sequence, generation: _generation, ...record }) { return record; }

test("ready waits for session identity and all events have increasing sequence", async () => {
  const f = fixture(policy, undefined, false);
  assert.deepEqual(f.records.map((r) => r.type), ["starting"]);
  await f.emit("session_start");
  await f.emit("agent_start");
  assert.equal(f.records[0].type, "starting");
  assert.deepEqual(f.records.map((r) => r.sequence), f.records.map((_r, i) => i + 1));
  const wrong = fixture(policy, undefined, false);
  wrong.ctx.sessionManager.getSessionId = () => "other-session";
  await wrong.emit("session_start");
  assert.equal(wrong.records.some((r) => r.type === "ready"), false);
  assert.equal(wrong.records.at(-1).type, "unsupported");
  assert.deepEqual(wrong.failures, ["session_identity_mismatch"]);
});

test("unsupported unsafe integer policy stays passive without rounding", async () => {
  const f = fixture({ ...policy, tokens: null, tokens_decimal: "18446744073709551615", enabled: false });
  assert.equal(await f.emit("session_before_compact", { reason: "threshold" }), undefined);
  assert.throws(() => fixture({ ...policy, tokens: null, tokens_decimal: "18446744073709551615" }), /invalid compaction policy/);
});

test("manual success settles after native cleanup only if idle and no pending continuation", async () => {
  const f = fixture();
  await f.emit("session_compact", { reason: "manual" });
  assert.equal(f.records.some((r) => r.type === "settled"), false);
  f.idle(true);
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(f.records.at(-1).type, "settled");
  const pending = fixture();
  pending.idle(true);
  pending.ctx.hasPendingMessages = () => true;
  await pending.emit("session_compact", { reason: "manual" });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(pending.records.some((r) => r.type === "settled"), false);
});

test("deferred manual cleanup cannot settle a newer run or duplicate its settlement", async () => {
  const f = fixture();
  await f.emit("session_compact", { reason: "manual" });
  await f.emit("agent_start");
  f.idle(true);
  await f.emit("agent_settled");
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(f.records.filter((r) => r.type === "settled").length, 1);
});

test("manual failure and abort release idle boundary without success", async () => {
  for (const aborted of [true, false]) {
    const f = fixture();
    f.idle(true);
    await f.emit("session_compact_failed", { reason: "manual", aborted, errorMessage: "private error body" });
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(f.records.at(-1).type, "settled");
    assert.equal(f.records.some((r) => r.completed === true), false);
    assert.equal(f.records.find((r) => r.type === "compaction").failed, true);
  }
});


test("shutdown and reload use fresh generations while old handlers stop cancelling", async () => {
  const first = fixture();
  const firstGeneration = first.records[0].generation;
  assert.equal(Number.isSafeInteger(firstGeneration) && firstGeneration > 0, true);
  await first.emit("session_shutdown");
  assert.equal(first.records.at(-1).type, "shutdown");
  assert.equal(await first.emit("session_before_compact", { reason: "threshold" }), undefined);
  const second = fixture();
  assert.equal(second.records[0].generation, firstGeneration + 1);
  assert.equal(second.records[0].sequence, 1);
  assert.equal(second.records[0].type, "starting");
  assert.deepEqual(await second.emit("session_before_compact", { reason: "threshold" }), { cancel: true });
  assert.equal(second.records.every((record) => record.generation === firstGeneration + 1), true);
  await second.emit("session_shutdown");
});

test("heartbeats cover startup without authorizing work and stop on failure", async () => {
  const f = fixture(policy, undefined, false);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(f.records.some((record) => record.type === "heartbeat"), true);
  assert.equal(f.records.some((record) => ["working", "settled", "ready"].includes(record.type)), false);
  f.telemetry.unsupported("extension_failed");
  const stopped = f.records.length;
  assert.equal(await f.emit("session_before_compact", { reason: "threshold" }), undefined);
  assert.equal(await f.emit("session_before_compact", { reason: "overflow" }), undefined);
  assert.equal(await f.emit("session_before_compact", { reason: "manual" }), undefined);
  await new Promise((resolve) => setTimeout(resolve, 1100));
  assert.equal(f.records.length, stopped);
});

test("Pi loader timer handles without unref do not disable telemetry", async () => {
  const timer = globalThis.setInterval;
  globalThis.setInterval = () => 42;
  try {
    const f = fixture();
    assert.equal(f.records.some((record) => record.type === "ready"), true);
    assert.deepEqual(f.failures, []);
    await f.emit("session_shutdown");
  } finally { globalThis.setInterval = timer; }
});
