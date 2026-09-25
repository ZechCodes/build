// @vitest-environment jsdom
// A subscribe refused for a kind the bridge does not know, end to end on the
// client (#2). Nothing here is mocked: the refusal is the bridge's own reply
// from `fixtures/api/v1/changes.subscribe.json` (which `bridge/src/api/v1/
// changes.rs` pins to what the verb answers), carried by the real session rpc
// (core/sessionRpc.js) into the real subscription manager (core/changeEvents.js).
// Only the wire under the session is a stand-in, answering like the bridge.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it } from "vitest";
import { createSessionRpc } from "../src/core/sessionRpc.js";

const fixture = (name) => JSON.parse(readFileSync(resolve(process.cwd(), `../fixtures/api/v1/${name}.json`), "utf8"));
const greeting = fixture("session.hello").result;
const { refusal } = fixture("changes.subscribe");
const [unknownKind] = refusal.reply.details.kinds;

let changeEvents;
let subscribes;

/** The session's wire, answering as the bridge does: the greeting, and a
 *  subscribe refused with the fixture's reply while it names a kind the
 *  greeting does not advertise. */
function bridgeWire() {
  const listeners = new Set();
  const answer = (payload) => {
    const { id, method, params } = payload;
    if (method === "session.hello") return { id, ok: true, result: greeting };
    if (method === "changes.subscribe") {
      subscribes.push(params);
      if (!params.kinds.every((kind) => greeting.changes.kinds.includes(kind))) return { ...refusal.reply, id };
      return { id, ok: true, result: { subscription_id: params.subscription_id, watch: "live" } };
    }
    if (method === "changes.unsubscribe") return { id, ok: true, result: { ok: true } };
    return { id, ok: true, result: {} };
  };
  return {
    send: async (envelope) => {
      const reply = answer(envelope.frameFields.payload);
      queueMicrotask(() => listeners.forEach((fn) => fn({ frameFields: { payload: reply } })));
    },
    onEnvelope: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

const transport = {
  encryptFrame: async ({ frameFields }) => ({ frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

beforeEach(async () => {
  subscribes = [];
  changeEvents = await import("../src/core/changeEvents.js");
});

afterEach(() => changeEvents.resetChangeEvents());

it("re-subscribes once without the kind the bridge named, keeping every other kind", async () => {
  expect(greeting.capabilities).toContain("changes.refusedKinds");
  expect(greeting.changes.kinds).not.toContain(unknownKind);
  const { kinds } = refusal.params;
  changeEvents.watchChanges({ refresh: () => {}, id: "s-inbox", scope: "all", kinds, mode: "realtime" });
  changeEvents.watchChanges({ refresh: () => {}, id: "s-active", entity: "run-7", kinds: ["git", "files"] });

  const session = createSessionRpc({ transport, sessionId: "sess-1", sessionKeyB64: "key-1", deviceId: "dev-1" });
  session.rideOn(bridgeWire());
  await changeEvents.greetBridge(session.call, { deviceId: "dev-1" });
  await changeEvents.subscriptionsSettled();

  const inbox = subscribes.filter((spec) => spec.subscription_id === "s-inbox");
  expect(inbox.map((spec) => spec.kinds)).toEqual([kinds, kinds.filter((kind) => kind !== unknownKind)]);
  expect(subscribes.filter((spec) => spec.subscription_id === "s-active:run-7")).toHaveLength(1);

  // Held as wanted: a later diff asks for neither again.
  changeEvents.watchChanges({ refresh: () => {}, entity: "run-9", kinds: ["git"] });
  await changeEvents.subscriptionsSettled();
  expect(subscribes.filter((spec) => spec.subscription_id === "s-inbox")).toHaveLength(2);
});
