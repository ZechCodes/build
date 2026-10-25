import { describe, expect, it } from "vitest";
import { createSyncDiagnostics } from "../src/core/syncDiagnostics.js";

function fixture() {
  let time = 1000;
  return { diagnostics: createSyncDiagnostics({ now: () => time }), tick: (ms) => { time += ms; } };
}

describe("sync timing recorder", () => {
  it("records monotonic period, pass, stage, snapshot and request milestones", () => {
    const { diagnostics: d, tick } = fixture();
    d.ring(true);
    tick(2);
    const pass = d.passStart();
    const finish = d.stageStart(pass, "lists");
    const request = d.itemQueued("board.list", "rpc");
    tick(3);
    d.itemStart(request);
    tick(4);
    d.itemReceipt(request);
    d.snapshot(pass, "capture", { bodyRunning: 1, captured: 3 });
    d.selected(pass, { active: 1, recent: 2, projectOwner: 3, heldBodies: 4 });
    tick(5);
    d.itemSettled(request, "reply");
    finish();
    finish();
    d.snapshot(pass, "drain", { drained: 3 });
    d.passEnd(pass);
    d.ring(false);
    const period = d.report().periods[0];
    expect(period.start).toBe(0);
    expect(period.stop).toBe(14);
    expect(period.passes[0]).toMatchObject({ start: 2, end: 14, stages: [{ stage: "lists", start: 2, end: 14 }],
      snapshot: { capture: { at: 9, bodyRunning: 1, captured: 3 }, drain: { at: 14, drained: 3 } },
      selected: { active: 1, recent: 2, projectOwner: 3, heldBodies: 4 } });
    expect(period.slowest).toEqual([{ verb: "board.list", kind: "rpc", queued: 2, start: 5, receipt: 9, settlement: 14, outcome: "reply" }]);
    expect(d.report().oldestUnsettledMs).toBeNull();
  });

  it("reports unanswered requests after receipt and includes queued admission age", () => {
    const { diagnostics: d, tick } = fixture();
    d.ring(true);
    const one = d.itemQueued("fs.read", "file");
    tick(10);
    d.itemStart(one);
    d.itemReceipt(one);
    const two = d.itemQueued("git.log", "log");
    tick(20);
    expect(d.report().oldestUnsettledMs).toBe(30);
    expect(d.report().periods[0].slowest[0]).toMatchObject({ receipt: 10, settlement: null });
    d.itemSettled(one, "answer-timeout");
    expect(d.report().oldestUnsettledMs).toBe(20);
    d.itemSettled(two, "admission-cancel");
    expect(d.report().oldestUnsettledMs).toBeNull();
  });

  it("keeps only eight periods and the five slowest items without hiding active work", () => {
    const { diagnostics: d, tick } = fixture();
    for (let period = 0; period < 12; period += 1) {
      d.ring(true);
      d.ring(true);
      for (let item = 0; item < 12; item += 1) {
        const token = d.itemQueued("fs.read", "file");
        tick(item + 1);
        d.itemSettled(token, "reply");
      }
      d.ring(false);
    }
    const report = d.report();
    expect(report.periods).toHaveLength(8);
    for (const period of report.periods) {
      expect(period.slowest).toHaveLength(5);
      expect(period.slowest.map((item) => item.settlement - item.queued)).toEqual([12, 11, 10, 9, 8]);
    }
  });

  it("allowlists all serialized fields and never retains caller data in a report", () => {
    const { diagnostics: d, tick } = fixture();
    const secret = "private/path?token=secret device-uuid";
    d.ring(true);
    const pass = d.passStart();
    d.stageStart(pass, secret)();
    d.snapshot(pass, "capture", { bodyRunning: 2, captured: -1, drained: Infinity, secret });
    d.selected(pass, { active: 2, recent: NaN, heldBodies: 3.9, projectOwner: secret, secret });
    d.deviceState([{ syncing: true, pass: false, bodyPending: 5, deviceId: secret }]);
    const token = d.itemQueued(secret, secret);
    tick(3);
    d.itemSettled(token, secret);
    const report = d.report();
    expect(JSON.stringify(report)).not.toContain(secret);
    expect(report.periods[0].passes[0].stages).toEqual([]);
    expect(report.periods[0].passes[0].selected).toEqual({ active: 2, recent: 0, projectOwner: 0, heldBodies: 3 });
    expect(report.periods[0].slowest[0]).toMatchObject({ verb: "unknown", kind: "unknown", outcome: null });
    report.periods[0].passes[0].selected.active = 999;
    expect(d.report().periods[0].passes[0].selected.active).toBe(2);
  });

  it("aggregates live SQL credits without a ring and unregisters closed sessions", () => {
    const { diagnostics: d } = fixture();
    d.deviceState([{ syncing: false, pass: false }]);
    const unregister = d.registerSqlCredits(() => ({ queued: 2, admitted: 1, timedOutHeld: 1, deviceId: "private" }));
    d.registerSqlCredits(() => ({ queued: 1, admitted: 3, timedOutHeld: 0 }));
    expect(d.report()).toMatchObject({ periods: [], liveCount: 1, sql: { queued: 3, admitted: 4, timedOutHeld: 1 } });
    unregister();
    unregister();
    expect(d.report()).toMatchObject({ liveCount: 1, sql: { queued: 1, admitted: 3, timedOutHeld: 0 } });
  });

  it("clears histories, rejects stale tokens, and preserves live credit providers", () => {
    const { diagnostics: d, tick } = fixture();
    d.deviceState([{ syncing: true, pass: true }]);
    d.registerSqlCredits(() => ({ queued: 1 }));
    d.ring(true);
    const pass = d.passStart();
    const token = d.itemQueued("fs.read", "file");
    const finish = d.stageStart(pass, "held");
    d.clear();
    tick(3);
    d.itemSettled(token, "reply");
    d.passEnd(pass);
    finish();
    expect(d.report()).toMatchObject({ liveCount: 1, oldestUnsettledMs: null });
    expect(d.report().periods[0]).toMatchObject({ passes: [], slowest: [] });
    d.ring(false);
    d.clear();
    expect(d.report().periods).toEqual([]);
  });

  it("counts every pass while bounding pass and stage history", () => {
    const { diagnostics: d } = fixture();
    d.ring(true);
    for (let index = 0; index < 40; index += 1) {
      const pass = d.passStart();
      for (let stage = 0; stage < 80; stage += 1) d.stageStart(pass, "foreground")();
      d.selected(pass, { active: 2, recent: 3 });
      d.selected(pass, { heldBodies: 4 });
      d.passEnd(pass);
    }
    const period = d.report().periods[0];
    expect(period.passCount).toBe(40);
    expect(period.passes).toHaveLength(32);
    expect(period.passes[0].stages).toHaveLength(64);
    expect(period.passes[0].selected).toEqual({ active: 2, recent: 3, projectOwner: 0, heldBodies: 4 });
  });

  it("discards active request history from evicted periods and ignores late milestones", () => {
    const { diagnostics: d, tick } = fixture();
    d.ring(true);
    const old = d.itemQueued("session.hello", "rpc");
    tick(1);
    d.ring(false);
    for (let index = 0; index < 8; index += 1) {
      d.ring(true);
      d.ring(false);
    }
    expect(d.report().oldestUnsettledMs).toBeNull();
    d.itemReceipt(old);
    d.itemSettled(old, "reply");
    expect(d.report().oldestUnsettledMs).toBeNull();
    expect(d.report().periods.every((period) => period.slowest.length === 0)).toBe(true);
  });

  it("ignores backwards and nonfinite clocks without producing negative offsets", () => {
    let time = 100;
    const d = createSyncDiagnostics({ now: () => time });
    d.ring(true);
    const pass = d.passStart();
    const request = d.itemQueued("thread.activity", "rpc");
    time = -1;
    d.itemStart(request);
    time = NaN;
    d.passEnd(pass);
    time = 110;
    d.itemSettled(request, "path-timeout");
    d.ring(false);
    expect(d.report().periods[0]).toMatchObject({ stop: 10, passes: [{ start: 0, end: 0 }],
      slowest: [{ verb: "thread.activity", start: 0, settlement: 10 }] });
  });
});
