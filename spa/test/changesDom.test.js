// @vitest-environment jsdom
// The rebuilt Changes surface, end to end in the DOM: Uncommitted on top with
// its +/− counts, one renderer for every changeset, no staging, discard behind
// the file header's ⋯, noise collapsed rather than hidden, and the per-file ✎
// posting anchored comments to the agent.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountGitPane, taskAgentCommitOptions } from "../src/core/gitPane.js";

import { patchFor, worktreeOf } from "./gitWireFixture.js";

// The working tree the pane reads: two dirty files, one of them noise. An edit
// is `tree.write(path, line)` — the file's content key moves with it, and so
// does the shape's status_key, exactly as the bridge would.
let tree = worktreeOf({ "src/a.js": "new line", "uv.lock": "locked" });

const dirtyStatus = (overrides = {}) => tree.status({ stat: { files_changed: 2, insertions: 6, deletions: 3 }, ...overrides });

const cleanStatus = () =>
  dirtyStatus({ files: [], stat: { files_changed: 0, insertions: 0, deletions: 0 } });

const log = () => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "earlier work", author: "Zech", email: "z@x", time: 1 }],
  more: false,
});

const show = () => ({
  hash: "a".repeat(40),
  short: "aaaaaaa",
  subject: "earlier work",
  body: "why it happened",
  author: "Zech",
  email: "z@x",
  stat: { files_changed: 1, insertions: 1, deletions: 1 },
  patch: patchFor("src/b.js", "committed line"),
  truncated: false,
});

/** Mount the pane over a scripted RPC channel, then let the first poll land. */
async function mount({ status = dirtyStatus(), scope = { run_id: "run-1" }, rpc = {}, review = null } = {}) {
  const calls = [];
  let currentStatus = status;
  const callRpc = vi.fn(async (method, params) => {
    calls.push({ method, params });
    if (rpc[method]) return rpc[method](params);
    if (method === "git.status") return currentStatus;
    if (method === "git.diff") return tree.diff(params);
    if (method === "git.log") return log();
    if (method === "git.show") return show();
    if (method === "git.stage") return currentStatus;
    if (method === "git.commit") return { hash: "b".repeat(40), short: "bbbbbbb", subject: "s", status: cleanStatus() };
    if (method === "git.discard") return currentStatus;
    if (method === "run.request_changes") return { ok: true };
    return {};
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, { scope, callRpc, review });
  await settle();
  return { container, pane, callRpc, calls };
}

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

beforeEach(() => {
  tree = worktreeOf({ "src/a.js": "new line", "uv.lock": "locked" });
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("the Changes rail", () => {
  it("puts Uncommitted at the top with +/− counts and opens on it", async () => {
    const { container, pane } = await mount();
    const rows = [...container.querySelectorAll(".crail .rrow, .crail .crow")];
    expect(rows[0].dataset.sel).toBe("uncommitted");
    expect(rows[0].textContent).toContain("+6");
    expect(rows[0].textContent).toContain("−3");
    expect(rows[0].classList.contains("sel")).toBe(true);
    pane.dispose();
  });

  it("opens a clean branch at the commit list — nothing selected, no commit box", async () => {
    const { container, pane } = await mount({ status: cleanStatus() });
    expect(container.querySelector(".crail .sel")).toBe(null);
    expect(container.querySelector(".gitmsg")).toBe(null);
    expect(container.querySelector(".cdetail-host").textContent).toContain("Pick a commit");
    pane.dispose();
  });

  it("discloses the commit box only while uncommitted changes exist", async () => {
    const { container, pane } = await mount();
    expect(container.querySelector(".gitmsg")).toBeTruthy();
    pane.dispose();
  });

  it("says once, and in its own words, that a clean tree has nothing in it", async () => {
    const { container, pane } = await mount({ status: cleanStatus() });
    await click(container.querySelector('.rrow[data-sel="uncommitted"]'));
    const detail = container.querySelector(".cdetail-host");
    expect(detail.textContent).toContain("No uncommitted changes.");
    expect(detail.textContent).not.toContain("No file changes");
    expect(detail.querySelectorAll(".empty").length).toBe(1);
    pane.dispose();
  });
});

// The aggregate review is a plug that owns the detail pane while it is
// selected, and every other changeset is painted into the same host. Whoever
// wrote there last, the next painter finds the parts it owns or makes them.
describe("the file menu a repaint finds open", () => {
  it("is the same menu afterwards, opened over the same file", async () => {
    const { container, pane } = await mount();
    await click(container.querySelector('.file[data-key$=":src/a.js"] .fmenu'));
    const pop = container.querySelector(".fmenu-pop");
    expect(pop).toBeTruthy();

    await click(container.querySelector(".noisehead"));
    expect(container.querySelector(".noisefiles")).toBeTruthy();
    expect(container.querySelector(".fmenu-pop")).toBe(pop);
    pane.dispose();
  });
});

describe("the detail pane the review plug and the changesets share", () => {
  /** A plug with the shape the real one has: it writes its own bar, its own
   *  keyed stack and its own tray into the host it is handed. */
  const stubReview = () => {
    const calls = [];
    return {
      calls,
      getBase: () => "main",
      mount: (host) => {
        calls.push("mount");
        host.innerHTML =
          '<div class="csbar">aggregate</div><div class="dstack" data-keyed-list><div class="file" data-key="EDIT:agg.js"></div></div><div class="cstray"></div>';
      },
      unmount: () => calls.push("unmount"),
    };
  };

  it("paints a changeset again after the plug has owned the pane, twice over", async () => {
    const review = stubReview();
    const { container, pane } = await mount({ review });
    expect(container.querySelector(".cdetail-host").textContent).toContain("aggregate");

    for (const round of [1, 2]) {
      await click(container.querySelector('.rrow[data-sel="uncommitted"]'));
      const detail = container.querySelector(".cdetail-host");
      expect(detail.textContent, `round ${round}`).not.toContain("aggregate");
      expect(detail.querySelector('.file[data-key$=":src/a.js"]'), `round ${round}`).toBeTruthy();
      expect(detail.querySelector(".csheader"), `round ${round}`).toBeTruthy();
      await click(container.querySelector('.rrow[data-sel="review"]'));
    }
    expect(review.calls).toEqual(["mount", "unmount", "mount", "unmount", "mount"]);
    pane.dispose();
  });
});

// The rail is reconciled row by row, not rewritten: a pass that moved only the
// diff leaves every rail row where it stood, and a commit that lands inserts one
// row without touching the rows already there.
describe("the rail's paint", () => {
  /** Everything the DOM under `target` did while `act` ran. */
  const churn = async (target, act) => {
    const seen = [];
    const observer = new MutationObserver((records) => seen.push(...records));
    observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
    await act();
    seen.push(...observer.takeRecords());
    observer.disconnect();
    return seen;
  };

  /** Mount over a channel whose status and log the test can move underneath it. */
  const mountLive = async (served) => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return served.status;
      if (method === "git.diff") return tree.diff(params);
      if (method === "git.log") return served.log;
      if (method === "git.show") return show();
      return {};
    });
    const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
    await settle();
    return { container, pane };
  };

  it("leaves every rail row alone when the pass moved only the diff", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const served = { status: dirtyStatus(), log: log() };
      const { container, pane } = await mountLive(served);
      const rail = container.querySelector(".crail");
      const rows = [...rail.children];
      const records = await churn(rail, async () => {
        tree.write("src/a.js", "the agent moved on");
        served.status = dirtyStatus({ head: "e".repeat(40) });
        await vi.advanceTimersByTimeAsync(2000);
        await settle();
      });
      expect(container.textContent).toContain("the agent moved on"); // the pane did repaint
      expect(records).toEqual([]);
      expect([...rail.children]).toEqual(rows);
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("inserts the commit that landed without touching the rows already there", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const served = { status: dirtyStatus(), log: log() };
      const { container, pane } = await mountLive(served);
      const rail = container.querySelector(".crail");
      const uncommitted = rail.querySelector('.rrow[data-sel="uncommitted"]');
      const earlier = rail.querySelector(`.crow[data-hash="${"a".repeat(40)}"]`);
      const landed = { hash: "b".repeat(40), short: "bbbbbbb", subject: "just landed", author: "Zech", email: "z@x", time: 2 };
      const records = await churn(rail, async () => {
        served.log = { ...log(), commits: [landed, ...log().commits] };
        await vi.advanceTimersByTimeAsync(2000);
        await settle();
      });
      const fresh = rail.querySelector(`.crow[data-hash="${"b".repeat(40)}"]`);
      expect(fresh.textContent).toContain("just landed");
      expect(rail.querySelector('.rrow[data-sel="uncommitted"]')).toBe(uncommitted);
      expect(rail.querySelector(`.crow[data-hash="${"a".repeat(40)}"]`)).toBe(earlier);
      // The one row that arrived is the only thing the rail did.
      expect(records.map((record) => record.type)).toEqual(["childList"]);
      expect([...records[0].addedNodes]).toEqual([fresh]);
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("one renderer for every changeset", () => {
  it("stacks the uncommitted files as full diffs with a ✎ per file — and no stage checkbox", async () => {
    const { container, pane } = await mount();
    const file = container.querySelector('.file[data-key$=":src/a.js"]');
    expect(file).toBeTruthy();
    expect(file.querySelector("td.ln")).toBeTruthy();
    expect(file.querySelector(".fcmt")).toBeTruthy();
    expect(container.querySelector(".stagebox")).toBe(null);
    pane.dispose();
  });

  it("renders a selected commit through the same stack, ✎ and all", async () => {
    const { container, pane } = await mount();
    await click(container.querySelector(".crow"));
    const file = container.querySelector('.file[data-key$=":src/b.js"]');
    expect(file).toBeTruthy();
    expect(file.querySelector(".fcmt")).toBeTruthy();
    expect(container.querySelector(".cdetail-host").textContent).toContain("why it happened");
    pane.dispose();
  });
});

describe("noise is collapsed, never hidden", () => {
  it("groups generated files under one count line and opens on request", async () => {
    const { container, pane } = await mount();
    expect(container.querySelector('.file[data-key$=":uv.lock"]')).toBe(null);
    const head = container.querySelector(".noisehead");
    expect(head.textContent).toContain("1 generated file");
    await click(head);
    expect(container.querySelector('.file[data-key$=":uv.lock"]')).toBeTruthy();
    pane.dispose();
  });
});

describe("commit is commit-all", () => {
  it("stages every changed path, then commits the message", async () => {
    const { container, pane, calls } = await mount();
    container.querySelector(".gitmsg").value = "a real message";
    await click(container.querySelector(".gitcommit-actions .btn.primary"));
    const stage = calls.find((c) => c.method === "git.stage");
    expect(stage.params.paths).toEqual(["src/a.js", "uv.lock"]);
    const commit = calls.find((c) => c.method === "git.commit");
    expect(commit.params.message).toBe("a real message");
    expect(calls.findIndex((c) => c.method === "git.stage")).toBeLessThan(calls.findIndex((c) => c.method === "git.commit"));
    pane.dispose();
  });

  it("refuses an empty message without touching the index", async () => {
    const { container, pane, calls } = await mount();
    await click(container.querySelector(".gitcommit-actions .btn.primary"));
    expect(calls.some((c) => c.method === "git.stage")).toBe(false);
    expect(container.textContent).toContain("Enter a commit message");
    pane.dispose();
  });
});

describe("per-file discard lives behind the header ⋯", () => {
  it("takes two clicks to fire, the second one carrying the path", async () => {
    const { container, pane, calls } = await mount();
    const file = container.querySelector('.file[data-key$=":src/a.js"]');
    expect(file.querySelector(".gitdiscard")).toBe(null); // shut until asked for
    await click(file.querySelector(".fmenu"));
    const discard = container.querySelector('.file[data-key$=":src/a.js"] .gitdiscard');
    expect(discard).toBeTruthy();
    await click(discard);
    expect(calls.some((c) => c.method === "git.discard")).toBe(false); // armed only
    const armed = container.querySelector('.file[data-key$=":src/a.js"] .gitdiscard');
    expect(armed.textContent).toContain("Discard changes?");
    await click(armed);
    const call = calls.find((c) => c.method === "git.discard");
    expect(call.params.paths).toEqual(["src/a.js"]);
    pane.dispose();
  });
});

describe("comments on any changeset", () => {
  /** Comment through the popover. A press opens the field itself, so there is
   *  no intermediate button to get past. */
  const addCommentViaPop = async (trigger) => {
    await click(trigger);
    document.querySelector(".cp-input").value = "rename this";
    await click(document.querySelector(".cp-save"));
  };

  it("sends a whole-file comment as an anchored post naming the file and no line", async () => {
    const { container, pane, calls } = await mount();
    await addCommentViaPop(container.querySelector('.file[data-key$=":src/a.js"] .fcmt'));
    expect(container.querySelector(".pcomment").textContent).toContain("rename this");
    await click(container.querySelector(".cssend"));
    const post = calls.find((c) => c.method === "run.request_changes");
    expect(post.params.run_id).toBe("run-1");
    expect(post.params.messages[0].body).toBe("rename this");
    expect(post.params.messages[0].anchor).toMatchObject({ artifact: "diff", path: "src/a.js" });
    // The whole file is not a line: the anchor carries no range at all.
    expect(post.params.messages[0].anchor.line_start).toBeUndefined();
    expect(post.params.messages[0].anchor.line_end).toBeUndefined();
    // sent comments leave the tray
    expect(container.querySelector(".pcomment")).toBe(null);
    pane.dispose();
  });

  it("comments a line once the file is expanded, anchoring to that line", async () => {
    const { container, pane, calls } = await mount();
    const file = container.querySelector('.file[data-key$=":src/a.js"]');
    file.classList.remove("capped");
    await addCommentViaPop(file.querySelector('tr[data-ln="1"] td.code'));
    await click(container.querySelector(".cssend"));
    const post = calls.find((c) => c.method === "run.request_changes");
    expect(post.params.messages[0].anchor).toMatchObject({ path: "src/a.js", line_start: 1, line_end: 1 });
    pane.dispose();
  });

  it("offers no ✎ where there is no agent to send to", async () => {
    const { container, pane } = await mount({ scope: { project_id: "p1" } });
    expect(container.querySelector(".fcmt")).toBe(null);
    expect(container.querySelector(".csgeneral")).toBe(null);
    pane.dispose();
  });
});

describe("the poll freeze holds a review in progress", () => {
  it("leaves a pending comment (and its tray) alone when the diff moves underneath", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let served = dirtyStatus();
      const container = document.createElement("div");
      document.body.appendChild(container);
      const callRpc = vi.fn(async (method, params) => {
        if (method === "git.status") return served;
        if (method === "git.diff") return tree.diff(params);
        if (method === "git.log") return log();
        if (method === "run.request_changes") return { ok: true };
        return {};
      });
      const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
      await settle();
      await click(container.querySelector('.file[data-key$=":src/a.js"] .fcmt'));
      document.querySelector(".cp-input").value = "hold this thought";
      await click(document.querySelector(".cp-save"));
      expect(container.querySelector(".pcomment")).toBeTruthy();

      // The agent commits underneath the reviewer: the poll must not rebuild
      // the changeset out from under the pending comment.
      tree.write("src/a.js", "the agent moved on");
      served = dirtyStatus({ head: "e".repeat(40) });
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
      expect(container.querySelector(".pcomment").textContent).toContain("hold this thought");
      expect(container.textContent).not.toContain("the agent moved on");
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("the poll freeze holds an open menu", () => {
  // The pane's own menus (the branch list, a file's ⋯) already hold the poll
  // off. A split button's menu is the same kind of thing: it is open because
  // somebody is reaching into it.
  it("leaves the commit menu open while the tree moves underneath", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let served = dirtyStatus();
      const container = document.createElement("div");
      document.body.appendChild(container);
      const callRpc = vi.fn(async (method, params) => {
        if (method === "git.status") return served;
        if (method === "git.diff") return tree.diff(params);
        if (method === "git.log") return log();
        return {};
      });
      const pane = mountGitPane(container, {
        scope: { run_id: "run-1" },
        callRpc,
        agentCommitOptions: taskAgentCommitOptions("building", "ship it"),
      });
      await settle();

      await click(container.querySelector(".gitcommit-actions .caret"));
      const menu = container.querySelector(".gitcommit-actions .splitmenu");
      expect(menu.hidden).toBe(false);

      served = dirtyStatus({ head: "e".repeat(40) });
      await vi.advanceTimersByTimeAsync(2000);
      await settle();

      expect(container.querySelector(".gitcommit-actions .splitmenu"), "the poll replaced the menu").toBe(menu);
      expect(menu.hidden).toBe(false);
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("re-review memory on every stack", () => {
  it("marks the files that moved since comments went out on THIS changeset", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let served = dirtyStatus();
      const container = document.createElement("div");
      document.body.appendChild(container);
      const callRpc = vi.fn(async (method, params) => {
        if (method === "git.status") return served;
        if (method === "git.diff") return tree.diff(params);
        if (method === "git.log") return log();
        if (method === "git.show") return show();
        if (method === "run.request_changes") return { ok: true };
        return {};
      });
      const pane = mountGitPane(container, { scope: { run_id: "run-1" }, callRpc });
      await settle();
      await click(container.querySelector('.file[data-key$=":src/a.js"] .fcmt'));
      document.querySelector(".cp-input").value = "rename this";
      await click(document.querySelector(".cp-save"));
      await click(container.querySelector(".cssend"));
      expect(container.querySelector(".fchanged")).toBe(null); // nothing has moved yet

      tree.write("src/a.js", "the agent moved on");
      served = dirtyStatus();
      await vi.advanceTimersByTimeAsync(2000);
      await settle();
      const changed = container.querySelector('.file[data-key$=":src/a.js"] .fchanged');
      expect(changed.textContent).toContain("changed since your review");

      // The stamp belongs to the changeset it was taken on: a commit's stack
      // has never been reviewed, so nothing in it is flagged.
      await click(container.querySelector(".crow[data-hash]"));
      await settle();
      expect(container.querySelector(".cdetail-host .fchanged")).toBe(null);
      pane.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});
