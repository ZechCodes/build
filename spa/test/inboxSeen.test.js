import { beforeEach, expect, it, vi } from "vitest";

const call = vi.fn();
const capture = vi.fn();
const merge = vi.fn();
vi.mock("../src/core/inboxDevices.js", () => ({ verbCall: () => call }));
vi.mock("../src/core/deviceContexts.js", () => ({ homeContext: () => ({ deviceId: "dev-1" }) }));
vi.mock("../src/core/localCache.js", () => ({
  captureCachedRecord: (...args) => capture(...args),
  cachedWriteOf: (record) => record?.written || null,
  mergeCachedIfUnwritten: (...args) => merge(...args),
}));
const { indexRowsByEntity, markSeen } = await import("../src/core/inboxSeen.js");

beforeEach(() => {
  call.mockReset();
  capture.mockReset().mockResolvedValue(null);
  merge.mockReset();
  indexRowsByEntity([]);
});

it("confirms a stored read cursor, including the partial window and thread generation", async () => {
  call.mockResolvedValue({ ok: true });
  await expect(markSeen("run-1", "agent-1", 11, 12, "thread-1")).resolves.toBe(true);
  expect(call).toHaveBeenCalledWith("entity.seen", {
    entity_id: "run-1", agent_id: "agent-1", thread_id: "thread-1",
    read_from_sequence: 11, read_through_sequence: 12,
  });
});

it("leaves a failed read eligible for retry instead of confirming it", async () => {
  call.mockRejectedValue(new Error("temporarily unavailable"));
  await expect(markSeen("run-1", "agent-1", 11, 12)).resolves.toBe(false);
  expect(call).toHaveBeenCalledTimes(1);
});

it("refreshes the read owner's cached roster even when an older bridge pushes no row", async () => {
  const row = { run_id: "run-1", kind: "project", agents: [{ id: "agent-1", unread_count: 2 }] };
  const updated = { ...row, agents: [{ id: "agent-1", unread_count: 0, read_through_sequence: 12 }] };
  indexRowsByEntity([{ ...row, deviceId: "dev-2" }]);
  capture.mockResolvedValue({ value: row, written: "before-pull" });
  call.mockResolvedValueOnce({ ok: true }).mockResolvedValueOnce({ items: [], runs: [updated] });
  await expect(markSeen("run-1", "agent-1", 11, 12)).resolves.toBe(true);
  expect(call).toHaveBeenLastCalledWith("board.list", {});
  const [address, written, replace] = merge.mock.calls[0];
  expect(address).toMatchObject({ deviceId: "dev-2", entityId: "run-1", kind: "row" });
  expect(written).toBe("before-pull");
  expect(replace(row)).toEqual({ ...updated, deviceId: "dev-2" });
  expect(replace(null)).toBeNull();
});

it("keeps a confirmed cursor confirmed when the roster refresh fails", async () => {
  capture.mockResolvedValue({ value: { run_id: "run-1" }, written: "before-pull" });
  call.mockResolvedValueOnce({ ok: true }).mockRejectedValueOnce(new Error("read failed"));
  await expect(markSeen("run-1", "agent-1", 11, 12)).resolves.toBe(true);
});
