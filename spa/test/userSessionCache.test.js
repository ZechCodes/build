// The user's session is one record per device, written by every list read.
import { expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { readUserSession, userSessionOf, writeUserSession } = await import("../src/core/userSessionCache.js");

const answer = (last, over = {}) => ({
  project_id: "proj-1",
  issues: [],
  user_session: { session_started_ms: 1, last_activity_ms: last, previous_session_ended_ms: null, gap_ms: 21_600_000, ...over },
});

it("keeps the newest session when two list reads land out of order", async () => {
  expect(await writeUserSession("dev-1", answer(20))).toBe(true);
  expect(await writeUserSession("dev-1", answer(10))).toBe(false);
  expect((await readUserSession("dev-1")).last_activity_ms).toBe(20);
  expect(await writeUserSession("dev-1", answer(20))).toBe(false);
  expect(await writeUserSession("dev-1", answer(30, { previous_session_ended_ms: 20 }))).toBe(true);
  expect(await readUserSession("dev-1")).toEqual(answer(30, { previous_session_ended_ms: 20 }).user_session);
});

it("writes nothing for a bridge that sends no session", async () => {
  expect(userSessionOf({ issues: [] })).toBeNull();
  expect(await writeUserSession("dev-2", { issues: [] })).toBe(false);
  expect(await readUserSession("dev-2")).toBeNull();
});
