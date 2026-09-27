// #200: the service worker opens a sealed push with this browser's own
// notification key and shows what it says, and shows the #191 generic copy for
// anything that does not open cleanly. Drives the real public/sw.js with Node's
// WebCrypto and an IndexedDB the app module (src/pushKeys.js) laid out, so the
// two copies of the database layout are held together here.
//
// The fixture is fixtures/push/sealed-v1.json, the vector the bridge's Rust
// sealer and the Python generator are held to as well.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { b64uDecode, b64uEncode, sealForTest } from "./pushSealFixture.js";

const read = (relative) => readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");
const swSource = read("../public/sw.js");
const fixture = JSON.parse(read("../../fixtures/push/sealed-v1.json"));
const fixtureMessage = JSON.parse(fixture.plaintext);
const IAT = fixtureMessage.iat;

let pushKeys;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  pushKeys = await import("../src/pushKeys.js");
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(IAT * 1000 + 5_000);
  vi.spyOn(console, "debug").mockImplementation(() => {});
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

/** The fixture's recipient key, imported the way the browser holds it: the
 *  private half non-extractable. */
async function fixtureKeyRecord(sid = fixture.subscription_id) {
  const privateKey = await crypto.subtle.importKey(
    "jwk", fixture.recipient_private_jwk, { name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"],
  );
  return { sid, privateKey, publicKey: b64uDecode(fixture.recipient_public_key) };
}

/** Put a key record into the database the app module laid out. */
async function storeKey(record) {
  await pushKeys.storedKeyIds(); // the app opens (and lays out) the database first
  const db = await new Promise((resolve, reject) => {
    const request = indexedDB.open("build-push");
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  await new Promise((resolve, reject) => {
    const tx = db.transaction("keys", "readwrite");
    tx.objectStore("keys").put(record);
    tx.oncomplete = resolve;
    tx.onerror = () => reject(tx.error);
  });
  db.close();
}

async function seenCount() {
  const db = await new Promise((resolve) => {
    const request = indexedDB.open("build-push");
    request.onsuccess = () => resolve(request.result);
  });
  const count = await new Promise((resolve) => {
    const request = db.transaction("seen", "readonly").objectStore("seen").count();
    request.onsuccess = () => resolve(request.result);
  });
  db.close();
  return count;
}

function loadWorker({ endpoint = fixture.endpoint } = {}) {
  const handlers = {};
  const showNotification = vi.fn(() => Promise.resolve());
  const self = {
    addEventListener: (type, handler) => {
      handlers[type] = handler;
    },
    skipWaiting: vi.fn(),
    crypto: globalThis.crypto,
    indexedDB: globalThis.indexedDB,
    registration: {
      showNotification,
      pushManager: { getSubscription: vi.fn(async () => (endpoint ? { endpoint } : null)) },
    },
    clients: { claim: vi.fn(), matchAll: vi.fn(async () => []), openWindow: vi.fn() },
  };
  // eslint-disable-next-line no-new-func
  new Function("self", swSource)(self);
  /** Deliver one push and wait for what it shows. */
  const deliver = async (payload) => {
    const waits = [];
    handlers.push({ data: { json: () => payload }, waitUntil: (p) => waits.push(p) });
    await Promise.all(waits);
    return showNotification.mock.calls.at(-1);
  };
  return { deliver, showNotification };
}

const fixturePayload = (over = {}) => ({
  task_id: fixture.entity_id,
  kind: fixture.kind,
  url: "/app/#/generic",
  sealed: fixture.blob,
  ...over,
});

const GENERIC_AGENT = ["Build", expect.objectContaining({ body: "An agent needs you", data: { url: "/app/#/generic" } })];

function expectContent(shown, message = fixtureMessage) {
  expect(shown).toEqual([message.title, {
    body: message.body,
    icon: "/app/static/icon-192.png",
    tag: `build-task-${fixture.entity_id}`,
    renotify: true,
    data: { url: message.url },
  }]);
}

/** A blob sealed to the fixture key with its own nonce and iat, as another
 *  bridge would seal it. */
const sealed = (message, over = {}) => sealForTest({
  recipientPublic: b64uDecode(fixture.recipient_public_key),
  sid: fixture.subscription_id,
  kind: fixture.kind,
  entityId: fixture.entity_id,
  message: { v: 1, title: "Another bridge", body: "said something", url: "/app/#/device/d/project/p?agent=a", iat: IAT, ...message },
  ...over,
});

describe("the subscription id and the key store", () => {
  it("computes the fixture's sid from its endpoint", async () => {
    expect(await pushKeys.subscriptionIdOf(fixture.endpoint)).toBe(fixture.subscription_id);
  });

  it("makes a non-extractable key per sid, deleting the stale ones, and the worker opens pushes sealed to it", async () => {
    const stale = await pushKeys.ensurePushKey("stale-sid");
    const made = await pushKeys.ensurePushKey(fixture.subscription_id);
    expect(made.created).toBe(true);
    expect(made.publicKey).not.toBe(stale.publicKey);
    expect(await pushKeys.storedKeyIds()).toEqual([fixture.subscription_id]);
    const again = await pushKeys.ensurePushKey(fixture.subscription_id);
    expect(again).toEqual({ ...made, created: false });

    const record = await pushKeys.readPushKey(fixture.subscription_id);
    expect(record.privateKey.extractable).toBe(false);
    expect(b64uDecode(made.publicKey)).toHaveLength(65);

    const message = { v: 1, title: "Own key", body: "opened", url: "/app/#/x", iat: IAT };
    const blob = await sealForTest({
      recipientPublic: b64uDecode(made.publicKey), sid: fixture.subscription_id,
      kind: "agent", entityId: fixture.entity_id, message,
    });
    const { deliver } = loadWorker();
    expectContent(await deliver(fixturePayload({ sealed: blob })), message);
  });

  it("generates one key when several greetings ask for a new sid at once", async () => {
    const answers = await Promise.all([1, 2, 3].map(() => pushKeys.ensurePushKey("new-sid")));
    expect(new Set(answers.map((answer) => answer.publicKey)).size).toBe(1);
    expect(answers.filter((answer) => answer.created)).toHaveLength(1);
  });
});

describe("a sealed push", () => {
  beforeEach(async () => {
    await storeKey(await fixtureKeyRecord());
  });

  it("shows the fixture's title and body, and its in-app url", async () => {
    const { deliver } = loadWorker();
    expectContent(await deliver(fixturePayload()));
  });

  it("the test sealer reproduces the fixture blob byte for byte", async () => {
    const ephemeralPrivate = await crypto.subtle.importKey(
      "jwk", fixture.ephemeral_private_jwk, { name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"],
    );
    const { d: _d, ...publicJwk } = fixture.ephemeral_private_jwk;
    const ephemeralPublic = await crypto.subtle.importKey("jwk", publicJwk, { name: "ECDH", namedCurve: "P-256" }, true, []);
    const blob = await sealForTest({
      recipientPublic: b64uDecode(fixture.recipient_public_key), sid: fixture.subscription_id,
      kind: fixture.kind, entityId: fixture.entity_id, message: fixture.plaintext,
      nonce: b64uDecode(fixture.nonce), ephemeral: { privateKey: ephemeralPrivate, publicKey: ephemeralPublic },
    });
    expect(blob).toBe(fixture.blob);
  });

  it("shows the generic copy when the kind in the clear is not the one sealed", async () => {
    const { deliver } = loadWorker();
    expect(await deliver(fixturePayload({ kind: "task" }))).toEqual(["Build", expect.objectContaining({ body: "New activity on a task" })]);
  });

  it("shows the generic copy when the entity in the clear is not the one sealed", async () => {
    const { deliver } = loadWorker();
    expect(await deliver(fixturePayload({ task_id: "run-other" }))).toEqual(GENERIC_AGENT);
  });

  it("shows the generic copy when the blob was sealed to another subscription", async () => {
    // This browser's subscription is another one, holding the same key: only
    // the AAD's sid differs, and the tag fails on it.
    const endpoint = "https://fcm.googleapis.com/fcm/send/another-endpoint";
    await storeKey(await fixtureKeyRecord(await pushKeys.subscriptionIdOf(endpoint)));
    const { deliver } = loadWorker({ endpoint });
    expect(await deliver(fixturePayload())).toEqual(GENERIC_AGENT);
  });

  it("never takes the sid from the payload", async () => {
    const { deliver } = loadWorker({ endpoint: "https://fcm.googleapis.com/fcm/send/another-endpoint" });
    expect(await deliver(fixturePayload({ subscription_id: fixture.subscription_id, sid: fixture.subscription_id })))
      .toEqual(GENERIC_AGENT);
  });

  it("shows content up to the window's edges and the generic copy past them", async () => {
    // PUSH_TTL_SECONDS (0) + CLOCK_SKEW_SECONDS (300) behind, the skew ahead.
    for (const [offset, opens] of [[300, true], [301, false], [-300, true], [-301, false]]) {
      vi.setSystemTime((IAT + offset) * 1000);
      const { deliver } = loadWorker();
      const shown = await deliver(fixturePayload({ sealed: await sealed({ title: `at ${offset}` }) }));
      expect([offset, shown[0]]).toEqual([offset, opens ? `at ${offset}` : "Build"]);
    }
  });

  it("shows the generic copy for a stale iat and a future one", async () => {
    vi.setSystemTime((IAT + 3600) * 1000);
    expect(await loadWorker().deliver(fixturePayload())).toEqual(GENERIC_AGENT);
    vi.setSystemTime((IAT - 3600) * 1000);
    expect(await loadWorker().deliver(fixturePayload())).toEqual(GENERIC_AGENT);
  });

  it("shows a replayed nonce generically, even from a fresh worker", async () => {
    expectContent(await loadWorker().deliver(fixturePayload()));
    expect(await loadWorker().deliver(fixturePayload())).toEqual(GENERIC_AGENT);
  });

  it("shows blobs from bridges with skewed clocks, out of order, inside the window", async () => {
    vi.setSystemTime((IAT + 200) * 1000);
    const { deliver } = loadWorker();
    const newer = await sealed({ title: "newer", iat: IAT + 150 });
    const older = await sealed({ title: "older", iat: IAT - 90 });
    const ahead = await sealed({ title: "ahead", iat: IAT + 450 });
    expect((await deliver(fixturePayload({ sealed: newer })))[0]).toBe("newer");
    expect((await deliver(fixturePayload({ sealed: older })))[0]).toBe("older");
    expect((await deliver(fixturePayload({ sealed: ahead })))[0]).toBe("ahead");
  });

  it("remembers at most 256 nonces, and forgets the ones past the window", async () => {
    const { deliver } = loadWorker();
    for (let i = 0; i < 258; i++) {
      expect((await deliver(fixturePayload({ sealed: await sealed({ title: `n${i}` }) })))[0]).toBe(`n${i}`);
    }
    expect(await seenCount()).toBe(256);
    vi.setSystemTime((IAT + 3600) * 1000);
    expect((await deliver(fixturePayload({ sealed: await sealed({ title: "late", iat: IAT + 3600 }) })))[0]).toBe("late");
    expect(await seenCount()).toBe(1);
  });

  it("uses a sealed url only inside the app, and the url in the clear otherwise", async () => {
    const { deliver } = loadWorker();
    for (const url of ["https://evil.example/app/#/x", "//evil.example/app/#/", "/app/settings", 42]) {
      const shown = await deliver(fixturePayload({ sealed: await sealed({ title: String(url), url }) }));
      expect(shown[0]).toBe(String(url));
      expect(shown[1].data).toEqual({ url: "/app/#/generic" });
    }
  });

  it("shows the generic copy for malformed blobs and plaintexts", async () => {
    const cases = [
      "!!not-base64url!!",
      "AQID",
      b64uEncode(Uint8Array.of(2, ...b64uDecode(fixture.blob).slice(1))),
      await sealed({ v: 2 }),
      await sealed({ title: 7 }),
      await sealed({ body: null }),
      await sealed({ iat: "1790000000" }),
      await sealForTest({ recipientPublic: b64uDecode(fixture.recipient_public_key), sid: fixture.subscription_id,
        kind: fixture.kind, entityId: fixture.entity_id, message: "not json" }),
    ];
    for (const blob of cases) {
      expect(await loadWorker().deliver(fixturePayload({ sealed: blob }))).toEqual(GENERIC_AGENT);
    }
  });

  it("shows the generic copy for an old payload without `sealed`, and for no subscription", async () => {
    const { sealed: _unsealed, ...old } = fixturePayload();
    expect(await loadWorker().deliver(old)).toEqual(GENERIC_AGENT);
    expect(await loadWorker({ endpoint: null }).deliver(fixturePayload())).toEqual(GENERIC_AGENT);
  });

  it("logs a reason code at debug level and never the content", async () => {
    await loadWorker().deliver(fixturePayload());
    await loadWorker().deliver(fixturePayload());
    const logged = JSON.stringify(console.debug.mock.calls);
    expect(console.debug).toHaveBeenCalledWith("sw: push shown with the generic copy", "replay");
    for (const secret of [fixtureMessage.title, fixtureMessage.body, fixtureMessage.url, fixture.blob]) {
      expect(logged).not.toContain(secret);
    }
  });
});

describe("a push with no key for this subscription", () => {
  it("shows the generic copy", async () => {
    await pushKeys.storedKeyIds();
    expect(await loadWorker().deliver(fixturePayload())).toEqual(GENERIC_AGENT);
    expect(console.debug).toHaveBeenCalledWith("sw: push shown with the generic copy", "no-key");
  });
});

describe("the freshness window", () => {
  it("uses the TTL the api sets on every push (skriftapp/buildapp/web_push.py)", () => {
    const api = read("../../skriftapp/buildapp/web_push.py").match(/^PUSH_TTL_SECONDS(?:\s*:\s*\w+)?\s*=\s*(\d+)\s*$/m);
    const worker = swSource.match(/^const PUSH_TTL_SECONDS = (\d+);$/m);
    expect(api, "web_push.py must define PUSH_TTL_SECONDS").not.toBeNull();
    expect(worker).not.toBeNull();
    expect(Number(worker[1])).toBe(Number(api[1]));
    expect(swSource).toMatch(/^const CLOCK_SKEW_SECONDS = 300;$/m);
  });
});
