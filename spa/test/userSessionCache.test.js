// The user's session is one record per device, written by every list read.
import { expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { readUserSession, userSessionOf, writeUserSession } = await import("../src/core/userSessionCache.js");

const answer = (last, over = {}) => ({
  project_id: "proj-1",
  issues: [],
  user_session: {
    session_started_ms: 1, last_activity_ms: last, previous_session_ended_ms: null, gap_ms: 21_600_000, now_ms: 1_000, ...over,
  },
});
const HEARD = 5_000;

it("keeps the newest session when two list reads land out of order", async () => {
  expect(await writeUserSession("dev-1", answer(20), HEARD)).toBe(true);
  expect(await writeUserSession("dev-1", answer(10), HEARD)).toBe(false);
  expect((await readUserSession("dev-1")).last_activity_ms).toBe(20);
  expect(await writeUserSession("dev-1", answer(20), HEARD)).toBe(false);
  expect(await writeUserSession("dev-1", answer(30, { previous_session_ended_ms: 20 }), HEARD)).toBe(true);
  expect(await readUserSession("dev-1")).toEqual({
    ...answer(30, { previous_session_ended_ms: 20 }).user_session, received_ms: HEARD,
  });
});

it("holds the bridge's clock beside the session, and rereads it only when it moved", async () => {
  expect(await writeUserSession("dev-3", answer(20), HEARD)).toBe(true);
  // Ten seconds on, both clocks moved together: nothing to write.
  expect(await writeUserSession("dev-3", answer(20, { now_ms: 11_000 }), HEARD + 10_000)).toBe(false);
  // This device's clock was set back an hour: the same session, a new skew.
  expect(await writeUserSession("dev-3", answer(20, { now_ms: 11_000 }), HEARD + 10_000 - 3_600_000)).toBe(true);
  expect((await readUserSession("dev-3")).received_ms).toBe(HEARD + 10_000 - 3_600_000);
});

it("writes nothing for a bridge that sends no session", async () => {
  expect(userSessionOf({ issues: [] })).toBeNull();
  expect(await writeUserSession("dev-2", { issues: [] })).toBe(false);
  expect(await readUserSession("dev-2")).toBeNull();
});
