import { describe, it, expect } from "vitest";
import { bucketBoard } from "../src/core/board.js";

const run = (over) => ({ run_id: "r", state: "building", needs_attention: false, ...over });
const plan = (over) => ({ plan_id: "p", state: "drafting", needs_attention: false, ...over });

describe("bucketBoard", () => {
  it("splits mixed plans and runs into attn / work / done", () => {
    const runs = [
      run({ run_id: "r-review", state: "review", needs_attention: true }),
      run({ run_id: "r-building", state: "building" }),
      run({ run_id: "r-merged", state: "merged" }),
    ];
    const plans = [
      plan({ plan_id: "p-review", state: "plan_review", needs_attention: true }),
      plan({ plan_id: "p-draft", state: "drafting" }),
      plan({ plan_id: "p-gone", state: "abandoned" }),
    ];
    const buckets = bucketBoard({ runs, plans });
    expect(buckets.attn.map((e) => e.kind === "run" ? e.r.run_id : e.p.plan_id)).toEqual(["r-review", "p-review"]);
    expect(buckets.work.map((e) => e.kind === "run" ? e.r.run_id : e.p.plan_id)).toEqual(["r-building", "p-draft"]);
    expect(buckets.done.map((e) => e.kind === "run" ? e.r.run_id : e.p.plan_id)).toEqual(["r-merged", "p-gone"]);
  });

  it("tags each entry with its kind so a card renderer can dispatch", () => {
    const buckets = bucketBoard({ runs: [run()], plans: [plan()] });
    expect(buckets.work[0]).toEqual({ kind: "run", r: expect.objectContaining({ run_id: "r" }) });
    expect(buckets.work[1]).toEqual({ kind: "plan", p: expect.objectContaining({ plan_id: "p" }) });
  });

  it("routes a terminal entity to done even when it still flags attention", () => {
    // needs_attention never survives a terminal state on the wire, but the
    // bucketer must not resurrect a merged/abandoned entity into NEEDS YOU.
    const buckets = bucketBoard({
      runs: [run({ run_id: "r-arch", state: "archived", needs_attention: true })],
      plans: [plan({ plan_id: "p-gone", state: "abandoned", needs_attention: true })],
    });
    expect(buckets.done.length).toBe(2);
    expect(buckets.attn).toEqual([]);
  });

  it("is safe for empty or absent lists", () => {
    expect(bucketBoard({})).toEqual({ attn: [], work: [], done: [] });
    expect(bucketBoard({ runs: [], plans: [] })).toEqual({ attn: [], work: [], done: [] });
  });
});
