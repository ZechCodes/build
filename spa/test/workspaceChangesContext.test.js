// @vitest-environment jsdom
// Real retained panes, cache, comment layers and enabled viewing context.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountWorkspaceChanges } from "../src/views/workspaceChanges.js";
import { createViewingContext } from "../src/core/viewingContext.js";
import { scopeFor } from "../src/core/cacheScope.js";
import { wipeCache, writeCached } from "../src/core/localCache.js";
import { worktreeOf } from "./gitWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const directories = [
  { sourceId: "repo", source_id: "repo", label: "Repository", path: "/workspace/repo", is_git: true },
  { sourceId: "assets", source_id: "assets", label: "Assets", path: "/workspace/assets", is_git: true },
];
const passage = "the same selected passage";
let pane;
const surface = (id) => document.querySelector(`[data-surface="${id}"]`);
const switchTo = (id) => document.querySelector(`[data-directory="${id}"]`).click();
const pop = () => document.querySelector("body > .comment-pop");

beforeEach(async () => {
  await wipeCache();
  document.body.innerHTML = "<main></main>";
});
afterEach(() => {
  pane?.dispose();
  pane = null;
  document.getSelection()?.removeAllRanges();
  document.body.innerHTML = "";
});

async function mount({ post = async () => ({}) } = {}) {
  const context = createViewingContext();
  const cacheScope = scopeFor("context-device");
  const tree = worktreeOf({ "shared.js": passage });
  const calls = [];
  await writeCached(cacheScope.address({ entityId: "", kind: "workspaces" }), [
    { id: "workspace", root: "/workspace", directories },
  ]);
  const callRpc = async (method, params) => {
    calls.push({ method, params });
    if (method === "git.status") return tree.status();
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return { commits: [], more: false };
    if (method === "git.unpushed") return { files: [], commits: [] };
    if (method === "workspace.ensure_conversation") return { entity_id: "conversation" };
    if (method === "thread.post") return post(params);
    return {};
  };
  pane = mountWorkspaceChanges(document.querySelector("main"), {
    directories, current: "repo", viewingContext: context, onSelectDirectory() {},
    git: (sourceId, viewingContext) => ({
      scope: { workspace_id: "workspace", source_id: sourceId }, callRpc, cacheScope, viewingContext,
    }),
  });
  await openUncommitted("repo");
  // Both real panes are ready before a synchronous directory change races send.
  switchTo("assets");
  await openUncommitted("assets");
  switchTo("repo");
  return { context, calls };
}

async function openUncommitted(id) {
  await vi.waitFor(() => expect(surface(id).querySelector('[data-sel="uncommitted"]')).not.toBeNull());
  surface(id).querySelector('[data-sel="uncommitted"]').click();
  await vi.waitFor(() => expect(surface(id).querySelector(".file .code")?.textContent).toBeTruthy());
}

function selectPassage(id) {
  const code = [...surface(id).querySelectorAll(".code")].find((element) => element.textContent.includes(passage));
  expect(code).toBeTruthy();
  const range = document.createRange();
  range.selectNodeContents(code);
  document.getSelection().removeAllRanges();
  document.getSelection().addRange(range);
  document.dispatchEvent(new Event("selectionchange"));
}

function sendNote() {
  const input = surface("repo").querySelector(".csinput");
  input.value = "Repository review";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  surface("repo").querySelector('[data-action="comment"]').click();
}

it("a hidden send completion preserves the shown pane's editor and matching selection", async () => {
  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const { context, calls } = await mount({ post: () => held });
  selectPassage("repo");
  const repositorySelection = context.snapshot().items.find((item) => item.kind === "selection");
  expect(repositorySelection.text).toBe(passage);
  sendNote();
  await vi.waitFor(() => expect(calls.some(({ method }) => method === "thread.post")).toBe(true));

  switchTo("assets");
  surface("assets").querySelector(".fcmt").click();
  const editor = pop();
  editor.querySelector(".cp-input").value = "Assets draft stays here";
  selectPassage("assets");
  const before = context.snapshot();
  expect(before.items.find((item) => item.kind === "selection")).toEqual(repositorySelection);

  finish({});
  await vi.waitFor(() => expect(surface("repo").querySelector(".csinput").value).toBe(""));
  expect(pop()).toBe(editor);
  expect(editor.querySelector(".cp-input").value).toBe("Assets draft stays here");
  expect(context.snapshot()).toEqual(before);
  expect(document.getSelection().toString()).toBe(passage);
  const message = calls.find(({ method }) => method === "thread.post").params.messages[0];
  expect(message.viewing_context.items).toContainEqual({ ...repositorySelection, path: "repo/shared.js" });
});

it("a send queued behind draft persistence keeps its directory's context after a synchronous switch", async () => {
  const { context, calls } = await mount();
  selectPassage("repo");
  const repositorySelection = context.snapshot().items.find((item) => item.kind === "selection");
  sendNote();
  // send() yields to draft persistence before it takes its context snapshot.
  switchTo("assets");
  context.setSelection([{ ...repositorySelection, text: "Assets-only selection" }]);
  await vi.waitFor(() => expect(calls.some(({ method }) => method === "thread.post")).toBe(true));
  const message = calls.find(({ method }) => method === "thread.post").params.messages[0];
  expect(message.viewing_context.items).toContainEqual({ ...repositorySelection, path: "repo/shared.js" });
  expect(message.viewing_context.items.some((item) => item.text === "Assets-only selection")).toBe(false);
});
