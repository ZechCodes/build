// @vitest-environment jsdom
// The inbox's one-tap offer to turn on push notifications (#191). It paints
// from its cached record like every other view, goes away for good once the
// browser's permission is decided either way, and a dismissal survives reload.
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, prompt;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  sessionStorage.clear();
  document.body.innerHTML = "";
  cache = await import("../src/core/localCache.js");
  prompt = await import("../src/core/pushPrompt.js");
});

/** A browser whose permission the test moves, and whose prompt it answers. */
function browser({ supported = true, permission = "default", answer = "granted" } = {}) {
  const listeners = new Set();
  const state = { permission };
  return {
    state,
    supported: () => supported,
    permission: () => state.permission,
    enable: vi.fn(async () => {
      state.permission = answer;
    }),
    onPermissionChange: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    decide(next) {
      state.permission = next;
      for (const listener of listeners) listener();
    },
  };
}

async function mount(env) {
  const host = document.createElement("div");
  host.hidden = true;
  document.body.append(host);
  const mounted = prompt.mountPushPrompt(host, env);
  await mounted.ready;
  return { host, mounted };
}

const shown = (host) => !host.hidden && host.querySelector("[data-push-enable]") !== null;

describe("the push prompt", () => {
  it("offers itself while the browser has not been asked", async () => {
    const { host } = await mount(browser());
    expect(shown(host)).toBe(true);
    expect(host.textContent).toContain("Turn on");
  });

  it("stays away where push cannot work or the browser already decided, not even for a frame", async () => {
    for (const env of [browser({ supported: false }), browser({ permission: "granted" }), browser({ permission: "denied" })]) {
      const host = document.createElement("div");
      host.hidden = true;
      document.body.append(host);
      const everShown = [];
      const observer = new MutationObserver(() => everShown.push(shown(host)));
      observer.observe(host, { attributes: true, childList: true });
      const mounted = prompt.mountPushPrompt(host, env);
      await mounted.ready;
      await new Promise((resolve) => setTimeout(resolve, 20));
      observer.disconnect();
      expect(everShown.includes(true)).toBe(false);
      mounted.dispose();
    }
  });

  it("asks from the click and is gone for good once granted", async () => {
    const env = browser();
    const { host, mounted } = await mount(env);
    host.querySelector("[data-push-enable]").click();
    await vi.waitFor(() => expect(shown(host)).toBe(false));
    expect(env.enable).toHaveBeenCalledTimes(1);
    mounted.dispose();

    // A browser reset back to "default" does not bring it back.
    env.state.permission = "default";
    const again = await mount(env);
    expect(shown(again.host)).toBe(false);
  });

  it("is gone for good once denied, from the prompt or from anywhere else", async () => {
    const denied = browser({ answer: "denied" });
    const first = await mount(denied);
    first.host.querySelector("[data-push-enable]").click();
    await vi.waitFor(() => expect(shown(first.host)).toBe(false));

    const elsewhere = browser();
    // A second tab's cache is the same store, so start from a clean one.
    globalThis.indexedDB = new IDBFactory();
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    prompt = await import("../src/core/pushPrompt.js");
    const second = await mount(elsewhere);
    expect(shown(second.host)).toBe(true);
    elsewhere.decide("denied"); // turned off in Settings or the browser's own UI
    await vi.waitFor(() => expect(shown(second.host)).toBe(false));
    second.mounted.dispose();
    elsewhere.state.permission = "default";
    const third = await mount(elsewhere);
    expect(shown(third.host)).toBe(false);
  });

  it("stays when the browser's prompt was closed without an answer", async () => {
    const env = browser({ answer: "default" });
    const { host } = await mount(env);
    host.querySelector("[data-push-enable]").click();
    await vi.waitFor(() => expect(env.enable).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(shown(host)).toBe(true);
  });

  it("keeps a dismissal across a reload", async () => {
    const env = browser();
    const { host, mounted } = await mount(env);
    host.querySelector("[data-push-dismiss]").click();
    await vi.waitFor(() => expect(shown(host)).toBe(false));
    expect(env.enable).not.toHaveBeenCalled();
    mounted.dispose();

    const reloaded = await mount(env);
    expect(shown(reloaded.host)).toBe(false);
  });

  it("paints from the cached record: a saved dismissal is never shown, not even for a frame", async () => {
    await cache.writeCached(prompt.PUSH_PROMPT_ADDRESS, { dismissed: true });
    const host = document.createElement("div");
    host.hidden = true;
    document.body.append(host);
    const painted = [];
    const observer = new MutationObserver(() => painted.push(host.hidden));
    observer.observe(host, { attributes: true, childList: true });
    const mounted = prompt.mountPushPrompt(host, browser());
    await mounted.ready;
    await new Promise((resolve) => setTimeout(resolve, 20));
    observer.disconnect();
    expect(shown(host)).toBe(false);
    expect(painted.every((hidden) => hidden)).toBe(true);
  });
});
