// @vitest-environment jsdom
// The Changes surface over the split wire: git.status carries shape, each
// file's body comes from git.diff, and a shape that has not moved is answered
// with a key rather than a diff.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { waitFor } from "./waitFor.js";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { mountGitPane } from "../src/core/gitPane.js";
import { refetchEverything } from "../src/core/changeEvents.js";
import { COLLAPSED_PREVIEW_ROWS } from "../src/core/fileEntries.js";
import { scopeFor } from "../src/core/cacheScope.js";
import { wipeCache } from "../src/core/localCache.js";
import { unchangedStatus, worktreeOf } from "./gitWireFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const testCacheScope = scopeFor("dev-1");

const log = () => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier work", author: "Zech", email: "z@x", time: 1 }],
  more: false,
});

// Observe the real body reader finishing, including a failed or unchanged
// read: those passes intentionally have no new DOM to wait for.
const bodyReads = vi.hoisted(() => new Map());
vi.mock("../src/core/fileDiffs.js", async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    createFileDiffs(options) {
      const reader = actual.createFileDiffs(options);
      const reads = { completed: 0 };
      bodyReads.set(options.call, reads);
      return {
        ...reader,
        sync(options) {
          const reading = reader.sync(options);
          reading.then(() => reads.completed++, () => reads.completed++);
          return reading;
        },
      };
    },
  };
});

const click = (element) => element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
const reread = async (...panes) => {
  const before = panes.map(({ callRpc }) => bodyReads.get(callRpc).completed);
  refetchEverything();
  await vi.advanceTimersByTimeAsync(0);
  await waitFor(() => panes.forEach(({ callRpc }, index) => {
    expect(bodyReads.get(callRpc).completed).toBeGreaterThan(before[index]);
  }));
};

/** Mount the pane over a worktree fixture; `answers` overrides one verb. */
async function mount({ tree, answers = {}, scope = { project_id: "p1" }, cacheScope = null, ready = null } = {}) {
  const calls = [];
  const callRpc = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (answers[method]) return answers[method](params);
    if (method === "git.status") return tree.status();
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return log();
    return {};
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope, callRpc, cacheScope });
  if (ready) await waitFor(() => ready(container), { timeout: 5_000 });
  else await waitFor(() => expect(bodyReads.get(callRpc)?.completed).toBeGreaterThan(0));
  return { container, pane, calls, callRpc };
}

const pathsAsked = (calls) => calls.filter((call) => call.method === "git.diff").map((call) => call.params.paths);
const fileOf = (container, path) => [...container.querySelectorAll(".file")].find((file) => file.dataset.key.endsWith(path));
const rowsIn = (element) => element.querySelectorAll("tr").length;

/** A bridge that refuses the first body it is asked for — busy, or a connection
 *  blip — and answers every one after it, while the shape never moves. */
function refusingTheFirstBody(tree) {
  const held = tree.status();
  let refused = false;
  return {
    "git.status": (params) => (params.if_status_key === held.status_key ? unchangedStatus(held) : held),
    "git.diff": (params) => {
      if (refused) return tree.diff(params);
      refused = true;
      return Promise.reject(new Error("bridge busy"));
    },
  };
}

beforeEach(async () => {
  bodyReads.clear();
  await wipeCache();
  document.body.innerHTML = "";
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("the shape and its bodies", () => {
  it("opens workspace directories on every change not represented by their push target", async () => {
    const tree = worktreeOf({ "dirty.js": "dirty" });
    const aggregatePatch = tree.wholePatch();
    const { container, pane, calls } = await mount({
      tree,
      scope: { workspace_id: "ws-1", source_id: "dir-1" },
      cacheScope: testCacheScope,
      ready: (container) => expect(container.textContent).toContain("dirty"),
      answers: {
        "git.unpushed": () => ({
          patch: aggregatePatch,
          diff_key: "all-1",
          base: { kind: "push_target", label: "fork/main" },
          file_edited_at: {},
        }),
      },
    });
    await waitFor(() => {
      const reviewRow = container.querySelector('.rrow[data-sel="review"]');
      expect(reviewRow).toBeTruthy();
      expect(reviewRow.querySelector(".rsub").textContent).toBe("vs fork/main");
      expect(container.textContent).toContain("dirty");
      expect(calls.some(({ method, params }) => method === "git.unpushed" && params.workspace_id === "ws-1")).toBe(true);
    });
    pane.dispose();
  });

  it("draws the shape's files, then their bodies from git.diff", async () => {
    const tree = worktreeOf({ "src/a.js": "new line", "src/b.js": "second" });
    const { container, pane, calls } = await mount({ tree });
    expect(pathsAsked(calls)).toEqual([["src/a.js", "src/b.js"]]);
    expect(container.textContent).toContain("new line");
    expect(container.textContent).toContain("second");
    pane.dispose();
  });

  it("sends the status key it holds, and does nothing at all with an unchanged answer", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line" });
    const held = tree.status();
    const { container, pane, calls, callRpc } = await mount({ tree, answers: { "git.status": () => held } });
    const stack = container.querySelector(".dstack");
    const before = fileOf(container, "src/a.js");
    const writes = [];
    const observer = new window.MutationObserver((records) => writes.push(...records));
    observer.observe(stack, { childList: true, subtree: true, characterData: true, attributes: true });

    calls.length = 0;
    let unchangedAsked = 0;
    const { status_key } = held;
    const answers = { "git.status": (params) => (params.if_status_key === status_key ? (unchangedAsked++, unchangedStatus(held)) : held) };
    // the pane's own poll re-asks with the key it holds
    const { container: second, pane: secondPane, calls: secondCalls, callRpc: secondCall } = await mount({ tree, answers });
    await reread({ callRpc }, { callRpc: secondCall });
    expect(unchangedAsked).toBeGreaterThan(0);
    expect(second.textContent).toContain("new line");
    observer.disconnect();
    expect(writes).toEqual([]);
    expect(fileOf(container, "src/a.js")).toBe(before);
    expect(pathsAsked(secondCalls)).toEqual([["src/a.js"]]);
    secondPane.dispose();
    pane.dispose();
  });

  it("asks again only for the file whose content moved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line", "src/b.js": "second" });
    const { container, pane, calls, callRpc } = await mount({ tree });
    const held = fileOf(container, "src/b.js");
    calls.length = 0;
    tree.write("src/a.js", "the agent moved on");
    await reread({ callRpc });
    expect(pathsAsked(calls)).toEqual([["src/a.js"]]);
    expect(container.textContent).toContain("the agent moved on");
    expect(fileOf(container, "src/b.js")).toBe(held);
    pane.dispose();
  });

  it("leaves a file the reader folded shut unfetched until they open it again", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line", "src/b.js": "second" });
    const { container, pane, calls, callRpc } = await mount({ tree });
    const head = () => fileOf(container, "src/a.js").querySelector(".fhead");
    click(head()); // capped → shut
    expect(fileOf(container, "src/a.js").classList.contains("collapsed")).toBe(true);

    calls.length = 0;
    tree.write("src/a.js", "the agent moved on");
    await reread({ callRpc });
    expect(pathsAsked(calls)).toEqual([]);
    expect(container.textContent).not.toContain("the agent moved on");

    click(head()); // shut → open, which is when the body is worth having
    await waitFor(() => expect(container.textContent).toContain("the agent moved on"));
    expect(pathsAsked(calls)).toEqual([["src/a.js"]]);
    expect(container.textContent).toContain("the agent moved on");
    pane.dispose();
  });

  it("asks again after a body fetch fails, even though the shape has not moved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line" });
    const { container, pane, calls, callRpc } = await mount({ tree, answers: refusingTheFirstBody(tree) });
    expect(container.textContent).toContain("loading…");

    await reread({ callRpc });
    expect(pathsAsked(calls)).toEqual([["src/a.js"], ["src/a.js"]]);
    expect(container.textContent).toContain("new line");
    pane.dispose();
  });

  it("paints a body that landed while the reader was mid-draft on the next free turn", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line" });
    const { container, pane, callRpc } = await mount({ tree, answers: refusingTheFirstBody(tree) });
    const draft = container.querySelector(".csinput");
    draft.value = "a commit message being typed";

    await reread({ callRpc });
    expect(container.textContent).not.toContain("new line"); // the draft holds the repaint

    draft.value = "";
    await reread({ callRpc });
    expect(container.textContent).toContain("new line");
    pane.dispose();
  });

  it("draws a collapsed file's header and a peek, not its whole diff", async () => {
    const long = Array.from({ length: 20 }, (_unused, index) => `line ${index}`);
    const tree = worktreeOf({ "src/a.js": long });
    const { container, pane } = await mount({ tree });
    expect(rowsIn(fileOf(container, "src/a.js"))).toBe(23);
    click(fileOf(container, "src/a.js").querySelector(".fhead")); // capped → shut
    expect(rowsIn(fileOf(container, "src/a.js"))).toBe(COLLAPSED_PREVIEW_ROWS);
    expect(fileOf(container, "src/a.js").textContent).toContain("src/a.js");
    pane.dispose();
  });
});
