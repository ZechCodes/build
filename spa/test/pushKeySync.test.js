/** @vitest-environment jsdom */
// #200: the notification key reaches the bridges that seal pushes, over E2EE
// only. Turning push on registers it everywhere that announces
// `push.registerKey`, every greeting registers it again (an idempotent upsert),
// a rotated subscription gets a new key, and turning push off revokes it and
// forgets it. Nothing here paints, and no refusal stands in push's way.
//
// Unmocked: the real greeting (changeEvents.js), the real device registry and
// the real key store, over one fake bridge per device.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { fakeSession } from "./deviceSessionFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const pushKeys = await import("../src/pushKeys.js");
const sync = await import("../src/core/pushKeySync.js");
const push = await import("../src/push.js");

const ENDPOINT = "https://fcm.googleapis.com/fcm/send/this-browser";
const PUSH_VERBS = ["push.registerKey", "push.revokeKey"];

/** A bridge on its own device, answering its greeting with `capabilities`. */
function bridge(deviceId, capabilities = PUSH_VERBS, answer = async () => ({})) {
  const session = fakeSession(deviceId);
  session.call = vi.fn(async (method, params) => (method === "session.hello"
    ? { api_version: "2.1.0", capabilities }
    : answer(method, params)));
  adoptDeviceSession(session);
  return {
    session,
    greet: () => greetBridge(session.call, { deviceId }),
    asked: (method) => session.call.mock.calls.filter(([name]) => name === method).map(([, params]) => params),
  };
}

let subscription;
let stop;

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  subscription = { endpoint: ENDPOINT };
  stop = sync.startPushKeySync({ subscription: async () => subscription });
  vi.spyOn(console, "warn").mockImplementation(() => {});
});

afterEach(() => {
  stop?.();
  resetChangeEvents();
  resetDeviceContexts();
  vi.restoreAllMocks();
});

describe("after a greeting", () => {
  it("registers this browser's key again with a bridge that announces the verb", async () => {
    const sid = await pushKeys.subscriptionIdOf(ENDPOINT);
    const { publicKey } = await pushKeys.ensurePushKey(sid);
    const one = bridge("dev-1");
    await one.greet();
    await vi.waitFor(() => expect(one.asked("push.registerKey")).toEqual([{ subscription_id: sid, public_key: publicKey }]));
    await one.greet();
    await vi.waitFor(() => expect(one.asked("push.registerKey")).toHaveLength(2));
    expect(one.asked("push.registerKey")[1]).toEqual({ subscription_id: sid, public_key: publicKey });
  });

  it("asks nothing of a bridge that does not announce it, and makes no key for it", async () => {
    const old = bridge("dev-1", []);
    await old.greet();
    await new Promise((done) => setTimeout(done, 20));
    expect(old.asked("push.registerKey")).toEqual([]);
    expect(await pushKeys.storedKeyIds()).toEqual([]);
  });

  it("registers nothing while this browser has no subscription", async () => {
    subscription = null;
    const one = bridge("dev-1");
    await one.greet();
    await new Promise((done) => setTimeout(done, 20));
    expect(one.asked("push.registerKey")).toEqual([]);
  });

  it("makes a new key for a rotated subscription, deletes the stale one, and gives it to every bridge", async () => {
    await pushKeys.ensurePushKey(await pushKeys.subscriptionIdOf("https://fcm.googleapis.com/fcm/send/old"));
    const sid = await pushKeys.subscriptionIdOf(ENDPOINT);
    const one = bridge("dev-1");
    const two = bridge("dev-2");
    const old = bridge("dev-3", []);
    await two.greet();
    await one.greet();
    await vi.waitFor(() => {
      expect(one.asked("push.registerKey")).not.toHaveLength(0);
      expect(two.asked("push.registerKey")).not.toHaveLength(0);
    });
    expect(await pushKeys.storedKeyIds()).toEqual([sid]);
    const { publicKey } = await pushKeys.ensurePushKey(sid);
    for (const registered of [...one.asked("push.registerKey"), ...two.asked("push.registerKey")]) {
      expect(registered).toEqual({ subscription_id: sid, public_key: publicKey });
    }
    expect(old.asked("push.registerKey")).toEqual([]);
  });
});

describe("turning push on and off", () => {
  let registration;

  beforeEach(() => {
    const browserSubscription = {
      endpoint: ENDPOINT,
      toJSON: () => ({ endpoint: ENDPOINT, keys: { p256dh: "p", auth: "a" } }),
      unsubscribe: vi.fn(async () => true),
    };
    registration = {
      pushManager: {
        subscribe: vi.fn(async () => {
          subscription = browserSubscription;
          return browserSubscription;
        }),
        getSubscription: vi.fn(async () => subscription),
      },
    };
    Object.defineProperty(navigator, "serviceWorker", {
      configurable: true,
      value: {
        register: vi.fn(async () => registration),
        ready: Promise.resolve(registration),
        getRegistration: vi.fn(async () => registration),
      },
    });
    globalThis.PushManager = function PushManager() {};
    globalThis.Notification = { permission: "default", requestPermission: vi.fn(async () => "granted") };
    vi.stubGlobal("fetch", vi.fn(async (url) => ({
      ok: true,
      text: async () => "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U",
      json: async () => ({ public_key: "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U" }),
      url,
    })));
    subscription = null;
  });

  afterEach(() => {
    delete navigator.serviceWorker;
    delete globalThis.PushManager;
    delete globalThis.Notification;
    vi.unstubAllGlobals();
  });

  it("registers a fresh non-extractable key with every bridge that announces the verb", async () => {
    const one = bridge("dev-1");
    const two = bridge("dev-2");
    const old = bridge("dev-3", []);
    await Promise.all([one.greet(), two.greet(), old.greet()]);
    await push.enablePush();
    const sid = await pushKeys.subscriptionIdOf(ENDPOINT);
    await vi.waitFor(() => {
      expect(one.asked("push.registerKey")).toHaveLength(1);
      expect(two.asked("push.registerKey")).toHaveLength(1);
    });
    const record = await pushKeys.readPushKey(sid);
    expect(record.privateKey.extractable).toBe(false);
    expect(one.asked("push.registerKey")[0]).toEqual({ subscription_id: sid, public_key: pushKeys.b64u(record.publicKey) });
    expect(old.asked("push.registerKey")).toEqual([]);
    // The api is handed the subscription and nothing of the key.
    for (const [, init] of fetch.mock.calls) {
      expect(String(init?.body || "")).not.toContain(pushKeys.b64u(record.publicKey));
    }
  });

  it("turns push on even when every bridge refuses the key", async () => {
    const refusing = bridge("dev-1", PUSH_VERBS, async (method) => {
      if (method === "push.registerKey") throw new Error("refused");
      return {};
    });
    await refusing.greet();
    await expect(push.enablePush()).resolves.toBeUndefined();
    await vi.waitFor(() => expect(console.warn).toHaveBeenCalled());
    expect(registration.pushManager.subscribe).toHaveBeenCalled();
  });

  it("revokes the key on every bridge that announces the verb, then forgets it", async () => {
    const one = bridge("dev-1");
    const old = bridge("dev-3", ["push.registerKey"]);
    await Promise.all([one.greet(), old.greet()]);
    await push.enablePush();
    const sid = await pushKeys.subscriptionIdOf(ENDPOINT);
    await vi.waitFor(async () => expect(await pushKeys.storedKeyIds()).toEqual([sid]));
    await push.disablePush();
    await vi.waitFor(async () => expect(await pushKeys.storedKeyIds()).toEqual([]));
    expect(one.asked("push.revokeKey")).toEqual([{ subscription_id: sid }]);
    expect(old.asked("push.revokeKey")).toEqual([]);
  });
});
