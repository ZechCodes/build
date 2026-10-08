import { beforeEach, describe, expect, it, vi } from "vitest";
import "fake-indexeddb/auto";
import { readFileSync } from "node:fs";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import * as reviewSupport from "../src/core/taskReviewSupport.js";
import * as adapter from "../src/core/bridgeApi/v1/index.js";

vi.mock("../src/core/deviceContexts.js", () => ({
  whenGreeted: (context, dispatch) => context.whenGreeted(dispatch),
}));

const fixture = (verb) => JSON.parse(readFileSync(new URL(`../../fixtures/api/v1/tasks.review.${verb}.json`, import.meta.url), "utf8"));
const prVerbs = ["open", "push", "update", "merge", "close", "reopen", "refresh"];
const legacyVerbs = ["get", "snapshot", "diff", "complete", "act"];
const allSupport = Object.fromEntries([...legacyVerbs, "comments", "pullRequests", ...prVerbs].map((verb) => [verb, true]));
const prReview = { mode: "pull_request", pull_request: { status: "open" } };
const context = (support = allSupport) => ({
  adapter: { capabilities: { reviews: support } },
  rpc: vi.fn(),
  whenGreeted: vi.fn(async (dispatch) => ({ sent: dispatch() })),
});

beforeEach(async () => { await wipeCache(); });

describe("cached review support", () => {
  it("restores a pre-PR cache record with false defaults for every PR operation", async () => {
    await writeCached(reviewSupport.reviewSupportAddress("device"), { get: true, act: true, snapshot: true });
    expect(await reviewSupport.readReviewSupport("device")).toEqual({
      ...reviewSupport.NO_REVIEW_SUPPORT, get: true, act: true, snapshot: true,
    });
  });

  it("stores only known Boolean capability facts and keeps stale greetings out", async () => {
    await reviewSupport.rememberReviewSupport("device", { reviews: {
      get: true, act: "yes", snapshot: 1, pullRequests: true, merge: true, future: true,
    } });
    const expected = { ...reviewSupport.NO_REVIEW_SUPPORT, get: true, pullRequests: true, merge: true };
    expect((await readCached(reviewSupport.reviewSupportAddress("device"))).value).toEqual(expected);
    expect(await reviewSupport.rememberReviewSupport("device", { reviews: {} }, () => false)).toBe(false);
    expect(await reviewSupport.readReviewSupport("device")).toEqual(expected);
  });

  it("revokes remembered PR support on a later legacy greeting", async () => {
    await reviewSupport.rememberReviewSupport("device", { reviews: allSupport });
    await reviewSupport.rememberReviewSupport("device", { reviews: { get: true, snapshot: true } });
    expect(await reviewSupport.readReviewSupport("device")).toEqual({
      ...reviewSupport.NO_REVIEW_SUPPORT, get: true, snapshot: true,
    });
  });
});

describe("review operation support", () => {
  it("exports only the fixture-backed exact review method names", () => {
    expect(adapter.REVIEW_METHODS).toEqual(Object.fromEntries([...legacyVerbs, ...prVerbs]
      .map((verb) => [verb, fixture(verb).method])));
    expect(reviewSupport.PR_REVIEW_VERBS).toEqual(prVerbs);
  });

  it.each(prVerbs)("requires the PR feature and the exact %s verb", (verb) => {
    for (const api_version of ["2.0.0", "3.15.0", "3.99.0"]) {
      const capabilities = (names) => adapter.capabilitiesOf({ api_version, capabilities: names }).reviews;
      expect(reviewSupport.canReviewOperation(capabilities([`tasks.review.${verb}`]), verb)).toBe(false);
      expect(reviewSupport.canReviewOperation(capabilities(["tasks.review.pullRequests"]), verb)).toBe(false);
      expect(reviewSupport.canReviewOperation(capabilities(["tasks.pullRequests", `tasks.review.${verb}`]), verb)).toBe(false);
      expect(reviewSupport.canReviewOperation(capabilities(["tasks.review.pullRequests", `tasks.review.${verb}`]), verb)).toBe(true);
      expect(reviewSupport.canReviewOperation(capabilities([]), verb)).toBe(false);
    }
  });

  it("gates legacy mutations by mode while retaining attached review behavior", () => {
    for (const verb of ["snapshot", "act", "complete"]) {
      expect(reviewSupport.canReviewOperation(allSupport, verb, prReview)).toBe(false);
      expect(reviewSupport.canReviewOperation(allSupport, verb, fixture("get").result.review)).toBe(true);
      expect(reviewSupport.canReviewOperation(allSupport, verb, { mode: "snapshot" })).toBe(true);
      expect(reviewSupport.canReviewOperation(allSupport, verb)).toBe(true);
    }
    expect(reviewSupport.reviewSupportFor(prReview, allSupport)).toEqual({
      ...allSupport, snapshot: false, act: false, complete: false,
    });
    expect(reviewSupport.reviewSupportFor(null, { open: true })).toEqual(reviewSupport.NO_REVIEW_SUPPORT);
    expect(allSupport.snapshot).toBe(true);
    expect(reviewSupport.canReviewOperation(allSupport, "get", prReview)).toBe(true);
    expect(reviewSupport.canReviewOperation(allSupport, "diff", prReview)).toBe(true);
  });

  it("claims no unknown, feature-only, malformed or unannounced operations", () => {
    for (const verb of ["unknown", "comments", "pullRequests", "tasks.review.open", "open.more", "__proto__", "constructor"]) {
      expect(reviewSupport.canReviewOperation({ ...allSupport, [verb]: true }, verb)).toBe(false);
    }
    expect(reviewSupport.canReviewOperation({ pullRequests: true, open: "true" }, "open")).toBe(false);
    expect(reviewSupport.canReviewOperation(null, "open")).toBe(false);
  });
});

describe("thin review RPC", () => {
  it.each(prVerbs)("passes the exact %s fixture request and whole reply through once", async (verb) => {
    const sample = fixture(verb);
    const machine = context();
    machine.rpc.mockResolvedValue(sample.result);
    const options = { priority: "foreground", timeoutMs: 5000 };
    expect(await reviewSupport.reviewRpc(machine)(sample.method, sample.params, options)).toBe(sample.result);
    expect(machine.rpc).toHaveBeenCalledExactlyOnceWith(sample.method, sample.params, options);
    expect(machine.rpc.mock.calls[0][1]).toBe(sample.params);
  });

  it("preserves call arity for existing review requests", async () => {
    const machine = context();
    const call = reviewSupport.reviewRpc(machine);
    await call("tasks.review.get");
    await call("tasks.review.diff", fixture("diff").params);
    expect(machine.rpc.mock.calls).toEqual([["tasks.review.get"], ["tasks.review.diff", fixture("diff").params]]);
  });

  it.each(prVerbs)("refuses unsupported %s without fallback or probing", async (verb) => {
    for (const support of [{ [verb]: true }, { pullRequests: true }]) {
      const machine = context(support);
      await expect(reviewSupport.reviewRpc(machine)(`tasks.review.${verb}`, fixture(verb).params))
        .rejects.toMatchObject({ code: "unknown_method" });
      expect(machine.rpc).not.toHaveBeenCalled();
    }
  });

  it("checks the current greeting at dispatch instead of trusting cached support", async () => {
    const machine = context(allSupport);
    const call = reviewSupport.reviewRpc(machine);
    machine.whenGreeted = async (dispatch) => {
      machine.adapter.capabilities.reviews = { ...allSupport, merge: false };
      return { sent: dispatch() };
    };
    await expect(call("tasks.review.merge", fixture("merge").params)).rejects.toMatchObject({ code: "unknown_method" });
    expect(machine.rpc).not.toHaveBeenCalled();
  });

  it("does not dispatch while a greeting refuses the device", async () => {
    const machine = context();
    machine.whenGreeted.mockResolvedValue(null);
    await expect(reviewSupport.reviewRpc(machine)("tasks.review.open", fixture("open").params))
      .rejects.toThrow("unavailable");
    expect(machine.rpc).not.toHaveBeenCalled();
  });

  it("keeps structured refusal details and never repeats a failed operation", async () => {
    const sample = fixture("open");
    const machine = context();
    const refusal = adapter.normalizeError(sample.refusals[0].reply);
    machine.rpc.mockRejectedValue(refusal);
    await expect(reviewSupport.reviewRpc(machine)(sample.method, sample.params)).rejects.toBe(refusal);
    expect(machine.rpc).toHaveBeenCalledExactlyOnceWith(sample.method, sample.params);
    expect(refusal.details).toEqual(sample.refusals[0].reply.details);
  });

  it("refuses names outside the closed review namespace", async () => {
    const machine = context({ ...allSupport, unknown: true });
    const call = reviewSupport.reviewRpc(machine);
    for (const method of ["tasks.review.unknown", "task.review.open", "tasks.review.open.more", "open", null]) {
      await expect(call(method, {})).rejects.toMatchObject({ code: "unknown_method" });
    }
    expect(machine.rpc).not.toHaveBeenCalled();
  });
});
