// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { writeCached, wipeCache, readCached } from "../src/core/localCache.js";
import { mountArchivedProjectView } from "../src/core/archivedProjectView.js";
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => ({ environments: { archive: true } }) }));
let view;
const project = { project_id: "p1", name: "Old line", archived_at: 12 };
const address = { deviceId: "d1", entityId: "", kind: "projects" };
beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = '<main><div data-banner></div><section data-content></section></main>';
  await writeCached(address, [project]);
  await writeCached({ ...address, kind: "environment-support" }, { archive: true });
});
afterEach(() => view?.dispose());
function mount(callRpc = vi.fn()) {
  view = mountArchivedProjectView(document.querySelector("main"), { deviceId: "d1", projectId: "p1", callRpc });
  return view;
}
it("archived project view shows the banner with restore", async () => {
  const callRpc = vi.fn(async () => ({ project: { ...project, archived_at: null } })); mount(callRpc);
  await vi.waitFor(() => expect(document.querySelector("[data-project-archived]")?.textContent).toContain("Old line is archived"));
  document.querySelector("[data-project-restore]").click();
  await vi.waitFor(async () => expect((await readCached(address)).value[0].archived_at).toBeNull());
  await vi.waitFor(() => expect(document.querySelector("[data-project-archived]")).toBeNull());
  expect(callRpc).toHaveBeenCalledWith("project.restore", { project_id: "p1" });
});
it("archived task and review views disable mutations", async () => {
  const content = document.querySelector("[data-content]");
  content.innerHTML = '<button data-task-state>Close task</button><select id="task-status"><option>Ready</option></select><textarea id="task-comment"></textarea><button data-review-verdict="approve">Approve</button><button data-review-view="files">Files</button><a href="#task">Task</a>';
  mount();
  await vi.waitFor(() => expect(content.querySelector("[data-task-state]").disabled).toBe(true));
  for (const selector of ["#task-status", "#task-comment", "[data-review-verdict]"]) expect(content.querySelector(selector).disabled).toBe(true);
  expect(content.querySelector("[data-review-view]").disabled).toBe(false);
  content.insertAdjacentHTML("beforeend", '<button data-review-reclaim>Reclaim</button>');
  await vi.waitFor(() => expect(content.querySelector("[data-review-reclaim]").disabled).toBe(true));
  await writeCached(address, [{ ...project, archived_at: null }]);
  await vi.waitFor(() => expect(content.querySelector("[data-task-state]").disabled).toBe(false));
});
it("project_archived refusal is shown verbatim", async () => {
  const message = "Old <line> is archived; restore it first";
  await writeCached(address, [{ ...project, archived_at: null }]);
  const callRpc = vi.fn(async () => { throw Object.assign(new Error(message), { code: "project_archived" }); });
  const view = mount(callRpc);
  await expect(view.callRpc("tasks.update", {})).rejects.toThrow(message);
  expect(document.querySelector("[data-project-archive-error]").textContent).toBe(message);
  expect(document.querySelector("line")).toBeNull();
});
