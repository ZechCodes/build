// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const recovery = vi.hoisted(() => ({ states: [], listener: null }));
vi.mock("../src/connection.js", () => ({
  deviceRecoverySnapshot: () => recovery.states,
  onDeviceRecoveryChanged: (listener) => {
    recovery.listener = listener;
    return () => { recovery.listener = null; };
  },
  forgetHomeFollow: () => {}, forgetRendezvousSockets: () => {}, forgetSecurityStops: () => {},
  connectDevice: async () => null, deviceWentAway: () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncDeviceRecoveryPresence: () => {}, syncHome: () => {},
  openDeviceSettingsSession: async () => ({}), retireDevice: () => {},
  securityStopText: () => "", chooseCreationDevice: () => {},
}));

const { App } = await import("../src/app.js");
const { mountRecoveryBanners, unmountRecoveryBanners } = await import("../src/recoveryBanner.js");

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = '<div id="recovery-banners"></div><div id="recovery-announcement" aria-live="polite"></div>';
  App.devices = [{ id: "a", name: "Studio" }, { id: "b", name: "Laptop" }];
  recovery.states = [];
});

afterEach(() => {
  unmountRecoveryBanners();
  vi.useRealTimers();
});

describe("recovery banners", () => {
  it("keeps the immediate first attempt silent, then names each failed device", () => {
    recovery.states = [{ deviceId: "a", status: "attempting", failedAttempts: 0, nextAttemptAt: 0 }];
    mountRecoveryBanners();
    expect(document.getElementById("recovery-banners").textContent).toBe("");

    recovery.states = [
      { deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000 },
      { deviceId: "b", status: "attempting", failedAttempts: 2, nextAttemptAt: 0 },
    ];
    recovery.listener();
    expect(document.getElementById("recovery-banners").textContent).toContain("Reconnecting to Studio in 2s");
    expect(document.getElementById("recovery-banners").textContent).toContain("Reconnecting to Laptop");
  });

  it("announces an episode once without repeating countdown updates", () => {
    recovery.states = [{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000 }];
    mountRecoveryBanners();
    const live = document.getElementById("recovery-announcement");
    expect(live.textContent).toBe("Reconnecting to Studio");
    live.textContent = "heard";
    vi.advanceTimersByTime(1000);
    expect(live.textContent).toBe("heard");
  });

  it("clears recovered, offline, or removed devices when the supervisor removes their state", () => {
    recovery.states = [{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000 }];
    mountRecoveryBanners();
    recovery.states = [];
    recovery.listener();
    expect(document.getElementById("recovery-banners").textContent).toBe("");
    expect(document.getElementById("recovery-announcement").textContent).toBe("");
  });

  it("announces simultaneous device failures together", () => {
    recovery.states = [
      { deviceId: "a", status: "attempting", failedAttempts: 1, nextAttemptAt: 0 },
      { deviceId: "b", status: "attempting", failedAttempts: 1, nextAttemptAt: 0 },
    ];
    mountRecoveryBanners();
    expect(document.getElementById("recovery-announcement").textContent).toBe("Reconnecting to Studio and Laptop");
  });
});
