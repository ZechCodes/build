// The api's refusal reaches the human: an approve past the device cap answers
// 409 with a sentence, and that sentence — not a generic "approve failed" — is
// what the pairing sheet and the gate print.
import { describe, expect, it, vi } from "vitest";
import { approveDevice } from "../src/api.js";

function answering(status, body) {
  return vi.fn(async () => ({
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  }));
}

describe("approveDevice", () => {
  it("throws the api's own detail when it refuses", async () => {
    globalThis.fetch = answering(409, {
      detail: "this account already has 3 devices — revoke one in Settings → Devices to add another",
    });
    await expect(approveDevice("ABCD-EFGH")).rejects.toThrow(/already has 3 devices/);
  });

  it("falls back to a plain message when the refusal carries none", async () => {
    globalThis.fetch = answering(500, {});
    await expect(approveDevice("ABCD-EFGH")).rejects.toThrow("approve failed");
  });

  it("resolves quietly on success", async () => {
    globalThis.fetch = answering(200, { device_id: "d", approved: true });
    await expect(approveDevice("ABCD-EFGH")).resolves.toBeUndefined();
  });
});
