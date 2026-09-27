// The watch gate (#65): does this machine's bridge know what watching is?
//
// A capability, not a version compare. The v1 adapter's own header states the
// rule — "a surface never asks what version the bridge reports, it asks the
// adapter's capabilities" — and #64 states `watching` in the greeting. The
// minor is the adapter's fallback for a bridge that has the verbs but not the
// flag, which is a judgement that belongs there and not in this file.
//
// Refused by default for the same reason as #21's task-context gate: a verb a
// bridge has never heard of is a refusal, not a polite no, and the read mark
// would produce one per glance at a task.
//
// These cases mock the capability read to pin the CONTRACT this file depends
// on. That the contract is really wired — that a greeting reaches it and the
// flag is spelled the way the bridge spells it — is railWatchSwitch.test.js,
// which greets a real bridge and asks the real gate.

import { describe, it, expect, vi, beforeEach } from "vitest";

const capabilities = vi.fn();
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: (...args) => capabilities(...args) }));

const { carriesWatching } = await import("../src/core/trackerWatch.js");

beforeEach(() => capabilities.mockReset());

describe("when watching is offered", () => {
  it("is offered to a bridge whose capabilities say tasks.watching", () => {
    capabilities.mockReturnValue({ tasks: { watching: true } });
    expect(carriesWatching("dev-1")).toBe(true);
  });

  it("asks about the machine it was given", () => {
    capabilities.mockReturnValue({ tasks: { watching: true } });
    carriesWatching("dev-7");
    expect(capabilities).toHaveBeenCalledWith("dev-7");
  });
});

describe("what it does when it cannot tell", () => {
  // Every one of these is a real answer the capability read can give: a bridge
  // that stated the flag false, one that predates it, one no adapter claims.
  it("says no to a flag that is absent, false, or not a boolean at all", () => {
    const shapes = [
      {},
      { tasks: {} },
      { tasks: { watching: false } },
      { tasks: { watching: "yes" } },
      { tasks: { watching: 1 } },
      { tasks: { watching: null } },
    ];
    for (const said of shapes) {
      capabilities.mockReturnValue(said);
      expect([said, carriesWatching("dev-1")]).toEqual([said, false]);
    }
  });

  // `bridgeCapabilities` answers every unknown with a capabilities object of
  // its own, so there is no device this can be asked about that throws — and an
  // adapter older than the flag has no `tasks` group at all.
  it("says no for no device at all", () => {
    capabilities.mockReturnValue({ tasks: { watching: false } });
    expect(carriesWatching(null)).toBe(false);
    expect(carriesWatching(undefined)).toBe(false);
  });
});
