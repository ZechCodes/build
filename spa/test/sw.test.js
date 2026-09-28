// Drives the real service worker (public/sw.js) by loading it into a controlled
// `self` and invoking its event handlers, so the push copy / per-task tag /
// deep-link click behavior is actually exercised — not just the server payload.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const swSource = readFileSync(
  fileURLToPath(new URL("../public/sw.js", import.meta.url)),
  "utf8",
);

// Instantiate sw.js against a fake global, capturing its registered listeners.
function loadWorker() {
  const handlers = {};
  const showNotification = vi.fn(() => Promise.resolve());
  const openWindow = vi.fn(() => Promise.resolve({ opened: true }));
  const self = {
    addEventListener: (type, handler) => {
      handlers[type] = handler;
    },
    skipWaiting: vi.fn(),
    registration: { showNotification },
    clients: {
      claim: vi.fn(() => Promise.resolve()),
      matchAll: vi.fn(() => Promise.resolve([])),
      openWindow,
    },
  };
  // eslint-disable-next-line no-new-func
  new Function("self", swSource)(self);
  return { self, handlers, showNotification, openWindow };
}

/** Deliver a push and wait for the notification it shows: opening a sealed
 *  payload is async, so every push resolves through waitUntil. */
async function deliver(handlers, payload, options) {
  const event = pushEvent(payload, options);
  handlers.push(event);
  await Promise.all(event.waits);
}

function pushEvent(payload, { malformed = false } = {}) {
  const waits = [];
  return {
    waits,
    data: malformed
      ? {
          json() {
            throw new Error("not json");
          },
        }
      : payload === undefined
        ? null
        : { json: () => payload },
    waitUntil: (p) => waits.push(p),
  };
}

function clickEvent(url) {
  const waits = [];
  return {
    event: {
      notification: { close: vi.fn(), data: { url } },
      waitUntil: (p) => waits.push(p),
    },
    waits,
  };
}

function appWindow(overrides = {}) {
  return {
    url: "https://build.example/app/board",
    focus: vi.fn(() => Promise.resolve()),
    navigate: vi.fn(() => Promise.resolve()),
    ...overrides,
  };
}

describe("service worker push notification copy + tag", () => {
  beforeEach(() => {
    vi.spyOn(console, "debug").mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it("renders kind-specific body copy for each kind the unread counter has (#191)", async () => {
    // User-facing copy calls a task a task (#190).
    const cases = [
      ["agent", "An agent needs you", "/app/#/task/run-1"],
      ["task", "New activity on a task", "/app/#/tasks/task-1"],
    ];
    for (const [kind, body, url] of cases) {
      const { handlers, showNotification } = loadWorker();
      await deliver(handlers, { task_id: "t1", kind, url });
      expect(showNotification).toHaveBeenCalledWith(
        "Build",
        expect.objectContaining({
          body,
          icon: "/app/static/icon-192.png",
          tag: "build-task-t1",
          renotify: true,
          data: { url },
        }),
      );
    }
  });

  it("falls back to the agent copy for an unknown kind (a typo'd key never renders blank)", async () => {
    const { handlers, showNotification } = loadWorker();
    await deliver(handlers, { task_id: "t2", kind: "planready", url: "/app/" });
    expect(showNotification).toHaveBeenCalledWith(
      "Build",
      expect.objectContaining({ body: "An agent needs you" }),
    );
  });

  it("renders no deploy announcement: an app_update is not unread news (#191)", () => {
    expect(swSource).not.toContain("app_update");
  });

  it("uses a per-task tag, and a shared tag when there is no task id", async () => {
    const withId = loadWorker();
    await deliver(withId.handlers, { task_id: "abc", kind: "agent", url: "/app/" });
    expect(withId.showNotification.mock.calls[0][1].tag).toBe("build-task-abc");

    const noId = loadWorker();
    await deliver(noId.handlers, { kind: "agent", url: "/app/" });
    expect(noId.showNotification.mock.calls[0][1].tag).toBe("build-attention");
  });

  it("rejects a protocol-relative url and falls back to the app root", async () => {
    const { handlers, showNotification } = loadWorker();
    await deliver(handlers, { task_id: "t3", kind: "agent", url: "//evil.example/x" });
    expect(showNotification.mock.calls[0][1].data).toEqual({ url: "/app/" });
  });

  it("renders a generic notification for a non-JSON payload", async () => {
    const { handlers, showNotification } = loadWorker();
    await deliver(handlers, null, { malformed: true });
    expect(showNotification).toHaveBeenCalledWith(
      "Build",
      expect.objectContaining({ body: "An agent needs you", data: { url: "/app/" } }),
    );
  });
});

describe("service worker notificationclick deep-linking", () => {
  beforeEach(() => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  it("focuses an open app window and asks it to route to the deep link itself", async () => {
    const worker = loadWorker();
    const win = appWindow({ postMessage: vi.fn() });
    worker.self.clients.matchAll = vi.fn(() => Promise.resolve([win]));
    const { event, waits } = clickEvent("/app/#/device/d/project/p/workspace/w/changes?agent=a");
    worker.handlers.notificationclick(event);
    await Promise.all(waits);
    expect(event.notification.close).toHaveBeenCalled();
    expect(win.focus).toHaveBeenCalled();
    expect(win.postMessage).toHaveBeenCalledWith({
      type: "build.push.open",
      url: "/app/#/device/d/project/p/workspace/w/changes?agent=a&from=push",
    });
    expect(win.navigate).not.toHaveBeenCalled();
    expect(worker.openWindow).not.toHaveBeenCalled();
  });

  it("navigates the window when the message cannot be posted", async () => {
    const worker = loadWorker();
    const win = appWindow({ postMessage: vi.fn(() => { throw new Error("detached"); }) });
    worker.self.clients.matchAll = vi.fn(() => Promise.resolve([win]));
    const { event, waits } = clickEvent("/app/#/task/t1/diff");
    worker.handlers.notificationclick(event);
    await Promise.all(waits);
    expect(win.focus).toHaveBeenCalled();
    expect(win.navigate).toHaveBeenCalledWith("/app/#/task/t1/diff?from=push");
    expect(worker.openWindow).not.toHaveBeenCalled();
  });

  it("opens a new window on the deep link when navigate() rejects (no silent drop)", async () => {
    const worker = loadWorker();
    const win = appWindow({
      postMessage: vi.fn(() => { throw new Error("detached"); }),
      navigate: vi.fn(() => Promise.reject(new Error("uncontrolled"))),
    });
    worker.self.clients.matchAll = vi.fn(() => Promise.resolve([win]));
    const { event, waits } = clickEvent("/app/#/task/t9/plan");
    worker.handlers.notificationclick(event);
    await Promise.all(waits);
    expect(win.navigate).toHaveBeenCalled();
    expect(worker.openWindow).toHaveBeenCalledWith("/app/#/task/t9/plan?from=push");
    expect(console.warn).toHaveBeenCalled();
  });

  it("ignores windows outside the app", async () => {
    const worker = loadWorker();
    const other = appWindow({ url: "https://build.example/landing", postMessage: vi.fn() });
    worker.self.clients.matchAll = vi.fn(() => Promise.resolve([other]));
    const { event, waits } = clickEvent("/app/#/task/t4/plan");
    worker.handlers.notificationclick(event);
    await Promise.all(waits);
    expect(other.postMessage).not.toHaveBeenCalled();
    expect(worker.openWindow).toHaveBeenCalledWith("/app/#/task/t4/plan?from=push");
  });

  it("opens a new window on a cold start, where the router reads the hash at boot", async () => {
    const worker = loadWorker();
    worker.self.clients.matchAll = vi.fn(() => Promise.resolve([]));
    const { event, waits } = clickEvent("/app/#/task/t2/plan");
    worker.handlers.notificationclick(event);
    await Promise.all(waits);
    expect(worker.openWindow).toHaveBeenCalledWith("/app/#/task/t2/plan?from=push");
  });

  // #200 review: the mark tells the app this open came from a notification, so
  // it lands on the latest message; only a link inside the app is marked.
  it("marks only links inside the app as a notification open", async () => {
    for (const url of ["/app/", "/app/settings", "/elsewhere/#/device/d"]) {
      const worker = loadWorker();
      worker.self.clients.matchAll = vi.fn(() => Promise.resolve([]));
      const { event, waits } = clickEvent(url);
      worker.handlers.notificationclick(event);
      await Promise.all(waits);
      expect(worker.openWindow).toHaveBeenCalledWith(url);
    }
  });
});
