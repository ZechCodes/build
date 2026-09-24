// @vitest-environment jsdom
// The connection icon at the foot of the conversation rail: a ring that says
// how many machines are connected, and turns yellow while one of them is being
// reconnected to.
//
// It replaces the toasts that used to stack in the corner, so it says the same
// things they did — and one they did not: it shows from the first attempt,
// where they waited for one to have failed first.
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const recovery = vi.hoisted(() => ({ states: [], listener: null }));
const contexts = vi.hoisted(() => ({ live: [], listener: null, paths: new Map(), restoring: new Set() }));

vi.mock("../src/connection.js", () => ({
  deviceRecoverySnapshot: () => recovery.states,
  onDeviceRecoveryChanged: (listener) => {
    recovery.listener = listener;
    return () => { recovery.listener = null; };
  },
  forgetHomeFollow: () => {}, forgetRendezvousSockets: () => {}, forgetSecurityStops: () => {},
  connectDevice: async () => null, deviceWentAway: () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {}, syncHome: () => {},
  openDeviceSettingsSession: async () => ({}), retireDevice: () => {},
  securityStopText: () => "", chooseCreationDevice: () => {},
}));

vi.mock("../src/core/deviceContexts.js", () => ({
  liveContexts: () => contexts.live.map((deviceId) => ({
    deviceId,
    // The peer link is what measured the path this machine is carrying on.
    peerLink: contexts.paths.has(deviceId) || contexts.restoring.has(deviceId)
      ? { transportPath: () => contexts.paths.get(deviceId) || null, restoring: () => contexts.restoring.has(deviceId) }
      : null,
  })),
  onDeviceStateChanged: (listener) => {
    contexts.listener = listener;
    return () => { contexts.listener = null; };
  },
  resetDeviceContexts: () => {},
}));

const { App } = await import("../src/app.js");
const { mountConnectionStatus, unmountConnectionStatus } = await import("../src/connectionStatus.js");

const host = () => document.getElementById("connection-status");
/** The ring itself: the button the state is drawn on and the menu hangs off. */
const icon = () => host().querySelector(".connection-status");
const menu = () => host().querySelector(".connection-menu");
const rows = () => [...menu().querySelectorAll(".mi")].map((row) => ({
  name: row.querySelector(".mt").textContent,
  status: row.querySelector(".md").textContent,
}));
const centre = () => host().querySelector(".connection-centre");
const announcement = () => document.getElementById("connection-announcement");

/** The supervisor says something new, and the icon hears it. */
const recoveryMoved = (states) => {
  recovery.states = states;
  recovery.listener();
};

beforeEach(() => {
  vi.useFakeTimers();
  // Fake timers cannot drive fake IndexedDB's transaction callbacks. This
  // icon suite exercises the cache-unavailable fallback; cache wiring has its
  // own real-timer IndexedDB test.
  globalThis.indexedDB = undefined;
  document.body.innerHTML = `<div id="connection-status" hidden></div>
    <div id="connection-announcement" aria-live="polite"></div>`;
  App.devices = [{ id: "a", name: "Studio" }, { id: "b", name: "Laptop" }];
  contexts.live = ["a", "b"];
  contexts.paths = new Map();
  contexts.restoring = new Set();
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
    expect(host().hidden).toBe(false);
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

    expect(host().hidden).toBe(true);
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

  // #123: before this, a machine stuck restarting its path against a bridge
  // that had restarted showed a green ring for as long as it stayed stuck.
  it("radiates while a connected machine's path is being restored in place", () => {
    mountConnectionStatus();
    contexts.restoring = new Set(["a"]);
    contexts.listener();

    expect(icon().dataset.state).toBe("attempting");
    expect(icon().getAttribute("aria-label")).toBe("Reconnecting to Studio");

    contexts.restoring = new Set();
    contexts.listener();
    expect(icon().dataset.state).toBe("connected");
    expect(centre().textContent).toBe("2");
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

// The ring answers "are my machines there" at a glance; the menu behind it
// answers "which ones, and what is happening to them". It is the only place
// that says a machine is simply off.
describe("the menu behind the ring", () => {
  beforeEach(() => vi.useRealTimers());
  const press = async (open = true) => {
    icon().click();
    await vi.waitFor(() => expect(menu().hidden).toBe(!open));
  };

  it("is shut until the ring is pressed, and says so", () => {
    mountConnectionStatus();

    expect(menu().hidden).toBe(true);
    expect(icon().getAttribute("aria-haspopup")).toBe("menu");
    expect(icon().getAttribute("aria-expanded")).toBe("false");
    expect(menu().getAttribute("role")).toBe("menu");
  });

  it("opens on a press and shuts on the next one", async () => {
    mountConnectionStatus();

    await press();
    expect(menu().hidden).toBe(false);
    expect(icon().getAttribute("aria-expanded")).toBe("true");
    expect(host().classList.contains("is-open")).toBe(true);

    await press(false);
    expect(menu().hidden).toBe(true);
    expect(icon().getAttribute("aria-expanded")).toBe("false");
  });

  it("lists every machine the account has, with what is true of each", async () => {
    contexts.live = ["a"];
    recovery.states = [{ deviceId: "b", status: "attempting", failedAttempts: 0, nextAttemptAt: null }];
    mountConnectionStatus();
    await press();

    expect(rows()).toEqual([
      { name: "Studio", status: "Connected" },
      { name: "Laptop", status: "Reconnecting" },
    ]);
  });

  // The one state the ring cannot show: a machine the account lists that holds
  // no session and has nothing being done about it.
  it("says Offline for a machine nobody is reconnecting to", async () => {
    contexts.live = ["a"];
    mountConnectionStatus();
    await press();

    expect(rows()).toEqual([
      { name: "Studio", status: "Connected" },
      { name: "Laptop", status: "Offline" },
    ]);
  });

  // Both are connected and neither is a fault, but a reader who is on a relay
  // path is on a different connection from one who is not, and this is the
  // only place that says which.
  it("says which way each connected machine is carrying", async () => {
    contexts.paths = new Map([["a", "direct"], ["b", "turn"]]);
    mountConnectionStatus();
    await press();

    expect(rows()).toEqual([
      { name: "Studio", status: "Connected WebRTC" },
      { name: "Laptop", status: "Connected TURN" },
    ]);
  });

  it("says the plain word for a machine whose path nothing has measured yet", async () => {
    contexts.paths = new Map([["a", "direct"]]);
    mountConnectionStatus();
    await press();

    expect(rows()[1]).toEqual({ name: "Laptop", status: "Connected" });
  });

  it("counts a waiting machine down on the same clock the ring uses", async () => {
    contexts.live = ["a"];
    mountConnectionStatus();
    await press();
    vi.useFakeTimers();
    recoveryMoved([{ deviceId: "b", status: "waiting", failedAttempts: 1, nextAttemptAt: Date.now() + 3000 }]);
    expect(rows()[1].status).toBe("Reconnecting in 3 s");

    vi.advanceTimersByTime(1000);
    expect(rows()[1].status).toBe("Reconnecting in 2 s");

    vi.advanceTimersByTime(2000);
    expect(rows()[1].status).toBe("Reconnecting");
    expect(menu().hidden, "the menu stays open while its rows move").toBe(false);
  });

  it("shuts on Escape and hands the focus back to the ring", async () => {
    mountConnectionStatus();
    await press();

    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    await vi.waitFor(() => expect(menu().hidden).toBe(true));
    expect(document.activeElement).toBe(icon());
  });

  it("shuts on a press outside it, and leaves the focus where that press put it", async () => {
    document.body.insertAdjacentHTML("beforeend", '<button id="elsewhere">elsewhere</button>');
    mountConnectionStatus();
    await press();

    const elsewhere = document.getElementById("elsewhere");
    elsewhere.dispatchEvent(new Event("pointerdown", { bubbles: true }));

    await vi.waitFor(() => expect(menu().hidden).toBe(true));
    expect(document.activeElement).not.toBe(icon());
  });

  it("stays open when the press lands inside it", async () => {
    mountConnectionStatus();
    await press();

    menu().querySelector(".mi").dispatchEvent(new Event("pointerdown", { bubbles: true }));

    expect(menu().hidden).toBe(false);
  });

  it("goes with the icon when the account has no machines left to list", async () => {
    mountConnectionStatus();
    await press();

    App.devices = [];
    contexts.live = [];
    contexts.listener();

    expect(host().hidden).toBe(true);
    await vi.waitFor(() => expect(menu().hidden).toBe(true));
    expect(icon().getAttribute("aria-expanded")).toBe("false");
  });
});

// Where the menu stands, read off the sheet: jsdom lays nothing out, so the
// rules themselves are the assertion. What they have to guarantee is that a
// menu opening from a corner can only run inwards from it.
describe("where the menu opens", () => {
  // jsdom gives import.meta.url an http origin, so the sheet is read from the
  // package root, as the other jsdom suites do.
  const sheet = readFileSync(resolve("src/styles.css"), "utf8");
  const ruleFor = (selector) => {
    const start = sheet.indexOf(`${selector} {`);
    if (start < 0) return "";
    const body = start + selector.length + 2;
    const end = sheet.indexOf("}", body);
    return end < 0 ? "" : sheet.slice(body, end).replace(/\s+/g, " ");
  };

  it("opens upward from the ring, anchored to the corner it stands in", () => {
    const menuRule = ruleFor(".connection-menu");
    expect(menuRule).toContain("position:absolute");
    expect(menuRule).toContain("bottom:calc(100% + 8px)");
    expect(menuRule).toContain("right:0");
  });

  it("is never wider than the viewport it has to fit in", () => {
    expect(ruleFor(".connection-menu")).toContain("max-width:min(260px, calc(100vw - 24px))");
  });

  it("stands above the composer rather than over it, and is pressable", () => {
    // The ring sits in the strip's column, which is beside the panel the
    // composer is pinned to — and it is the press, so nothing may swallow it.
    const iconRule = ruleFor("#connection-status");
    expect(iconRule).toContain("bottom:8px");
    expect(iconRule).not.toContain("pointer-events:none");
    expect(ruleFor(".connection-menu")).toContain("z-index:39");
  });

  it("stands at the strip's right-hand end on a phone, above whatever the console takes", () => {
    const narrow = sheet.slice(sheet.indexOf("@media (max-width: 760px)"));
    expect(narrow).toMatch(/#connection-status \{[^}]*right:8px/);
    expect(narrow).toMatch(/#connection-status \{[^}]*bottom:calc\(var\(--console-space, 0px\) \+ 8px\)/);
  });
});
