// @vitest-environment jsdom
// The greeting picks the adapter (wire spec step 2.5): every greeting selects
// one and installs it, so a reconnect or a device switch onto another bridge
// version re-selects, and a bridge nobody here speaks to installs nothing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let bridgeAdapter, bridgeCapabilities, changeEventsArmed, greetBridge, resetChangeEvents, watchChanges;

const NONE = { changes: { subscriptions: false, kinds: [] }, requests: { priority: false }, errors: { codes: false } };

const greeting11 = () => ({
  api_version: "1.2.0",
  push_events: true,
  events: ["board.changed", "entity.changed", "changes"],
  changes: { subscriptions: true, kinds: ["state", "thread", "git", "files"], batch_ms: { min: 1000, max: 600000 } },
});

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  ({ bridgeAdapter, bridgeCapabilities, changeEventsArmed, greetBridge, resetChangeEvents, watchChanges } =
    await import("../src/core/changeEvents.js"));
});

afterEach(() => {
  resetChangeEvents();
  vi.useRealTimers();
});

describe("the adapter a greeting selects", () => {
  it("is nothing until a bridge has been greeted", () => {
    expect(bridgeAdapter()).toBe(null);
    expect(bridgeCapabilities()).toEqual(NONE);
  });

  it("is installed through `install`, which is handed the selection", async () => {
    const install = vi.fn((selection) => selection.create(async () => ({})));
    await greetBridge(async () => greeting11(), { install });
    expect(install).toHaveBeenCalledTimes(1);
    expect(install.mock.calls[0][0]).toMatchObject({ major: 1, version: "1.2.0" });
    expect(bridgeAdapter()).toBe(install.mock.results[0].value);
    expect(bridgeCapabilities()).toEqual({
      // The kinds ride through as the greeting states them, so a caller can ask
      // whether this bridge carries the one it is about to subscribe to.
      changes: { subscriptions: true, kinds: ["state", "thread", "git", "files"] },
      requests: { priority: true },
      errors: { codes: true },
    });
  });

  it("is created on the greeting call itself when nobody installs it elsewhere", async () => {
    // A greeting that reports no version is the one bridge shape left that
    // advertises nothing: the lowest adapter takes it with every flag off.
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeAdapter()).toMatchObject({ major: 1, version: "0.0.0" });
    expect(bridgeCapabilities()).toEqual(NONE);
    expect(changeEventsArmed()).toBe(true);
  });

  it("serves a bridge that refuses the greeting as pre-alpha on the v1 adapter", async () => {
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(bridgeAdapter()).toMatchObject({ major: 1, version: "0.0.0" });
    expect(bridgeCapabilities()).toEqual(NONE);
  });

  it("installs nothing for a bridge above every adapter, and neither arms nor refetches", async () => {
    const refresh = vi.fn();
    watchChanges({ refresh });
    const install = vi.fn(() => null);
    const armed = await greetBridge(async () => ({ api_version: "2.0.0", push_events: true }), { install });
    expect(install).toHaveBeenCalledWith({ unsupported: "app", version: "2.0.0" });
    expect(armed).toBe(false);
    expect(changeEventsArmed()).toBe(false);
    expect(bridgeAdapter()).toBe(null);
    expect(bridgeCapabilities()).toEqual(NONE);
    expect(refresh).not.toHaveBeenCalled();
  });

  it("re-selects on every greeting: a reconnect onto another bridge version replaces the adapter", async () => {
    await greetBridge(async () => greeting11());
    expect(bridgeCapabilities().changes.subscriptions).toBe(true);
    await greetBridge(async () => ({ api_version: "2.0.0" }));
    expect(bridgeAdapter()).toBe(null);
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeAdapter()).toMatchObject({ version: "0.0.0" });
    expect(bridgeCapabilities().changes.subscriptions).toBe(false);
    expect(changeEventsArmed()).toBe(true);
  });

  it("does not install for a greeting whose session stopped owning the application", async () => {
    const install = vi.fn();
    await greetBridge(async () => greeting11(), { install, isCurrent: () => false });
    expect(install).not.toHaveBeenCalled();
    expect(bridgeAdapter()).toBe(null);
  });

  it("asks for subscriptions off the adapter's capabilities, not the raw greeting", async () => {
    const call = vi.fn(async () => greeting11());
    await greetBridge(call);
    expect(call.mock.calls.map(([, params]) => params.changes)).toEqual([undefined, "subscriptions"]);
  });
});
