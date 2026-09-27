// @vitest-environment jsdom
// The one sentence this client says about a machine it cannot reach, and the
// markup a work surface stands up in place of itself while that is true. Every
// surface that names a missing machine — a branch, a task, a sheet the
// toolbar refuses to open — says it in these words.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The one thing a reader can do about a machine nothing could reach is ask
// again, and running the sequence is the connection layer's (rule 3).
const connection = vi.hoisted(() => ({ connectDevice: vi.fn(async () => null), recovery: new Map(), listeners: new Set() }));
vi.mock("../src/connection.js", () => ({
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {}, deviceRecoverySnapshot: (id) => connection.recovery.get(id) || null,
  onDeviceRecoveryChanged: (listener) => { connection.listeners.add(listener); return () => connection.listeners.delete(listener); },
  connectDevice: (...args) => connection.connectDevice(...args),
  chooseCreationDevice: () => {},
  deviceWentAway: () => {},
  goOffline: () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  openDeviceSettingsSession: async () => ({}),
  retireDevice: () => {},
  securityStopText: () => "",
  syncHome: () => {},
  forgetHomeFollow: () => {},
  forgetRendezvousSockets: () => {},
  forgetSecurityStops: () => {},
}));

import { App } from "../src/app.js";
import { blockedMark, blockedText, deviceAwayMark, deviceAwayWord } from "../src/core/deviceAway.js";
import { deviceOfflineNotice, mountDeviceNotice, mountDeviceStrip } from "../src/core/deviceNotice.js";
import { deviceBlockedMark, deviceBlockedWord, deviceFrozenText, deviceOfflineMark, deviceOfflineWord } from "../src/core/text.js";
import {
  adoptBridgeSelection,
  adoptDeviceSession,
  resetDeviceContexts,
  setContextOffline,
} from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

beforeEach(() => {
  connection.recovery.clear();
  connection.listeners.clear();
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

  it("does not insert a strip or move mounted content during online recovery", () => {
    mountDeviceStrip(host, context);
    const content = host.querySelector(".tab");
    App.devices[0].status = "online";
    connection.recovery.set("dev-1", { deviceId: "dev-1", status: "attempting", failedAttempts: 0 });

    setContextOffline("dev-1", blockedMark("lost"));

    expect(strips()).toHaveLength(0);
    expect(host.firstElementChild).toBe(content);
  });

  it("shows a fatal reason when recovery stops", () => {
    App.devices[0].status = "online";
    connection.recovery.set("dev-1", { deviceId: "dev-1", status: "attempting", failedAttempts: 0 });
    mountDeviceStrip(host, context);
    setContextOffline("dev-1", blockedMark("no-webrtc"));
    expect(strips()).toHaveLength(0);

    connection.recovery.delete("dev-1");
    connection.listeners.forEach((listener) => listener());

    expect(strips()).toHaveLength(1);
    expect(strips()[0].textContent).toContain("direct connection");
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

// A machine whose bridge is up and whose direct connection could not be made is
// not offline: the account lists it online and it answered the relay. It is
// blocked (spec rule 3), and every length the app has room for says so — the
// sentence a surface stands up, the mark a refused call carries, and the one
// word its greyed rows wear.
describe("a machine whose direct connection could not be made", () => {
  const blocked = (reason) => {
    const context = adoptDeviceSession(fakeSession("dev-1"));
    setContextOffline("dev-1", blockedMark(reason));
    return context;
  };

  it("keeps the reason on the machine's own context", () => {
    const context = blocked("ice-servers");

    expect(context.offline).toBe(true);
    expect(context.blocked).toBe("ice-servers");
    expect(context.offlineSince).toBeTypeOf("number");
  });

  it("says why in one sentence, in the account's name for the machine", () => {
    blocked("ice-servers");

    expect(deviceOfflineNotice("dev-1")).toBe(
      "workshop's direct connection could not be made: the connection servers could not be reached.",
    );
  });

  it("wears blocked as its word and its mark, whichever reason it was", () => {
    for (const reason of ["no-webrtc", "ice-servers", "refused", "timeout", "failed", "lost", "unreached"]) {
      const context = blocked(reason);
      expect(deviceAwayWord(context)).toBe(deviceBlockedWord);
      expect(deviceAwayMark(context)).toBe(deviceBlockedMark);
      // Every reason has words of its own; none of them falls through to a
      // blank or to the word "undefined".
      expect(blockedText(reason)).toMatch(/^This machine's direct connection could not be made: \S.+\.$/);
    }
  });

  it("says a plain away word again once the machine answers", () => {
    const context = blocked("timeout");

    setContextOffline("dev-1", { offline: false });

    expect(context.blocked).toBe(null);
    expect(deviceAwayWord(context)).toBe(deviceOfflineWord);
  });
});

// Rule 3 is a dead end with one way out: the reader asks again. Only a blocked
// machine offers it — an outage resolves itself the moment its bridge is back,
// and there is nothing to press for that.
describe("asking a blocked machine again", () => {
  let host;

  beforeEach(() => {
    connection.connectDevice.mockClear();
    document.body.innerHTML = '<main id="host"><div class="tab">the diff, as it was read</div></main>';
    host = document.querySelector("#host");
  });

  const stripOn = () => host.querySelector(":scope > .device-strip");

  it("says why over the surface, and offers the retry", () => {
    const context = adoptDeviceSession(fakeSession("dev-1"));
    mountDeviceStrip(host, context);

    setContextOffline("dev-1", blockedMark("ice-servers"));

    expect(stripOn().textContent).toContain(blockedText("ice-servers", "workshop"));
    const retry = stripOn().querySelector("[data-retry-device]");
    expect(retry.dataset.retryDevice).toBe("dev-1");

    retry.click();

    expect(connection.connectDevice).toHaveBeenCalledWith("dev-1", { reason: "row-retry" });
  });

  // The frozen sentence is about a machine that is away; a blocked one has a
  // reason, and the reason is what the reader needs to see.
  it("says the reason even over a surface with something on it", () => {
    const context = adoptDeviceSession(fakeSession("dev-1"));
    mountDeviceStrip(host, context, { hasContent: () => true });

    setContextOffline("dev-1", blockedMark("timeout"));

    expect(stripOn().textContent).toContain(blockedText("timeout", "workshop"));
    expect(stripOn().textContent).not.toContain(deviceFrozenText("workshop"));
  });

  it("offers nothing to press for a machine that is simply away", () => {
    const context = adoptDeviceSession(fakeSession("dev-1"));
    mountDeviceStrip(host, context);

    setContextOffline("dev-1");

    expect(stripOn().textContent).toBe(deviceFrozenText("workshop"));
    expect(stripOn().querySelector("[data-retry-device]")).toBe(null);
  });

  it("offers it in place of a surface a link named, too", () => {
    adoptDeviceSession(fakeSession("dev-1"));
    setContextOffline("dev-1", blockedMark("failed"));
    document.body.innerHTML = '<main id="root"></main>';
    const root = document.querySelector("#root");

    mountDeviceNotice(root, "dev-1");

    expect(root.textContent).toContain(blockedText("failed", "workshop"));
    root.querySelector("[data-retry-device]").click();

    expect(connection.connectDevice).toHaveBeenCalledWith("dev-1", { reason: "row-retry" });
    App.viewDispose?.();
    App.viewDispose = null;
  });
});
