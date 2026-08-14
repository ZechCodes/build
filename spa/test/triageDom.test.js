// @vitest-environment jsdom
// The triage overlay where a reviewer meets it: inside the Changes pane's
// changeset stack. Criticals first, low work folded into named groups that open
// on a click, the untriaged label when there is no pass, and the trust dial —
// per project, remembered — that turns the whole reading off.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { mountGitPane, GIT_PANE_POLL_MS } from "../src/core/gitPane.js";
import { patchHunks } from "../src/core/diff.js";
import { createReviewPlug } from "../src/core/changesReview.js";

const patchFor = (path, line) =>
  `diff --git a/${path} b/${path}\nindex 1111111..2222222 100644\n--- a/${path}\n+++ b/${path}\n@@ -1,2 +1,2 @@\n-old\n+${line}\n context\n`;

const DIRTY_PATCH = patchFor("src/crypto.rs", "seal(key)") + patchFor("Cargo.toml", 'v = "2"');
const ids = Object.fromEntries(patchHunks(DIRTY_PATCH).map((hunk) => [hunk.path, hunk.hunk_id]));

const TRIAGE = {
  based_on: "rev-1",
  stale: false,
  overrides: [],
  hunks: [
    { hunk_id: ids["src/crypto.rs"], level: "critical", rationale: "changes how the key is sealed" },
    { hunk_id: ids["Cargo.toml"], level: "low", group: "Version bumps", rationale: "a version string" },
  ],
};

const dirtyStatus = () => ({
  branch: "build/x",
  path: "/repo",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/build/x",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [
    { path: "src/crypto.rs", staged: "none", index_status: "M", worktree_status: "M" },
    { path: "Cargo.toml", staged: "none", index_status: "M", worktree_status: "M" },
  ],
  files_truncated: false,
  stat: { files_changed: 2, insertions: 2, deletions: 2 },
  patch: DIRTY_PATCH,
  truncated: false,
});

const log = () => ({ branch: "build/x", commits: [], more: false });

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};

async function mount({ triage = TRIAGE, projectId = "proj-1" } = {}) {
  const callRpc = vi.fn(async (method) => {
    if (method === "git.status") return dirtyStatus();
    if (method === "git.log") return log();
    return {};
  });
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, {
    scope: { run_id: "run-1" },
    projectId,
    triage: () => triage,
    callRpc,
  });
  await settle();
  return { container, pane };
}

let mounted = [];

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  localStorage.clear();
});

afterEach(() => {
  for (const pane of mounted) pane.dispose();
  mounted = [];
  document.body.innerHTML = "";
  vi.useRealTimers();
  localStorage.clear();
});

const open = async (options) => {
  const { container, pane } = await mount(options);
  mounted.push(pane);
  return container;
};

describe("the triage overlay in the Changes pane", () => {
  it("surfaces the critical file above the collapsed group", async () => {
    const container = await open();
    const critical = container.querySelector(".tsection.tcritical .file");
    expect(critical.dataset.file).toBe("src/crypto.rs");
    const group = container.querySelector(".tgroup");
    expect(group.querySelector(".tgname").textContent).toBe("Version bumps");
    expect(group.querySelector(".tgcount").textContent).toBe("1 file · 1 hunk");
    expect(group.querySelector(".tgrationale").textContent).toBe("a version string");
    expect(critical.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows the pass's rationale on the critical hunk itself", async () => {
    const container = await open();
    const chip = container.querySelector(".hchip.critical");
    expect(chip.getAttribute("title")).toBe("changes how the key is sealed");
    expect(chip.querySelector(".hrationale").textContent).toBe("changes how the key is sealed");
  });

  it("keeps the collapsed group's diff in the page and opens it on a click", async () => {
    const container = await open();
    expect(container.querySelector('.tgroup .file[data-file="Cargo.toml"]')).toBeTruthy();
    expect(container.querySelector(".tgroup").classList.contains("open")).toBe(false);
    container.querySelector(".tgrouphead").click();
    await settle();
    const group = container.querySelector(".tgroup");
    expect(group.classList.contains("open")).toBe(true);
    expect(group.querySelector(".tgrouphead").getAttribute("aria-expanded")).toBe("true");
    container.querySelector(".tgrouphead").click();
    await settle();
    expect(container.querySelector(".tgroup").classList.contains("open")).toBe(false);
  });

  it("says a changeset is untriaged rather than ordering it silently", async () => {
    const container = await open({ triage: null });
    expect(container.querySelector(".tuntriaged")).toBeTruthy();
    expect(container.querySelector(".tsection.tcritical")).toBeNull();
    expect(container.querySelectorAll(".file").length).toBe(2);
    // Nothing to trust, so nothing to dial.
    expect(container.querySelector(".tdial")).toBeNull();
  });

  it("dials the overlay off into the plain stack, and remembers that per project", async () => {
    const container = await open();
    container.querySelector(".tdial").click();
    await settle();
    expect(container.querySelector(".tsection.tcritical")).toBeNull();
    expect(container.querySelector(".tgroup")).toBeNull();
    expect(container.querySelectorAll(".file").length).toBe(2);
    expect(container.querySelector(".tdial").getAttribute("aria-pressed")).toBe("true");

    const reopened = await open();
    expect(reopened.querySelector(".tsection.tcritical")).toBeNull();
    expect(reopened.querySelector(".tdial").getAttribute("aria-pressed")).toBe("true");

    reopened.querySelector(".tdial").click();
    await settle();
    expect(reopened.querySelector(".tsection.tcritical")).toBeTruthy();
    // Another project is unaffected by this one's dial.
    const elsewhere = await open({ projectId: "proj-2" });
    expect(elsewhere.querySelector(".tsection.tcritical")).toBeTruthy();
  });

  it("re-orders the open stack when a pass lands under it, repo untouched", async () => {
    let pass = null;
    const callRpc = vi.fn(async (method) => {
      if (method === "git.status") return dirtyStatus();
      if (method === "git.log") return log();
      return {};
    });
    const container = document.createElement("div");
    document.body.appendChild(container);
    const pane = mountGitPane(container, {
      scope: { run_id: "run-1" },
      projectId: "proj-1",
      triage: () => pass,
      callRpc,
    });
    mounted.push(pane);
    await settle();
    expect(container.querySelector(".tuntriaged")).toBeTruthy();

    pass = TRIAGE;
    await vi.advanceTimersByTimeAsync(GIT_PANE_POLL_MS + 50);
    await settle();
    expect(container.querySelector(".tsection.tcritical .file").dataset.file).toBe("src/crypto.rs");
  });

  it("leaves a surface with no triage to read exactly as it was", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "git.status") return dirtyStatus();
      if (method === "git.log") return log();
      return {};
    });
    const container = document.createElement("div");
    document.body.appendChild(container);
    const pane = mountGitPane(container, { scope: { project_id: "proj-1" }, callRpc });
    mounted.push(pane);
    await settle();
    expect(container.querySelector(".triagebar")).toBeNull();
    expect(container.querySelectorAll(".file").length).toBe(2);
  });
});

// Every triage decision is overridable, and the reviewer's word is the one the
// surface renders from the moment they give it — not from the next poll.
describe("disagreeing with the pass in the Changes pane", () => {
  /** A pane over a pass the test can move under it, plus what the pane said to
   *  the bridge. `override` decides what `triage.override` answers. */
  const openWithPass = async ({ override = async () => ({}) } = {}) => {
    let pass = TRIAGE;
    const calls = [];
    const callRpc = vi.fn(async (method, params) => {
      if (method === "git.status") return dirtyStatus();
      if (method === "git.log") return log();
      if (method === "triage.override") {
        calls.push(params);
        return override(params);
      }
      return {};
    });
    const container = document.createElement("div");
    document.body.appendChild(container);
    const pane = mountGitPane(container, {
      scope: { run_id: "run-1" },
      projectId: "proj-1",
      triage: () => pass,
      callRpc,
    });
    mounted.push(pane);
    await settle();
    return { container, calls, echo: (updated) => (pass = updated) };
  };

  /** Press an offer and answer its note popover. */
  const disagree = async (control, note = "") => {
    control.click();
    if (note) document.querySelector(".cp-input").value = note;
    document.querySelector(".cp-save").click();
    await settle();
  };

  it("collapses a surfaced critical on the tap, and tells the bridge what was said", async () => {
    const { container, calls } = await openWithPass();
    await disagree(
      container.querySelector('.tsection.tcritical .toverride[data-direction="collapse"]'),
      "a rename, nothing more",
    );

    expect(calls).toEqual([
      {
        run_id: "run-1",
        hunk_id: ids["src/crypto.rs"],
        direction: "collapse",
        note: "a rename, nothing more",
      },
    ]);
    expect(container.querySelector(".tsection.tcritical")).toBeNull();
    const collapsed = container.querySelector('.tgroup .file[data-file="src/crypto.rs"]');
    expect(collapsed).toBeTruthy();
    const chip = collapsed.querySelector(".hchip.overridden");
    expect(chip.textContent).toContain("your call: collapsed");
    expect(chip.textContent).toContain("a rename, nothing more");
    // The offer sits on a hunk row inside a capped file: pressing it must not
    // also read as "expand this file" or "comment on this line".
    expect(container.querySelector(".cslist").children.length).toBe(0);
    expect(collapsed.classList.contains("capped")).toBe(true);
  });

  it("keeps a hunk surfaced from inside the group it was folded into", async () => {
    const { container, calls } = await openWithPass();
    container.querySelector(".tgrouphead").click();
    await settle();
    await disagree(container.querySelector('.tgroup .toverride[data-direction="surface"]'));

    expect(calls).toEqual([
      { run_id: "run-1", hunk_id: ids["Cargo.toml"], direction: "surface", note: "" },
    ]);
    const surfaced = container.querySelector('.tsection.tnormal .file[data-file="Cargo.toml"]');
    expect(surfaced).toBeTruthy();
    expect(surfaced.querySelector(".hchip.overridden").textContent).toContain("your call: surfaced");
    // And the way back is what it now offers.
    expect(surfaced.querySelector(".toverride").dataset.direction).toBe("collapse");
  });

  it("holds the correction through the polls before the pass carries it, and never doubles it", async () => {
    const { container, echo } = await openWithPass();
    await disagree(container.querySelector('.tsection.tcritical .toverride[data-direction="collapse"]'));

    // The pass has not caught up yet: the reviewer's reading must not flicker.
    await vi.advanceTimersByTimeAsync(GIT_PANE_POLL_MS + 50);
    await settle();
    expect(container.querySelector(".tsection.tcritical")).toBeNull();
    expect(container.querySelectorAll(".hchip.overridden").length).toBe(1);

    // Now it does, and the surface renders the disagreement from the pass alone.
    echo({ ...TRIAGE, overrides: [{ hunk_id: ids["src/crypto.rs"], direction: "collapse", note: "" }] });
    await vi.advanceTimersByTimeAsync(GIT_PANE_POLL_MS + 50);
    await settle();
    expect(container.querySelector(".tsection.tcritical")).toBeNull();
    expect(container.querySelectorAll(".hchip.overridden").length).toBe(1);
  });

  it("puts the hunk back when the bridge refuses the correction, and says so", async () => {
    const { container } = await openWithPass({
      override: async () => {
        throw new Error("the pass on run-1 did not classify that hunk");
      },
    });
    await disagree(container.querySelector('.tsection.tcritical .toverride[data-direction="collapse"]'));

    expect(container.querySelector('.tsection.tcritical .file').dataset.file).toBe("src/crypto.rs");
    expect(container.querySelector(".hchip.overridden")).toBeNull();
    expect(document.querySelector("#notices .notice")).toBeTruthy();
  });

  it("offers nothing on a surface with no run to disagree on behalf of", async () => {
    const callRpc = vi.fn(async (method) => {
      if (method === "git.status") return dirtyStatus();
      if (method === "git.log") return log();
      return {};
    });
    const container = document.createElement("div");
    document.body.appendChild(container);
    const pane = mountGitPane(container, { scope: { project_id: "proj-1" }, callRpc });
    mounted.push(pane);
    await settle();
    expect(container.querySelector(".toverride")).toBeNull();
  });
});

describe("the triage overlay on the aggregate review stack", () => {
  const mountPlug = async (payload) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({ fetchDiff: async () => ({ patch: DIRTY_PATCH, ...payload }) });
    plug.mount(host);
    await settle();
    return { host, plug };
  };

  it("orders the run's own diff and remembers the dial for its project", async () => {
    const { host, plug } = await mountPlug({ triage: TRIAGE, projectId: "proj-1" });
    expect(host.querySelector(".tsection.tcritical .file").dataset.file).toBe("src/crypto.rs");
    expect(host.querySelector(".tgname").textContent).toBe("Version bumps");

    host.querySelector(".tgrouphead").click();
    await settle();
    expect(host.querySelector(".tgroup").classList.contains("open")).toBe(true);

    host.querySelector(".tdial").click();
    await settle();
    expect(host.querySelector(".tsection.tcritical")).toBeNull();
    expect(host.querySelectorAll(".file").length).toBe(2);
    plug.unmount();

    const second = await mountPlug({ triage: TRIAGE, projectId: "proj-1" });
    expect(second.host.querySelector(".tdial").getAttribute("aria-pressed")).toBe("true");
    second.plug.unmount();
  });

  it("says untriaged on a run whose diff no pass has read", async () => {
    const { host, plug } = await mountPlug({ triage: null, projectId: "proj-1" });
    expect(host.querySelector(".tuntriaged")).toBeTruthy();
    expect(host.querySelectorAll(".file").length).toBe(2);
    plug.unmount();
  });

  it("leaves a surface that plugs no triage in exactly as it was", async () => {
    const { host, plug } = await mountPlug({});
    expect(host.querySelector(".triagebar")).toBeNull();
    plug.unmount();
  });

  it("posts the reviewer's disagreement and re-orders the stack on the tap", async () => {
    const submitOverride = vi.fn(async () => ({}));
    const host = document.createElement("div");
    document.body.appendChild(host);
    const plug = createReviewPlug({
      fetchDiff: async () => ({ patch: DIRTY_PATCH, triage: TRIAGE, projectId: "proj-1" }),
      submitOverride,
    });
    plug.mount(host);
    await settle();

    host.querySelector('.tsection.tcritical .toverride[data-direction="collapse"]').click();
    document.querySelector(".cp-save").click();
    await settle();

    expect(submitOverride).toHaveBeenCalledWith({
      hunk_id: ids["src/crypto.rs"],
      direction: "collapse",
      note: "",
    });
    expect(host.querySelector(".tsection.tcritical")).toBeNull();
    expect(host.querySelector('.tgroup .file[data-file="src/crypto.rs"]')).toBeTruthy();
    plug.unmount();
  });
});
