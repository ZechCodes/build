// @vitest-environment jsdom
// The Changes surface over the split wire: git.status carries shape, each
// file's body comes from git.diff, and a shape that has not moved is answered
// with a key rather than a diff.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountGitPane } from "../src/core/gitPane.js";
import { COLLAPSED_PREVIEW_ROWS } from "../src/core/fileEntries.js";
import { unchangedStatus, worktreeOf } from "./gitWireFixture.js";

const log = () => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier work", author: "Zech", email: "z@x", time: 1 }],
  more: false,
});

const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

/** Mount the pane over a worktree fixture; `answers` overrides one verb. */
async function mount({ tree, answers = {}, scope = { project_id: "p1" } } = {}) {
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
  const pane = mountGitPane(container, { scope, callRpc });
  await settle();
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

beforeEach(() => {
  document.body.innerHTML = "";
});

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("the shape and its bodies", () => {
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
    const { container, pane, calls } = await mount({ tree, answers: { "git.status": () => held } });
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
    const { container: second, pane: secondPane, calls: secondCalls } = await mount({ tree, answers });
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
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
    const { container, pane, calls } = await mount({ tree });
    const held = fileOf(container, "src/b.js");
    calls.length = 0;
    tree.write("src/a.js", "the agent moved on");
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(pathsAsked(calls)).toEqual([["src/a.js"]]);
    expect(container.textContent).toContain("the agent moved on");
    expect(fileOf(container, "src/b.js")).toBe(held);
    pane.dispose();
  });

  it("leaves a file the reader folded shut unfetched until they open it again", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line", "src/b.js": "second" });
    const { container, pane, calls } = await mount({ tree });
    const head = () => fileOf(container, "src/a.js").querySelector(".fhead");
    await click(head()); // capped → shut
    expect(fileOf(container, "src/a.js").classList.contains("collapsed")).toBe(true);

    calls.length = 0;
    tree.write("src/a.js", "the agent moved on");
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(pathsAsked(calls)).toEqual([]);
    expect(container.textContent).not.toContain("the agent moved on");

    await click(head()); // shut → open, which is when the body is worth having
    expect(pathsAsked(calls)).toEqual([["src/a.js"]]);
    expect(container.textContent).toContain("the agent moved on");
    pane.dispose();
  });

  it("asks again after a body fetch fails, even though the shape has not moved", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line" });
    const { container, pane, calls } = await mount({ tree, answers: refusingTheFirstBody(tree) });
    expect(container.textContent).toContain("loading…");

    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(pathsAsked(calls)).toEqual([["src/a.js"], ["src/a.js"]]);
    expect(container.textContent).toContain("new line");
    pane.dispose();
  });

  it("paints a body that landed while the reader was mid-draft on the next free turn", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const tree = worktreeOf({ "src/a.js": "new line" });
    const { container, pane } = await mount({ tree, answers: refusingTheFirstBody(tree) });
    const draft = container.querySelector(".csinput");
    draft.value = "a commit message being typed";

    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(container.textContent).not.toContain("new line"); // the draft holds the repaint

    draft.value = "";
    await vi.advanceTimersByTimeAsync(2000);
    await settle();
    expect(container.textContent).toContain("new line");
    pane.dispose();
  });

  it("draws a collapsed file's header and a peek, not its whole diff", async () => {
    const long = Array.from({ length: 20 }, (_unused, index) => `line ${index}`);
    const tree = worktreeOf({ "src/a.js": long });
    const { container, pane } = await mount({ tree });
    expect(rowsIn(fileOf(container, "src/a.js"))).toBe(23);
    await click(fileOf(container, "src/a.js").querySelector(".fhead")); // capped → shut
    expect(rowsIn(fileOf(container, "src/a.js"))).toBe(COLLAPSED_PREVIEW_ROWS);
    expect(fileOf(container, "src/a.js").textContent).toContain("src/a.js");
    pane.dispose();
  });
});
