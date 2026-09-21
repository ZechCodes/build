// @vitest-environment jsdom
// The watch switch on the panel head, and the gate under it (#65).
//
// Two holes this file fills, both found after the panel head was measured.
//
// The gate's own suite (trackerWatchGate.test.js) mocks the capability read,
// so it proves the CONTRACT and nothing about the wiring: a renamed export, a
// greeting that never reaches the store, or a flag spelled differently from the
// way the bridge spells it would all leave it green. So the first half here
// greets a real bridge and asks the real gate — which is the shape of the break
// issues-spa hit when its copy still exported `carriesWatch`.
//
// The second half is the head's own half of that gate. I reported "the panel
// head draws no switch on a bridge without watching" with nothing asserting it:
// the rail's DOM suite never greets a bridge, so its heads have always rendered
// switchless and would have gone on doing so if the gate were deleted.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// agentRail.js reaches the conversation cache on import.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeApiVersion, bridgeCapabilities, greetBridge, resetChangeEvents } =
  await import("../src/core/changeEvents.js");
const { carriesWatching } = await import("../src/core/trackerWatch.js");
const { panelHeadHtml } = await import("../src/core/agentRail.js");

/** One machine saying what it is, through the real greeting path. `over` is
 *  whatever else that greeting states — `{ issues: { watching: false } }` for a
 *  bridge new enough to carry it that says it does not. */
const greet = (apiVersion, { deviceId = "dev-1", ...over } = {}) =>
  greetBridge(async () => ({ push_events: true, api_version: apiVersion, ...over }), { deviceId });

const head = (given) => {
  document.body.innerHTML = panelHeadHtml("claude", "chat", given);
  return document.body.firstElementChild;
};
const switchIn = (node) => node.querySelector(".rail-watch");

beforeEach(() => resetChangeEvents());
afterEach(() => {
  resetChangeEvents();
  vi.restoreAllMocks();
});

describe("the gate, asked of a bridge that actually greeted", () => {
  it("offers watching to a machine that greeted 1.9.0", async () => {
    await greet("1.9.0");
    expect(bridgeApiVersion("dev-1")).toBe("1.9.0");
    expect(carriesWatching("dev-1")).toBe(true);
  });

  it("refuses the roll before it, which is the bridge live today", async () => {
    await greet("1.8.0");
    expect(carriesWatching("dev-1")).toBe(false);
  });

  // The flag is the point of moving off the version compare: a bridge states
  // what it can do, and is taken at its word in BOTH directions.
  it("takes a stated flag over the minor, either way", async () => {
    await greet("1.8.0", { deviceId: "dev-early", issues: { watching: true } });
    await greet("1.9.0", { deviceId: "dev-withdrawn", issues: { watching: false } });
    expect([carriesWatching("dev-early"), carriesWatching("dev-withdrawn")]).toEqual([true, false]);
  });

  // The whole reason the gate reads a capability rather than a number: the
  // client asks what the bridge can do, and the flag is where that is said.
  it("is the capability the adapter derived, not a second opinion", async () => {
    await greet("1.9.0");
    expect(bridgeCapabilities("dev-1").issues.watching).toBe(true);
    expect(carriesWatching("dev-1")).toBe(bridgeCapabilities("dev-1").issues.watching);
  });

  // Not a mocked null: this is the real floor a machine reads as before it has
  // ever been greeted, which is the state every machine starts in.
  it("refuses a machine that has never greeted at all", () => {
    expect(bridgeApiVersion("dev-1")).toBe("0.0.0");
    expect(bridgeCapabilities("dev-1").issues.watching).toBe(false);
    expect(carriesWatching("dev-1")).toBe(false);
  });

  // The gate is per machine. A phone paired to two bridges must not be offered
  // a switch on the older one because the newer one answered first.
  it("answers per machine, not once for the client", async () => {
    await greet("1.9.0", { deviceId: "dev-new" });
    await greet("1.8.0", { deviceId: "dev-old" });
    expect([carriesWatching("dev-new"), carriesWatching("dev-old")]).toEqual([true, false]);
  });
});

describe("the head's half of it", () => {
  // `watchStateFor` hands null through for a machine the gate refused, and this
  // is what null has to mean by the time it reaches the markup.
  it("draws no switch at all when there is no watch state", () => {
    expect(switchIn(head({ watch: null }))).toBe(null);
  });

  it("draws none for a caller that predates watching and says nothing", () => {
    expect(switchIn(head({}))).toBe(null);
  });

  it("draws one, saying what it was given, when there is", () => {
    const control = switchIn(head({ watch: { watching: true, watchers: 3, pending: false } }));
    expect(control).not.toBe(null);
    expect(control.getAttribute("aria-pressed")).toBe("true");
    expect(control.getAttribute("title")).toBe("Watching · 3");
    expect(control.disabled).toBe(false);
  });

  // The head is rewritten whole whenever what it says changes, and the rail
  // repaints on the feed's beat. A rewrite landing mid-verb must not come back
  // offering a press the switch is going to ignore.
  it("draws it unpressable when the rewrite lands mid-verb", () => {
    expect(switchIn(head({ watch: { watching: true, watchers: 3, pending: true } })).disabled).toBe(true);
  });

  // The switch stands between Done and the pin. Its place is what makes it
  // reachable by thumb on a phone rather than under the menu.
  it("stands after the remove button and before the pin", () => {
    const node = head({ removable: true, watch: { watching: false, watchers: 0, pending: false } });
    const classes = [...node.children].map((child) => child.className);
    const at = (name) => classes.findIndex((value) => String(value).includes(name));
    expect(at("rail-remove")).toBeLessThan(at("rail-watch"));
    expect(at("rail-watch")).toBeLessThan(at("pinbtn"));
  });
});
