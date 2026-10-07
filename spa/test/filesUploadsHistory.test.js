// @vitest-environment jsdom
import { afterAll, afterEach, expect, it, vi } from "vitest";
const state = vi.hoisted(() => ({ snapshot: { active: [], recent: [] }, listener: null }));
vi.mock("../src/core/fileUploads.js", () => ({ uploadsFor: () => ({ snapshot: () => state.snapshot, subscribe: (fn) => { state.listener = fn; return () => {}; } }) }));
vi.mock("../src/core/deviceContexts.js", () => ({ contextFor: () => ({}) }));
vi.mock("../src/core/fileUploadRpc.js", () => ({ fileUploadRpc: (_context, rpc) => rpc }));
vi.mock("../src/core/localCache.js", () => ({ deleteCached: vi.fn(async () => {}), subscribeCache: () => () => {} }));
vi.mock("../src/core/fileUploadSupport.js", () => ({ readFileUploadSupport: async () => ({}), fileUploadSupportAddress: () => ({}) }));
vi.mock("../src/core/filesUploadActions.js", () => ({ mountFilesUploadActions: () => ({ setCapabilities() {}, dispose() {} }) }));
vi.mock("../src/core/fileUploadTray.js", () => ({ mountFileUploadTray: () => Object.assign(() => {}, { setCapabilities() {} }) }));
const { mountFilesUploads } = await import("../src/core/filesUploads.js");
let mounted;
afterEach(() => mounted?.dispose());
afterAll(() => {
  for (const name of ["fileUploads", "deviceContexts", "fileUploadRpc", "localCache", "fileUploadSupport", "filesUploadActions", "fileUploadTray"]) vi.doUnmock(`../src/core/${name}.js`);
  vi.resetModules();
});
const mount = (scope) => {
  const root = { id: "code", scope };
  const tree = { reveal: vi.fn(async () => {}), relist: vi.fn() };
  mounted = mountFilesUploads({ roots: [root], keyOf: (_root, path) => path, tree, callRpc: vi.fn(), deviceId: "d", listingAddress: () => ({}) });
  return tree;
};
const completed = (scope) => ({ id: "upload", rootId: "code", scope, parent: "ignored", path: "ignored/a", status: "finished", finishedAt: 123 });
for (const scope of [{ workspace_id: "w", source_id: "code" }, { project_id: "p", source_id: "code" }]) {
  it(`does not reveal completed history on remount ${JSON.stringify(scope)}`, async () => {
    state.snapshot = { active: [], recent: [completed(scope)] };
    const tree = mount(scope);
    await Promise.resolve(); await Promise.resolve();
    expect(tree.reveal).not.toHaveBeenCalled(); expect(tree.relist).not.toHaveBeenCalled();
  });
  it(`ignores hydrated history but reveals in-flight completions ${JSON.stringify(scope)}`, async () => {
    state.snapshot = { active: [{ ...completed(scope), finishedAt: null, status: "uploading" }], recent: [] };
    const tree = mount(scope);
    state.listener({ active: state.snapshot.active, recent: [{ ...completed(scope), id: "historical" }] });
    await Promise.resolve(); await Promise.resolve();
    expect(tree.reveal).not.toHaveBeenCalled();
    state.listener({ active: [], recent: [completed(scope)] });
    await vi.waitFor(() => expect(tree.reveal).toHaveBeenCalledWith("ignored/a"));
    expect(tree.relist).toHaveBeenCalledWith(["ignored"], "code");
  });
}
it("ignores persisted history arriving after an initially empty mount", async () => {
  const scope = { project_id: "p", source_id: "code" };
  state.snapshot = { active: [], recent: [] };
  const tree = mount(scope);
  state.listener({ active: [], recent: [completed(scope)] });
  await Promise.resolve(); await Promise.resolve();
  expect(tree.reveal).not.toHaveBeenCalled(); expect(tree.relist).not.toHaveBeenCalled();
});
