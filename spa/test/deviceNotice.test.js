// @vitest-environment jsdom
// The one sentence this client says about a machine it cannot reach, and the
// markup a work surface stands up in place of itself while that is true. Every
// surface that names a missing machine — a branch, an issue, a sheet the
// toolbar refuses to open — says it in these words.

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { App } from "../src/app.js";
import { deviceOfflineNotice, mountDeviceNotice, mountDeviceStrip } from "../src/core/deviceNotice.js";
import { deviceFrozenText } from "../src/core/text.js";
import { adoptDeviceSession, resetDeviceContexts, setContextOffline } from "../src/core/deviceContexts.js";
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
