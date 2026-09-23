// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let createCommentLayer;
let readCached;
let uiAddress;
const address = { deviceId: "dev-1", entityId: "run-1", kind: "ui-draft", sub: "changes:comments" };
const host = () => document.querySelector("#changes");
const layer = (submit = vi.fn(async () => {})) => createCommentLayer({
  submit,
  revisionId: () => "revision-1",
  cacheAddressOf: () => uiAddress({ deviceId: "dev-1", entityId: "run-1", view: "changes", kind: "draft", sub: "comments" }),
});

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="changes"><div class="file" data-key="M:src/a.js"><button class="fcmt">Comment</button></div></div>';
  ({ createCommentLayer } = await import("../src/core/changesComments.js"));
  ({ readCached } = await import("../src/core/localCache.js"));
  ({ uiAddress } = await import("../src/core/localUiState.js"));
});

it("keeps anchored comments across remount, then clears them after sending", async () => {
  const first = layer();
  first.attach(host());
  first.handleClick({ target: host().querySelector(".fcmt") });
  const input = document.querySelector(".cp-input");
  input.value = "Please check this line";
  document.querySelector(".cp-save").click();
  await vi.waitFor(async () => expect((await readCached(address))?.value.comments).toHaveLength(1));
  first.dispose();

  const submit = vi.fn(async () => {});
  const second = layer(submit);
  second.attach(host());
  await vi.waitFor(() => expect(second.count()).toBe(1));
  await second.send();
  expect(submit).toHaveBeenCalledOnce();
  expect(submit.mock.calls[0][0][0]).toMatchObject({ body: "Please check this line" });
  await vi.waitFor(async () => expect((await readCached(address))?.value.comments).toEqual([]));
  second.dispose();
});
