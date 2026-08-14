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
});
