// @vitest-environment jsdom
// The one sentence this client says about a machine it cannot reach, and the
// markup a work surface stands up in place of itself while that is true. Every
// surface that names a missing machine — a branch, an issue, a sheet the
// toolbar refuses to open — says it in these words.

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { App } from "../src/app.js";
import { deviceAwayMark, deviceAwayWord } from "../src/core/deviceAway.js";
import { deviceOfflineNotice, mountDeviceNotice, mountDeviceStrip } from "../src/core/deviceNotice.js";
import { deviceFrozenText, deviceOfflineMark, deviceOfflineWord } from "../src/core/text.js";
import {
  adoptBridgeSelection,
  adoptDeviceSession,
  resetDeviceContexts,
  setContextOffline,
} from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

beforeEach(() => {
  App.devices = [{ id: "dev-1", name: "workshop", status: "offline" }];
});

afterEach(() => {
  resetDeviceContexts();
});

describe("what a client says about a machine it cannot reach", () => {
  it("names the device the way the account does", () => {
    expect(deviceOfflineNotice("dev-1")).toBe("workshop isn't connected, so this can't be opened right now.");
  });

  it("falls back to plain words for a device the account has never listed", () => {
    expect(deviceOfflineNotice("dev-unknown")).toContain("That device isn't connected");
  });

  // A device name comes off the account and is painted as markup, so a hostile
  // one must not be able to write any.
  it("stands the same sentence up as a surface's empty state, with the name escaped", () => {
    App.devices = [{ id: "dev-1", name: "<script>", status: "offline" }];
    document.body.innerHTML = '<main id="root"></main>';
    const root = document.querySelector("#root");

    mountDeviceNotice(root, "dev-1");

    expect(root.querySelector(".empty")).toBeTruthy();
    expect(root.textContent).toContain("<script>");
    expect(root.innerHTML).toContain("&lt;script&gt;");
    expect(root.querySelector("script")).toBeNull();
    App.viewDispose?.();
    App.viewDispose = null;
  });
});

// A row has room for a word, not a sentence: grey plus "offline" is the whole
// of what a row says about a machine that is not here. A machine that IS here
// and answering in a shape this tab cannot read is not offline, so the word and
// the mark say which side needs the update instead.
describe("the word and the mark a row wears for a machine that cannot answer", () => {
  const behind = (side) => adoptBridgeSelection(adoptDeviceSession(fakeSession("dev-1")), { version: "9.0.0", unsupported: side }, null);

  it("says offline for a machine that is not connected", () => {
    const context = adoptDeviceSession(fakeSession("dev-1"));
    setContextOffline("dev-1");
    expect(deviceAwayWord(context)).toBe(deviceOfflineWord);
    expect(deviceAwayMark(context)).toBe(deviceOfflineMark);
  });

  it("asks for the update on the side that is behind", () => {
    expect(deviceAwayWord(behind("bridge"))).toBe("update");
    expect(deviceAwayMark(behind("bridge"))).toBe("Bridge is out of date");
    expect(deviceAwayWord(behind("app"))).toBe("reload");
    expect(deviceAwayMark(behind("app"))).toBe("App is out of date");
  });

  it("says offline for a machine this client holds nothing for at all", () => {
    expect(deviceAwayWord(null)).toBe(deviceOfflineWord);
    expect(deviceAwayMark(null)).toBe(deviceOfflineMark);
  });
});

// A surface that was already open when its machine went keeps what it read —
// there is nothing to hand back, and nothing to re-read — so what it needs is
// to say whose state it is showing. The strip says it, and comes down by itself
// the moment that machine answers again.
describe("naming the machine over a surface that is already open", () => {
  const strips = () => [...document.querySelectorAll("#host > .device-strip")];
  let host;
  let context;

  beforeEach(() => {
    document.body.innerHTML = '<main id="host"><div class="tab">the diff, as it was read</div></main>';
    host = document.querySelector("#host");
    context = adoptDeviceSession(fakeSession("dev-1"));
  });

  it("says nothing while that machine is answering", () => {
    mountDeviceStrip(host, context);

    expect(strips()).toHaveLength(0);
    expect(host.classList.contains("device-away")).toBe(false);
  });

  it("names the device when it goes, and takes the strip down when it answers again", () => {
    mountDeviceStrip(host, context);

    setContextOffline("dev-1");

    expect(strips().map((strip) => strip.textContent)).toEqual([deviceFrozenText("workshop")]);
    expect(host.classList.contains("device-away")).toBe(true);
    // What the reader was looking at is still on screen: the strip is over the
    // surface, not instead of it.
    expect(host.querySelector(".tab")).toBeTruthy();

    setContextOffline("dev-1", { offline: false });

    expect(strips()).toHaveLength(0);
    expect(host.classList.contains("device-away")).toBe(false);
  });

  // "This is what it last said" is a promise about what is on screen. A surface
  // whose machine went before its first read landed has nothing on it — the
  // reader is looking at "loading…" over a frame that never filled — so it says
  // the plain thing instead: this cannot be opened right now.
  it("says the plain sentence over a surface that never painted anything", () => {
    mountDeviceStrip(host, context, { hasContent: () => false });

    setContextOffline("dev-1");

    expect(strips().map((strip) => strip.textContent)).toEqual([deviceOfflineNotice("dev-1")]);
  });

  // And it turns over with the surface: the read that lands while the machine
  // is away gives the reader something the machine did say.
  it("says the frozen sentence once the surface has something on it", () => {
    let painted = false;
    mountDeviceStrip(host, context, { hasContent: () => painted });

    setContextOffline("dev-1");
    expect(strips().map((strip) => strip.textContent)).toEqual([deviceOfflineNotice("dev-1")]);

    painted = true;
    setContextOffline("dev-1", { offline: false });
    setContextOffline("dev-1");

    expect(strips().map((strip) => strip.textContent)).toEqual([deviceFrozenText("workshop")]);
  });

  it("carries one strip however often the account says the same thing", () => {
    mountDeviceStrip(host, context);

    setContextOffline("dev-1");
    setContextOffline("dev-1");

    expect(strips()).toHaveLength(1);
  });

  it("stops listening once the surface it was mounted over is gone", () => {
    const dispose = mountDeviceStrip(host, context);

    dispose();
    setContextOffline("dev-1");

    expect(strips()).toHaveLength(0);
    expect(host.classList.contains("device-away")).toBe(false);
  });
});
