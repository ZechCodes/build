// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { forgetRtcClientHints, rememberRtcClientSupport, rtcClientId } from "../src/core/rtcClientHint.js";

const greeting = (capabilities = ["rtc.clientLanCache"]) => ({ api_version: "3.12.0", capabilities });
const ids = ["773d4f16-a915-43cb-a067-c31bf8dfe1aa", "4c14a372-9db5-4faa-bbf1-d93583114e89", "22833293-45ee-4c96-89a3-ff8722be388f"];
let randomUUID;

beforeEach(() => {
  localStorage.clear();
  let next = 0;
  randomUUID = vi.fn(() => ids[next++]);
  vi.stubGlobal("crypto", { randomUUID });
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe("paired bridge LAN cache hint", () => {
  it("omits a hint until a compatible greeting advertises the capability", () => {
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    rememberRtcClientSupport("dev-a", "key-a", greeting([]));
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    rememberRtcClientSupport("dev-a", "key-a", { api_version: "4.0.0", capabilities: ["rtc.clientLanCache"] });
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    expect(randomUUID).not.toHaveBeenCalled();
  });

  it("persists a random UUID separately for each paired device and pinned key", () => {
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    expect(rtcClientId("dev-a", "key-a")).toBe(ids[0]);
    expect(rtcClientId("dev-a", "key-a")).toBe(ids[0]);
    rememberRtcClientSupport("dev-b", "key-a", greeting());
    expect(rtcClientId("dev-b", "key-a")).toBe(ids[1]);
    expect(rtcClientId("dev-a", "rotated-key")).toBeNull();
    rememberRtcClientSupport("dev-a", "rotated-key", greeting());
    expect(rtcClientId("dev-a", "rotated-key")).toBe(ids[2]);
    expect(randomUUID).toHaveBeenCalledTimes(3);
  });

  it("stops sending the hint when the latest greeting no longer supports it", () => {
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    expect(rtcClientId("dev-a", "key-a")).toBe(ids[0]);
    rememberRtcClientSupport("dev-a", "key-a", greeting([]));
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
  });

  it("has no shared fallback when storage is unavailable or cannot persist the UUID", () => {
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    vi.restoreAllMocks();
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("disabled"); });
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    expect(() => rememberRtcClientSupport("dev-a", "key-a", greeting())).not.toThrow();
  });

  it("requires the paired key and crypto.randomUUID", () => {
    rememberRtcClientSupport("dev-a", null, greeting());
    expect(rtcClientId("dev-a", null)).toBeNull();
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    vi.stubGlobal("crypto", {});
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
  });

  it("drops all hints for an unpaired device while preserving another pairing", () => {
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    rememberRtcClientSupport("dev-a", "old-key-a", greeting());
    rememberRtcClientSupport("dev-b", "key-b", greeting());
    expect(rtcClientId("dev-a", "key-a")).toBe(ids[0]);
    expect(rtcClientId("dev-b", "key-b")).toBe(ids[1]);
    forgetRtcClientHints("dev-a");
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    expect(rtcClientId("dev-a", "old-key-a")).toBeNull();
    expect(rtcClientId("dev-b", "key-b")).toBe(ids[1]);
  });

  it("clears the account's hints without deleting unrelated browser preferences", () => {
    rememberRtcClientSupport("dev-a", "key-a", greeting());
    localStorage.setItem("unrelated", "keep");
    forgetRtcClientHints();
    expect(rtcClientId("dev-a", "key-a")).toBeNull();
    expect(localStorage.getItem("unrelated")).toBe("keep");
  });
});
