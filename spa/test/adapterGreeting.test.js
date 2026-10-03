// @vitest-environment jsdom
// The greeting picks the adapter (wire spec step 2.5): every greeting selects
// one and installs it, so a reconnect or a device switch onto another bridge
// version re-selects, and a bridge nobody here speaks to installs nothing.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let bridgeAdapter, bridgeCapabilities, changeEventsArmed, greetBridge, resetChangeEvents, subscriptionsActive, watchChanges;

const NONE = {
  changes: { subscriptions: false, kinds: [] },
  fs: { projectSources: false },
  requests: { priority: false },
  errors: { codes: false },
  diffs: { perFile: false },
  bodies: { pages: false, mediaRawPages: false },
  reviews: { get: false, snapshot: false, diff: false, complete: false, act: false, comments: false },
  tasks: { attachments: false, watching: false, context: false, doneSinceLeft: false, commentUserNotifies: false, listPaged: false, bodyPrecondition: false },
  conversations: { settings: false },
  github: { repos: false },
  messages: { context: false },
  threads: { postOperations: false, attachmentChunks: false },
  branches: { finishDelete: false },
  push: { registerKey: false, revokeKey: false },
  agents: { createdBy: false },
  projects: { updateSource: false, syncBase: false },
  workspaces: { measureSizes: false },
};

/** What a bridge that takes subscriptions, priorities and coded refusals
 *  names in its greeting, and nothing past them. */
const SUBSCRIBING_CAPABILITIES = ["changes.subscriptions", "requests.priority", "errors.codes"];

const subscribingGreeting = () => ({
  api_version: "2.0.0",
  capabilities: SUBSCRIBING_CAPABILITIES,
  push_events: true,
  events: ["changes"],
  changes: { subscriptions: true, kinds: ["state", "thread", "git", "files"], batch_ms: { min: 1000, max: 600000 } },
});

beforeEach(async () => {
  vi.resetModules();
  vi.useFakeTimers();
  ({ bridgeAdapter, bridgeCapabilities, changeEventsArmed, greetBridge, resetChangeEvents, subscriptionsActive, watchChanges } =
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
    await greetBridge(async () => subscribingGreeting(), { install });
    expect(install).toHaveBeenCalledTimes(1);
    expect(install.mock.calls[0][0]).toMatchObject({ major: 3, version: "2.0.0" });
    expect(bridgeAdapter()).toBe(install.mock.results[0].value);
    expect(bridgeCapabilities()).toEqual({
      // The kinds ride through as the greeting states them, so a caller can ask
      // whether this bridge carries the one it is about to subscribe to.
      changes: { subscriptions: true, kinds: ["state", "thread", "git", "files"] },
      fs: { projectSources: false },
      requests: { priority: true },
      errors: { codes: true },
      diffs: { perFile: false },
      bodies: { pages: false, mediaRawPages: false },
  reviews: { get: false, snapshot: false, diff: false, complete: false, act: false, comments: false },
      // A greeting that names no task feature carries none of them.
      tasks: { attachments: false, watching: false, context: false, doneSinceLeft: false, commentUserNotifies: false, listPaged: false, bodyPrecondition: false },
      conversations: { settings: false },
      github: { repos: false },
      messages: { context: false },
      threads: { postOperations: false, attachmentChunks: false },
      branches: { finishDelete: false },
      push: { registerKey: false, revokeKey: false },
      agents: { createdBy: false },
      projects: { updateSource: false, syncBase: false },
      workspaces: { measureSizes: false },
    });
  });

  it("is created on the greeting call itself when nobody installs it elsewhere", async () => {
    // A greeting that reports no version is the one bridge shape left that
    // advertises nothing: the lowest adapter takes it with every flag off.
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeAdapter()).toMatchObject({ major: 3, version: "0.0.0" });
    expect(bridgeCapabilities()).toEqual(NONE);
    expect(changeEventsArmed()).toBe(true);
  });

  it("serves a bridge that refuses the greeting as pre-alpha on the lowest adapter", async () => {
    await greetBridge(async () => {
      throw new Error("unknown method: session.hello");
    });
    expect(bridgeAdapter()).toMatchObject({ major: 3, version: "0.0.0" });
    expect(bridgeCapabilities()).toEqual(NONE);
  });

  it("installs nothing for a bridge above every adapter, and neither arms nor refetches", async () => {
    const refresh = vi.fn();
    watchChanges({ refresh });
    const install = vi.fn(() => null);
    const armed = await greetBridge(async () => ({ api_version: "4.0.0", push_events: true }), { install });
    expect(install).toHaveBeenCalledWith({ unsupported: "app", version: "4.0.0" });
    expect(armed).toBe(false);
    expect(changeEventsArmed()).toBe(false);
    expect(bridgeAdapter()).toBe(null);
    expect(bridgeCapabilities()).toEqual(NONE);
    expect(refresh).not.toHaveBeenCalled();
  });

  it("re-selects on every greeting: a reconnect onto another bridge version replaces the adapter", async () => {
    await greetBridge(async () => subscribingGreeting());
    expect(bridgeCapabilities().changes.subscriptions).toBe(true);
    await greetBridge(async () => ({ api_version: "4.0.0" }));
    expect(bridgeAdapter()).toBe(null);
    await greetBridge(async () => ({ push_events: true }));
    expect(bridgeAdapter()).toMatchObject({ version: "0.0.0" });
    expect(bridgeCapabilities().changes.subscriptions).toBe(false);
    expect(changeEventsArmed()).toBe(true);
  });

  it("does not install for a greeting whose session stopped owning the application", async () => {
    const install = vi.fn();
    await greetBridge(async () => subscribingGreeting(), { install, isCurrent: () => false });
    expect(install).not.toHaveBeenCalled();
    expect(bridgeAdapter()).toBe(null);
  });

  it("rechecks greeting authority after negotiation, before installing its verdict", async () => {
    let current = true;
    const install = vi.fn();
    const isCurrent = () => {
      const answer = current;
      // A newer greeting starts after negotiation returns, before its caller
      // resumes to install the negotiated adapter.
      queueMicrotask(() => { current = false; });
      return answer;
    };
    await greetBridge(async () => ({ api_version: "2.0.0" }), { install, isCurrent });
    expect(install).not.toHaveBeenCalled();
    expect(bridgeAdapter()).toBe(null);
  });

  it("serves subscriptions off the adapter's capabilities, not the raw greeting", async () => {
    const call = vi.fn(async () => subscribingGreeting());
    await greetBridge(call);
    expect(call.mock.calls.map(([, params]) => params.changes)).toEqual(["subscriptions"]);
    expect(subscriptionsActive()).toBe(true);
    await greetBridge(async () => ({ ...subscribingGreeting(), capabilities: [] }));
    expect(subscriptionsActive()).toBe(false);
  });
});
