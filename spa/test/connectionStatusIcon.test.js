// @vitest-environment jsdom
// The connection icon at the foot of the conversation rail: a ring that says
// how many machines are connected, and turns yellow while one of them is being
// reconnected to.
//
// It replaces the toasts that used to stack in the corner, so it says the same
// things they did — and one they did not: it shows from the first attempt,
// where they waited for one to have failed first.
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";

const recovery = vi.hoisted(() => ({ states: [], listener: null }));
const contexts = vi.hoisted(() => ({ live: [], listener: null }));

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

vi.mock("../src/core/deviceContexts.js", () => ({
  liveContexts: () => contexts.live.map((deviceId) => ({ deviceId })),
  onDeviceStateChanged: (listener) => {
    contexts.listener = listener;
    return () => { contexts.listener = null; };
  },
  resetDeviceContexts: () => {},
}));

const { App } = await import("../src/app.js");
const { mountConnectionStatus, unmountConnectionStatus } = await import("../src/connectionStatus.js");

const icon = () => document.getElementById("connection-status");
const centre = () => icon().querySelector(".connection-centre");
const announcement = () => document.getElementById("connection-announcement");

/** The supervisor says something new, and the icon hears it. */
const recoveryMoved = (states) => {
  recovery.states = states;
  recovery.listener();
};

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = `<div id="connection-status" hidden></div>
    <div id="connection-announcement" aria-live="polite"></div>`;
  App.devices = [{ id: "a", name: "Studio" }, { id: "b", name: "Laptop" }];
  contexts.live = ["a", "b"];
  recovery.states = [];
});

afterEach(() => {
  unmountConnectionStatus();
  vi.useRealTimers();
});

describe("every machine connected", () => {
  it("is a green ring with the number of machines in it", () => {
    mountConnectionStatus();

    expect(icon().dataset.state).toBe("connected");
    expect(icon().classList.contains("is-connected")).toBe(true);
    expect(centre().textContent).toBe("2");
    expect(icon().hidden).toBe(false);
    expect(icon().getAttribute("aria-label")).toBe("Connected to 2 devices");
    expect(icon().getAttribute("title")).toBe("Connected to 2 devices");
  });

  it("counts only the machines holding a session", () => {
    contexts.live = ["a"];
    mountConnectionStatus();

    expect(centre().textContent).toBe("1");
    expect(icon().getAttribute("aria-label")).toBe("Connected to 1 device");
  });

  it("stays out of the way of an account with no machines at all", () => {
    App.devices = [];
    contexts.live = [];
    mountConnectionStatus();

    expect(icon().hidden).toBe(true);
  });
});

describe("a machine being reconnected to", () => {
  it("turns the ring yellow and radiates from the first attempt, before any has failed", () => {
    mountConnectionStatus();
    recoveryMoved([{ deviceId: "a", status: "attempting", failedAttempts: 0, nextAttemptAt: null }]);

    expect(icon().dataset.state).toBe("attempting");
    expect(icon().classList.contains("is-attempting")).toBe(true);
    expect(icon().classList.contains("is-connected")).toBe(false);
    expect(icon().querySelector(".connection-radiate")).not.toBeNull();
    expect(centre().textContent).toBe("");
    expect(icon().getAttribute("aria-label")).toBe("Reconnecting to Studio");
  });

  it("counts the seconds down to the next attempt, a second at a time", () => {
    mountConnectionStatus();
    recoveryMoved([{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 3000 }]);

    expect(icon().dataset.state).toBe("waiting");
    expect(icon().classList.contains("is-waiting")).toBe(true);
    expect(centre().textContent).toBe("3");
    expect(icon().getAttribute("aria-label")).toBe("Reconnecting to Studio in 3 seconds");

    vi.advanceTimersByTime(1000);
    expect(centre().textContent).toBe("2");
    vi.advanceTimersByTime(2000);
    expect(centre().textContent).toBe("0");
  });

  it("goes back to the count when the machine comes back", () => {
    mountConnectionStatus();
    recoveryMoved([{ deviceId: "a", status: "attempting", failedAttempts: 1, nextAttemptAt: null }]);
    recoveryMoved([]);

    expect(icon().dataset.state).toBe("connected");
    expect(centre().textContent).toBe("2");
  });

  it("stops counting once nothing is waiting", () => {
    mountConnectionStatus();
    recoveryMoved([{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 3000 }]);
    recoveryMoved([]);

    expect(vi.getTimerCount()).toBe(0);
  });

  it("hears the device list move as well as the supervisor", () => {
    mountConnectionStatus();
    contexts.live = ["a"];
    contexts.listener();

    expect(centre().textContent).toBe("1");
  });
});

describe("what the icon animates between", () => {
  it("keeps its parts across a state change, so the states transition rather than redraw", () => {
    mountConnectionStatus();
    const ring = centre();
    recoveryMoved([{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000 }]);

    // The same elements: a repaint that rebuilt them would restart every
    // transition from nothing and the ring would jump between states.
    expect(centre()).toBe(ring);
  });

  it("says which state it came from, so the change can be animated", () => {
    mountConnectionStatus();
    expect(icon().dataset.from).toBe("");

    recoveryMoved([{ deviceId: "a", status: "attempting", failedAttempts: 0, nextAttemptAt: null }]);
    expect(icon().dataset.from).toBe("connected");

    recoveryMoved([{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 2000 }]);
    expect(icon().dataset.from).toBe("attempting");
  });

  it("writes nothing on a tick that says nothing, so a transition is never cut short", () => {
    mountConnectionStatus();
    const written = icon().dataset.state;
    icon().dataset.state = "watch";
    contexts.listener();

    expect(icon().dataset.state).toBe("watch");
    expect(written).toBe("connected");
  });
});

describe("what a screen reader is told", () => {
  it("announces each state once, and not every second of a countdown", () => {
    mountConnectionStatus();
    recoveryMoved([{ deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 3000 }]);
    expect(announcement().textContent).toBe("Reconnecting to Studio in 3 seconds");

    announcement().textContent = "heard";
    vi.advanceTimersByTime(2000);
    expect(announcement().textContent).toBe("heard");

    recoveryMoved([]);
    expect(announcement().textContent).toBe("Connected to 2 devices");
  });
});
