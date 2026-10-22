import { describe, expect, it, vi } from "vitest";
import { createBridgeEpoch, endedOpeningError } from "../src/core/bridgeEpoch.js";

const greeting = (bridge_epoch = "boot-B") => ({
  bridge_epoch, journal_incarnation: "journal-A", capabilities: ["bridge.epoch"],
  ack_confirm_methods: ["tasks.comment"],
});

describe("authoritative bridge epochs", () => {
  it("rejects application frames before a greeting establishes authority", () => {
    const epoch = createBridgeEpoch();
    expect(epoch.accepts({ bridge_epoch: "unsolicited-boot" })).toBe(false);
    expect(epoch.accepts({})).toBe(false);
    expect(epoch.current()).toBe(null);
  });

  it("accepts only current greetings and never orders UUID epochs from uncorrelated frames", () => {
    const epoch = createBridgeEpoch();
    const changed = vi.fn();
    const regreet = vi.fn();
    epoch.onChange(changed);
    epoch.onUnexpected(regreet);
    expect(epoch.acceptGreeting(greeting(), { isCurrent: () => false })).toBe(false);
    expect(epoch.current()).toBe(null);
    expect(epoch.acceptGreeting(greeting())).toBe(true);
    expect(epoch.accepts({ bridge_epoch: "boot-A" })).toBe(false);
    expect(epoch.accepts({ bridge_epoch: "boot-C" })).toBe(false);
    expect(epoch.accepts({})).toBe(false);
    expect(epoch.current().bridge_epoch).toBe("boot-B");
    expect(regreet).toHaveBeenCalledTimes(3);
    expect(changed).toHaveBeenCalledTimes(1);
    expect(epoch.accepts({ bridge_epoch: "boot-B" })).toBe(true);
    epoch.acceptGreeting(greeting());
    expect(changed).toHaveBeenCalledTimes(1);
    epoch.acceptGreeting(greeting("boot-A"));
    expect(epoch.current()).toMatchObject({ bridge_epoch: "boot-A", opening: 2 });
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("rejects malformed authoritative epoch greetings", () => {
    const epoch = createBridgeEpoch();
    expect(() => epoch.acceptGreeting({ capabilities: ["bridge.epoch"] })).toThrow(/epoch/i);
    expect(epoch.current()).toBe(null);
  });

  it("marks ephemeral control reconciliation separately from unsafe stream replay", () => {
    expect(endedOpeningError("term.attach", false)).toMatchObject({ code: "scope_ended", retryable: true, uncertain: false });
    expect(endedOpeningError("term.input", true)).toMatchObject({ code: "uncertain_delivery", retryable: false, uncertain: true });
    expect(endedOpeningError("stream.start", true)).toMatchObject({ code: "uncertain_delivery", retryable: false });
  });
});
