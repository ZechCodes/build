import { expect, it, vi } from "vitest";
import * as scopes from "../src/core/syncVisibleScopes.js";

it("announces mounted read lifetimes and forwards a bounded file hint to its reader", async () => {
  const changed = vi.fn();
  const stop = scopes.subscribeSyncReads(changed);
  const read = vi.fn();
  const release = scopes.registerSyncRead(() => ({ deviceId: "hint-device", entityId: "hint-owner", kind: "tree" }), read);
  await Promise.resolve();
  expect(changed).toHaveBeenCalled();
  const files = { paths: ["src/file.js"], truncated: false };
  const visible = scopes.visibleSyncReads("hint-device")[0];
  await visible.read(() => true, { files });
  expect(read).toHaveBeenCalledWith(expect.any(Function), { files });
  changed.mockClear();
  release();
  expect(changed).toHaveBeenCalled();
  expect(scopes.visibleSyncReads("hint-device")).toEqual([]);
  stop();
});
