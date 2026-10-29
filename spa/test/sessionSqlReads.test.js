import { transportEpoch } from "./transportEpoch.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ANSWER_TIMEOUT_MS, SQL_ADMISSION_TIMEOUT_MS, createSessionRpc } from "../src/core/sessionRpc.js";
import { normalizeError } from "../src/core/bridgeApi/index.js";
import { isTransientTransportError } from "../src/core/transientRead.js";
import { SQL_READ_METHODS } from "../src/core/sqlReadCredits.js";
import { readFileSync } from "node:fs";

const METHODS = ["thread.page", "thread.activity", "thread.operation", "thread.revision", "tasks.list", "tasks.get", "tasks.waiting_on.blocks", "tasks.filterChoices", "tasks.counts", "tasks.lookup"];
const WRITE_READS = ["writes.status", "writes.subscribe"];
const PACED_METHODS = [...METHODS, ...WRITE_READS];
const flush = async () => { for (let turn = 0; turn < 20; turn += 1) await Promise.resolve(); };
const transport = {
  encryptFrame: async ({ frameFields }) => frameFields,
  decryptEnvelope: async ({ envelope }) => envelope,
};

function carrier() {
  const envelopes = new Set();
  const closes = new Set();
  return {
    sent: [],
    send(frame) { this.sent.push(frame.payload); },
    onEnvelope(fn) { envelopes.add(fn); return () => envelopes.delete(fn); },
    onClose(fn) { closes.add(fn); return () => closes.delete(fn); },
    deliver(payload) { for (const fn of envelopes) fn({ payload }); },
    close() { for (const fn of closes) fn(new Error("the channel closed")); },
    listeners: () => envelopes.size,
  };
}

const sessions = [];
function setup(overrides = {}) {
  const wire = carrier();
  const rpc = createSessionRpc({ bridgeEpoch: transportEpoch(), transport, deviceId: "device", sessionId: "opening", sessionKeyB64: "key", timeoutMs: 0, ...overrides });
  rpc.rideOn(wire);
  sessions.push(rpc);
  const ask = (method = "thread.page", options = {}) => {
    const answer = rpc.call(method, {}, options);
    answer.catch(() => {});
    return answer;
  };
  return { rpc, wire, ask };
}

afterEach(() => {
  for (const rpc of sessions.splice(0)) rpc.close(new Error("test ended"));
  vi.useRealTimers();
});

describe("SQL reads share eight sender credits per opening", () => {
  it("reports queued, admitted and timed-out held credits after caller promises settle", async () => {
    vi.useFakeTimers();
    const { rpc, wire, ask } = setup({ sqlReadLimit: 1 });
    const timedOut = ask("tasks.get", { timeoutMs: 10 });
    ask("tasks.list");
    await flush();
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 1, timedOutHeld: 0 });
    await vi.advanceTimersByTimeAsync(11);
    await expect(timedOut).rejects.toMatchObject({ timedOut: true });
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 1, timedOutHeld: 1 });
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 1, timedOutHeld: 0 });
    rpc.close();
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 0, timedOutHeld: 0 });
  });

  it("queues thread.revision behind eight SQL reads until a credit is released", async () => {
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask("tasks.get");
    const revision = ask("thread.revision");
    await flush();
    expect(wire.sent).toHaveLength(8);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(9);
    expect(wire.sent[8].method).toBe("thread.revision");
    wire.deliver({ id: wire.sent[8].id, ok: true, result: { revision: 1 } });
    await expect(revision).resolves.toEqual({ revision: 1 });
  });

  it("queues writes.subscribe behind eight task reads until a SQL credit is released", async () => {
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask("tasks.get");
    const subscription = ask("writes.subscribe");
    await flush();
    expect(wire.sent).toHaveLength(8);
    expect(wire.sent.every(({ method }) => method === "tasks.get")).toBe(true);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(9);
    expect(wire.sent[8].method).toBe("writes.subscribe");
    wire.deliver({ id: wire.sent[8].id, ok: true, result: { operations: [] } });
    await expect(subscription).resolves.toEqual({ operations: [] });
  });

  it.each(["interactive", "background"])("shares task-read credits with filter choices at %s priority", async (priority) => {
    const { rpc, wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask("tasks.get");
    await flush();
    const choices = ask("tasks.filterChoices", { priority });
    ask("ping");
    await flush();
    expect(wire.sent.map(({ method }) => method)).toEqual([...Array(8).fill("tasks.get"), "ping"]);
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 8, timedOutHeld: 0 });

    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(wire.sent.at(-1).method).toBe("tasks.filterChoices");
    const choiceId = wire.sent.at(-1).id;
    const task = ask("tasks.get");
    await flush();
    expect(wire.sent).toHaveLength(10);
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 8, timedOutHeld: 0 });

    wire.deliver({ id: choiceId, ok: true, result: { labels: [] } });
    await expect(choices).resolves.toEqual({ labels: [] });
    await flush();
    expect(wire.sent).toHaveLength(11);
    expect(wire.sent.at(-1).method).toBe("tasks.get");
    wire.deliver({ id: wire.sent.at(-1).id, ok: true, result: { id: "task-1" } });
    await expect(task).resolves.toEqual({ id: "task-1" });
  });

  it.each(["interactive", "background"])("shares counts and lookup credits with mixed SQL reads at %s priority", async (priority) => {
    const { rpc, wire, ask } = setup();
    const initial = ["tasks.counts", "tasks.lookup", "thread.page", "tasks.list", "tasks.filterChoices", "writes.status", "thread.revision", "tasks.get"];
    const active = initial.map((method) => ask(method, { priority }));
    await flush();
    const counts = ask("tasks.counts", { priority });
    const lookup = ask("tasks.lookup", { priority });
    ask("ping");
    await flush();
    expect(wire.sent.map(({ method }) => method)).toEqual([...initial, "ping"]);
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 2, admitted: 8, timedOutHeld: 0 });

    wire.deliver({ id: wire.sent[0].id, ok: false, error: "marker unsupported", error_code: "unavailable", retryable: false });
    await expect(active[0]).rejects.toMatchObject({ error_code: "unavailable", retryable: false });
    await flush();
    expect(wire.sent.at(-1).method).toBe("tasks.counts");
    const countsId = wire.sent.at(-1).id;
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 1, admitted: 8, timedOutHeld: 0 });
    wire.deliver({ id: wire.sent[1].id, ok: true, result: { task: null } });
    await expect(active[1]).resolves.toEqual({ task: null });
    await flush();
    expect(wire.sent.at(-1).method).toBe("tasks.lookup");
    const lookupId = wire.sent.at(-1).id;
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 8, timedOutHeld: 0 });

    const next = ask("thread.activity");
    await flush();
    expect(wire.sent.at(-1).id).toBe(lookupId);
    wire.deliver({ id: countsId, ok: true, result: { unfinished_open_count: 3 } });
    await expect(counts).resolves.toEqual({ unfinished_open_count: 3 });
    await flush();
    expect(wire.sent.at(-1).method).toBe("thread.activity");
    wire.deliver({ id: lookupId, ok: true, result: { task: { number: 42 } } });
    await expect(lookup).resolves.toEqual({ task: { number: 42 } });
    wire.deliver({ id: wire.sent.at(-1).id, ok: true, result: {} });
    await expect(next).resolves.toEqual({});
    expect(rpc.sqlReadCreditSnapshot()).toEqual({ queued: 0, admitted: 6, timedOutHeld: 0 });
  });

  it("paces bridge SQL routing and the 3.24 durable reconciliation reads", () => {
    const source = readFileSync(new URL("../../bridge/src/api/effects.rs", import.meta.url), "utf8");
    const constant = source.match(/pub const SQL_READ_METHODS:[\s\S]*?=\s*&\[([\s\S]*?)\];/)[1];
    const server = [...constant.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
    expect([...SQL_READ_METHODS].sort()).toEqual([...new Set([...server, ...WRITE_READS])].sort());
  });

  it("paces every SQL method across priorities and carriers while control and other reads pass", async () => {
    const { rpc, wire, ask } = setup();
    const other = carrier();
    for (let n = 0; n < 4; n += 1) ask(METHODS[n], { priority: "background" });
    await flush();
    rpc.rideOn(other);
    for (let n = 4; n < 12; n += 1) ask(PACED_METHODS[n % PACED_METHODS.length]);
    await flush();
    expect(wire.sent).toHaveLength(4);
    expect(other.sent).toHaveLength(4);
    expect([...wire.sent, ...other.sent].map((entry) => entry.method)).toEqual(PACED_METHODS.slice(0, 8));

    ask("ping");
    ask("board.list");
    ask("thread.post");
    await flush();
    expect(other.sent.map((entry) => entry.method)).toEqual([...PACED_METHODS.slice(4, 8), "ping", "board.list", "thread.post"]);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(other.sent).toHaveLength(8);
  });

  it("receipts and duplicate or wrong-carrier replies cannot release credits", async () => {
    const { rpc, wire, ask } = setup();
    for (let n = 0; n < 10; n += 1) ask();
    await flush();
    const other = carrier();
    rpc.readFrom(other);
    const id = wire.sent[0].id;
    wire.deliver({ id, accepted: true });
    other.deliver({ id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(8);
    wire.deliver({ id, ok: false, error: "missing", error_code: "not_found" });
    await flush();
    expect(wire.sent).toHaveLength(9);
    wire.deliver({ id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(9);
  });

  it("starts deadlines only when credit is admitted and retains sent credit after timeout", async () => {
    vi.useFakeTimers();
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask();
    const observed = vi.fn();
    const queued = ask("tasks.get", { timeoutMs: 10 }).catch(observed);
    await vi.advanceTimersByTimeAsync(1000);
    expect(observed).not.toHaveBeenCalled();
    expect(wire.sent).toHaveLength(8);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await vi.advanceTimersByTimeAsync(11);
    await queued;
    expect(observed).toHaveBeenCalledWith(expect.objectContaining({ timedOut: true }));
    ask();
    await flush();
    expect(wire.sent).toHaveLength(9);
    wire.deliver({ id: wire.sent[8].id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(10);
  });

  it.each([undefined, null, 0])("bounds a ninth background read behind eight stuck sent reads (timeoutMs=%s)", async (timeoutMs) => {
    vi.useFakeTimers();
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask();
    await flush();
    for (const { id } of wire.sent) wire.deliver({ id, accepted: true });
    await vi.advanceTimersByTimeAsync(ANSWER_TIMEOUT_MS);
    const observed = vi.fn();
    const queued = ask("tasks.list", { priority: "background", timeoutMs }).catch(observed);
    await vi.advanceTimersByTimeAsync(12000);
    expect(observed).toHaveBeenCalledOnce();
    await queued;
    const error = normalizeError(observed.mock.calls[0][0]);
    expect(error).toMatchObject({ timedOut: true, uncertain: false, retryable: true, deadline: "admission" });
    expect(isTransientTransportError(error)).toBe(true);
    expect(wire.sent).toHaveLength(8);
    const next = ask("thread.revision");
    ask("thread.post");
    await flush();
    expect(wire.sent.at(-1).method).toBe("thread.post");
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(wire.sent.map(({ method }) => method)).not.toContain("tasks.list");
    expect(wire.sent.at(-1).method).toBe("thread.revision");
    wire.deliver({ id: wire.sent.at(-1).id, ok: true, result: { revision: 1 } });
    await expect(next).resolves.toEqual({ revision: 1 });
  });

  it("cancels admission atomically before a credit released in the expiry turn can dispatch it", async () => {
    vi.useFakeTimers();
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask();
    const failed = ask("tasks.list", { priority: "background" }).catch((error) => error);
    await flush();
    vi.advanceTimersByTime(12000);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(await failed).toMatchObject({ deadline: "admission", uncertain: false });
    expect(wire.sent).toHaveLength(8);
  });

  it("replaces admission with a fresh path deadline when a credit arrives", async () => {
    vi.useFakeTimers();
    const { wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask();
    const observed = vi.fn();
    const queued = ask("tasks.list", { priority: "background", timeoutMs: 20000 }).catch(observed);
    await vi.advanceTimersByTimeAsync(SQL_ADMISSION_TIMEOUT_MS - 1);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await vi.advanceTimersByTimeAsync(1);
    expect(wire.sent).toHaveLength(9);
    expect(observed).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(19999);
    await queued;
    expect(observed).toHaveBeenCalledWith(expect.objectContaining({ deadline: "path", uncertain: true }));
  });

  it.each(["abort", "close", "reset"])("clears the admission clock on %s without sending the queued read", async (reason) => {
    vi.useFakeTimers();
    const { rpc, wire, ask } = setup();
    for (let n = 0; n < 8; n += 1) ask();
    const abort = new AbortController();
    const queued = ask("tasks.list", { priority: "background", signal: abort.signal }).catch((error) => error);
    await flush();
    expect(vi.getTimerCount()).toBe(1);
    if (reason === "abort") abort.abort();
    if (reason === "close") rpc.close(new Error("closed"));
    if (reason === "reset") rpc.resetSqlReads();
    expect(await queued).toBeInstanceOf(Error);
    expect(vi.getTimerCount()).toBe(0);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await vi.advanceTimersByTimeAsync(SQL_ADMISSION_TIMEOUT_MS);
    expect(wire.sent).toHaveLength(8);
  });

  it("cancels unsent reads without sending later, and retains sent credits until a final reply", async () => {
    const { wire, ask } = setup();
    const sentAbort = new AbortController();
    const sent = ask("tasks.get", { signal: sentAbort.signal });
    for (let n = 1; n < 8; n += 1) ask();
    const queuedAbort = new AbortController();
    const queued = ask("tasks.list", { signal: queuedAbort.signal });
    ask("thread.operation");
    await flush();
    sentAbort.abort();
    queuedAbort.abort();
    await expect(sent).rejects.toMatchObject({ name: "AbortError" });
    await expect(queued).rejects.toMatchObject({ name: "AbortError" });
    expect(wire.sent).toHaveLength(8);
    wire.deliver({ id: wire.sent[0].id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(9);
    expect(wire.sent[8].method).toBe("thread.operation");
  });

  it("drops cancelled reservations whose encryption has not finished", async () => {
    const encryptions = [];
    const { wire, ask } = setup({ transport: { ...transport, encryptFrame: (frame) => new Promise((resolve) => encryptions.push(() => resolve(frame.frameFields))) } });
    const abort = new AbortController();
    const cancelled = ask("tasks.get", { signal: abort.signal });
    for (let n = 1; n < 9; n += 1) ask();
    await flush();
    expect(encryptions).toHaveLength(8);
    abort.abort();
    await expect(cancelled).rejects.toMatchObject({ name: "AbortError" });
    await flush();
    expect(encryptions).toHaveLength(9);
    for (const finish of encryptions) finish();
    await flush();
    expect(wire.sent).toHaveLength(8);
    expect(wire.sent.some((entry) => entry.method === "tasks.get")).toBe(false);
  });

  it("frees only a closed carrier's reads, keeping replacement calls and identity usable", async () => {
    const { rpc, wire, ask } = setup();
    const old = Array.from({ length: 10 }, () => ask());
    await flush();
    const replacement = carrier();
    rpc.rideOn(replacement);
    const fresh = ask("tasks.get");
    await flush();
    expect(replacement.sent).toHaveLength(0);
    wire.close();
    await flush();
    for (const call of old) await expect(call).rejects.toMatchObject({ uncertain: true, retryable: true });
    expect(wire.sent).toHaveLength(8);
    expect(replacement.sent).toHaveLength(1);
    replacement.deliver({ id: replacement.sent[0].id, ok: true, result: { fresh: true } });
    await expect(fresh).resolves.toEqual({ fresh: true });
    expect(rpc.sessionId).toBe("opening");
  });

  it("resets outstanding and unsent SQL before new pulls while retaining non-SQL calls", async () => {
    const { rpc, wire, ask } = setup();
    const old = Array.from({ length: 10 }, () => ask());
    const ping = ask("ping");
    await flush();
    const oldIds = wire.sent.filter(({ method }) => method !== "ping").map(({ id }) => id);
    rpc.resetSqlReads();
    for (const call of old) {
      const error = await call.catch((failure) => failure);
      expect(normalizeError(error)).toMatchObject({ uncertain: true, retryable: true });
      expect(isTransientTransportError(normalizeError(error))).toBe(true);
    }
    const fresh = Array.from({ length: 9 }, () => ask());
    await flush();
    expect(wire.sent).toHaveLength(17);
    for (const id of oldIds) wire.deliver({ id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(17);
    wire.deliver({ id: wire.sent.find(({ method }) => method === "ping").id, ok: true, result: { pong: true } });
    await expect(ping).resolves.toEqual({ pong: true });
    wire.deliver({ id: wire.sent[9].id, ok: true, result: { fresh: true } });
    await expect(fresh[0]).resolves.toEqual({ fresh: true });
    await flush();
    expect(wire.sent).toHaveLength(18);
  });

  it("does not send queued or encrypting reads after session end, and a new opening has fresh credits", async () => {
    const encryptions = [];
    const { rpc, wire, ask } = setup({ transport: { ...transport, encryptFrame: (frame) => new Promise((resolve) => encryptions.push(() => resolve(frame.frameFields))) } });
    const waiting = Array.from({ length: 10 }, () => ask());
    await flush();
    rpc.close(new Error("session closed"));
    for (const finish of encryptions) finish();
    await flush();
    for (const call of waiting) await expect(call).rejects.toThrow("session closed");
    expect(wire.sent).toEqual([]);
    const reopened = setup();
    for (let n = 0; n < 9; n += 1) reopened.ask();
    await flush();
    expect(reopened.wire.sent).toHaveLength(8);
  });

  it("releases an unsent credit after encryption fails before carrier handoff", async () => {
    const encryptFrame = vi.fn().mockRejectedValueOnce(new Error("encryption failed")).mockImplementation(transport.encryptFrame);
    const { wire, ask } = setup({ transport: { ...transport, encryptFrame } });
    const failed = ask();
    for (let n = 0; n < 8; n += 1) ask();
    await expect(failed).rejects.toMatchObject({ message: "encryption failed" });
    await expect(failed).rejects.not.toMatchObject({ uncertain: true });
    await flush();
    expect(wire.sent).toHaveLength(8);
  });

  it("does not reuse old completions through repeated restores on the same live carrier", async () => {
    const { rpc, wire, ask } = setup();
    const oldIds = [];
    for (let round = 0; round < 3; round += 1) {
      for (let n = 0; n < 8; n += 1) ask();
      await flush();
      oldIds.push(...wire.sent.slice(-8).map(({ id }) => id));
      rpc.resetSqlReads();
    }
    for (let n = 0; n < 9; n += 1) ask();
    await flush();
    expect(wire.sent).toHaveLength(32);
    for (const id of oldIds) wire.deliver({ id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(32);
    wire.deliver({ id: wire.sent[24].id, ok: true, result: {} });
    await flush();
    expect(wire.sent).toHaveLength(33);
  });

  it("cancels a carrier-waiting reservation and never sends when that carrier arrives", async () => {
    const { wire, ask } = setup();
    let arrive;
    const abort = new AbortController();
    const waiting = ask("tasks.get", { carrier: new Promise((resolve) => { arrive = resolve; }), signal: abort.signal });
    for (let n = 0; n < 8; n += 1) ask();
    await flush();
    expect(wire.sent).toHaveLength(7);
    abort.abort();
    await expect(waiting).rejects.toMatchObject({ name: "AbortError" });
    arrive(wire);
    await flush();
    expect(wire.sent).toHaveLength(8);
    expect(wire.sent.every(({ method }) => method === "thread.page")).toBe(true);
  });
});
