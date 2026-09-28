/** @vitest-environment jsdom */
// A notification clicked while the app is open (#200): the service worker
// focuses the window and posts `build.push.open`, and the app routes to the
// link itself, without a reload. Only this origin's own active worker may steer
// the app, and only to a link inside it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

/** jsdom has no ServiceWorker; the listener checks the class, so the fake one
 *  is installed as the global. */
class FakeServiceWorker {
  constructor(scriptURL) {
    this.scriptURL = scriptURL;
  }
}
globalThis.ServiceWorker = FakeServiceWorker;

const { installPushOpenListener } = await import("../src/push.js");

const OWN = new URL("/app/sw.js", location.origin).href;
const LINK = "/app/#/device/dev-1/project/proj-1/workspace/ws-1/changes?agent=agent-1";

let active;
let container;
let open;
let listener;
let stop;

beforeEach(() => {
  active = new FakeServiceWorker(OWN);
  container = {
    controller: null,
    addEventListener: vi.fn((type, fn) => {
      if (type === "message") listener = fn;
    }),
    removeEventListener: vi.fn(),
    getRegistration: vi.fn(async (scope) => (scope === "/app/" ? { active } : undefined)),
  };
  open = vi.fn();
  stop = installPushOpenListener({ open, container });
});

afterEach(() => stop());

/** Post a message as `source` and let the async check settle. */
async function post(data, source = active) {
  listener({ data, source });
  for (let i = 0; i < 5; i++) await Promise.resolve();
  await new Promise((done) => setTimeout(done, 0));
}

describe("the app's build.push.open listener", () => {
  it("routes to the link's hash when this origin's active worker asks", async () => {
    await post({ type: "build.push.open", url: LINK });
    expect(open).toHaveBeenCalledWith("#/device/dev-1/project/proj-1/workspace/ws-1/changes?agent=agent-1");
  });

  it("routes to a notification-marked link, the mark kept for the app to take", async () => {
    await post({ type: "build.push.open", url: `${LINK}&from=push` });
    expect(open).toHaveBeenCalledWith("#/device/dev-1/project/proj-1/workspace/ws-1/changes?agent=agent-1&from=push");
  });

  it("accepts the page's controller too", async () => {
    const controller = new FakeServiceWorker(OWN);
    container.controller = controller;
    await post({ type: "build.push.open", url: "/app/#/device/d/project/p?agent=a" }, controller);
    expect(open).toHaveBeenCalledWith("#/device/d/project/p?agent=a");
  });

  it("refuses a message from any other source", async () => {
    const sources = [
      new FakeServiceWorker("https://evil.example/app/sw.js"),
      new FakeServiceWorker(new URL("/app/other-sw.js", location.origin).href),
      new FakeServiceWorker(OWN), // the right script, but not the active worker nor the controller
      { scriptURL: OWN }, // not a ServiceWorker at all: a window or a port
      window,
      null,
    ];
    for (const source of sources) await post({ type: "build.push.open", url: LINK }, source);
    expect(open).not.toHaveBeenCalled();
  });

  it("refuses a link outside the app, even from the active worker", async () => {
    const urls = [
      "https://evil.example/app/#/device/d",
      "//evil.example/app/#/device/d",
      "/app/settings",
      "/app/",
      "/elsewhere/#/device/d",
      "/elsewhere/#/device/d?from=push",
      "https://evil.example/app/#/device/d?agent=a&from=push",
      "javascript:alert(1)",
      42,
      undefined,
    ];
    for (const url of urls) await post({ type: "build.push.open", url });
    expect(open).not.toHaveBeenCalled();
  });

  it("ignores other messages", async () => {
    await post({ type: "build.push.other", url: LINK });
    await post("build.push.open");
    expect(open).not.toHaveBeenCalled();
  });

  it("sets location.hash by default, and stops listening when asked", async () => {
    stop();
    stop = installPushOpenListener({ container });
    await post({ type: "build.push.open", url: "/app/#/device/d/project/p?agent=a" });
    expect(location.hash).toBe("#/device/d/project/p?agent=a");
    stop();
    expect(container.removeEventListener).toHaveBeenCalledWith("message", listener);
  });
});
