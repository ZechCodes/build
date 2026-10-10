import { beforeEach, expect, it, vi } from "vitest";

const call = vi.fn();
vi.mock("../src/core/inboxDevices.js", () => ({ verbCall: () => call }));
const { markSeen } = await import("../src/core/inboxSeen.js");

beforeEach(() => { call.mockReset(); });

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
});
