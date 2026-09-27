// @vitest-environment jsdom
// The agents overview as an overview of the work on a project (#186): the
// production rail on the Skrift picture (test/agentsOverviewFixture.js), read
// from the real cache with nothing stood in. Each workspace once, ordered by
// what needs the reader, previews in plain words, every action still wired.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { writeCached, wipeCache } = await import("../src/core/localCache.js");
const { stampWorkspace } = await import("../src/core/feedMerge.js");
const { writeIssuesRecord } = await import("../src/core/trackerCache.js");
const fixture = await import("./agentsOverviewFixture.js");

const markup = `<div id="shell"><div id="view"><header id="toolbar">Skrift</header>
  <div id="view-body"><main id="root"></main><aside id="agent-rail" aria-label="Agents"></aside></div>
</div></div>`;

let rail = null;
const host = () => document.querySelector("#agent-rail");
const list = () => host().querySelector(".rail-overview-list");
const sections = () => [...host().querySelectorAll(".rail-overview-section")];
const sectionNames = () => sections().map((section) => section.getAttribute("aria-label"));
const sectionNamed = (name) => sections().find((section) => section.getAttribute("aria-label") === name);
const row = (agentId) => host().querySelector(`[data-overview-agent="${agentId}"]`);
const rowIds = (section) => [...section.querySelectorAll("[data-overview-agent]")].map((node) => node.dataset.overviewAgent);
const navigated = () => new Promise((resolve) => window.addEventListener("hashchange", resolve, { once: true }));

beforeEach(async () => {
  document.body.innerHTML = markup;
  localStorage.clear();
  localStorage.setItem("build.rail.expanded", "1");
  resetAgentRailMemory();
  await wipeCache();
  await fixture.writeAgentsOverviewFixture({ writeCached, stampWorkspace, writeIssuesRecord });
  rail = mountAgentRail(host(), fixture.overviewRailContext());
  await vi.waitFor(() => expect(host().querySelector(".rail-overview-toggle")).toBeTruthy());
  host().querySelector(".rail-overview-toggle").click();
  await vi.waitFor(() => expect(sectionNames()).toHaveLength(5));
});

afterEach(() => {
  rail?.dispose();
  rail = null;
  window.history.replaceState({}, "", "/");
});

describe("the agents overview (#186)", () => {
  it("draws each workspace exactly once, its unwatched agents nested under it with the mark", async () => {
    expect(new Set(sectionNames()).size).toBe(sectionNames().length);
    expect(sectionNames().filter((name) => name === "skrift-review")).toHaveLength(1);
    expect(host().querySelector(".rail-overview-group")).toBeNull();
    expect(host().textContent).not.toContain("NOT WATCHING");
    const review = sectionNamed("skrift-review");
    expect(rowIds(review).sort()).toEqual(["critical-review", "pr-reviewer"]);
    for (const node of review.querySelectorAll("[data-overview-agent]")) {
      expect(node.classList.contains("rail-overview-row-unwatched")).toBe(true);
      expect(node.querySelector('.rail-overview-watch[aria-label="Not watching"]')).toBeTruthy();
    }
    expect(row("fixer").classList.contains("rail-overview-row-unwatched")).toBe(false);
    expect(row("fixer").querySelector(".rail-overview-watch")).toBeNull();
    // The workspace nobody has spoken in is there once, compactly: a heading, no rows.
    const soak = sectionNamed("relay-soak");
    expect(soak.classList.contains("rail-overview-section-empty")).toBe(true);
    expect(rowIds(soak)).toEqual([]);
    expect(soak.querySelector(".rail-overview-none").textContent).toBe("No agents");
  });

  it("orders workspaces by what needs the reader, then what is working, then the quiet, then the empty", async () => {
    expect(sectionNames()).toEqual(["Project agents", "skrift-review", "skrift-fixes", "issue-implementation-audit", "relay-soak"]);
    expect(sections().map((section) => section.dataset.rank)).toEqual(["1", "3", "3", "1", "0"]);
    // Within a workspace the same order: the failed start before the working agent.
    expect(rowIds(sectionNamed("skrift-fixes"))).toEqual(["fix-reviewer", "fixer"]);
    expect(rowIds(sectionNamed("skrift-review"))).toEqual(["critical-review", "pr-reviewer"]);
  });

  it("says each agent's model and state, and what the workspace has going on", () => {
    const shown = (agentId) => ({
      model: row(agentId).querySelector(".rail-overview-model").textContent,
      state: row(agentId).dataset.state,
      word: row(agentId).querySelector(".rail-overview-state").textContent,
    });
    expect(shown("project-agent")).toEqual({ model: "Fable 5.1", state: "idle", word: "Idle" });
    expect(shown("fixer")).toEqual({ model: "Opus 5", state: "working", word: "Working" });
    expect(shown("fix-reviewer")).toEqual({ model: "GPT 6 Sol", state: "error", word: "Failed to start" });
    expect(row("fix-reviewer").querySelector(".rail-overview-state").title).toContain("not a model id");
    expect(shown("pr-reviewer")).toEqual({ model: "GPT 6 Sol", state: "waiting", word: "Unread" });
    expect(shown("critical-review")).toEqual({ model: "GPT 6 Astra", state: "waiting", word: "Blocked" });
    expect(row("critical-review").querySelector(".rail-overview-model").title).toBe("GPT 6 Astra · xhigh");
    expect(shown("auditor")).toEqual({ model: "GPT 6 Sol", state: "idle", word: "Idle" });

    const summary = (name) => sectionNamed(name).querySelector(".rail-overview-sum");
    expect(summary("skrift-fixes").querySelector(".rail-overview-live")).toBeTruthy();
    expect(summary("skrift-fixes").querySelector(".rail-overview-need.is-error").textContent).toBe("!");
    expect(summary("skrift-review").querySelector(".rail-overview-need").textContent).toBe("3");
    expect(summary("skrift-review").querySelector(".rail-overview-live")).toBeNull();
    expect(summary("issue-implementation-audit").children).toHaveLength(0);
  });

  it("names the issue a workspace is for, linked or held, and never a finished one", () => {
    const issue = (name) => sectionNamed(name).querySelector(".rail-overview-issue");
    expect(issue("skrift-fixes").querySelector(".rail-overview-issue-number").textContent).toBe("#183");
    expect(issue("skrift-fixes").querySelector(".rail-overview-issue-title").textContent)
      .toBe("Worker shutdown cancels running jobs on SIGTERM");
    expect(issue("skrift-review").querySelector(".rail-overview-issue-number").textContent).toBe("#1");
    expect(issue("issue-implementation-audit")).toBeNull();
    expect(issue("relay-soak")).toBeNull();
  });

  it("previews a markdown table as one plain line, with no pipes or marks", () => {
    const snippet = row("auditor").querySelector(".rail-overview-snippet").textContent;
    expect(snippet.startsWith("Issue Verdict Evidence / remaining work #183 Done")).toBe(true);
    expect(snippet).not.toMatch(/[|*`[\]]/);
    expect(snippet).not.toContain("\n");
    expect(snippet).toContain("see PR 12");
    expect(row("fixer").querySelector(".rail-overview-snippet").textContent)
      .toBe("Running pytest skrift/tests/test_worker.py -q");
  });

  it("keeps every action wired: the heading, the +, the row and the pin", async () => {
    expect(host().querySelector("#rail-panel .rail-head .pinbtn")).toBeTruthy();
    expect(sectionNamed("Project agents").querySelector(".rail-overview-add")).toBeNull();
    for (const name of ["skrift-fixes", "skrift-review", "issue-implementation-audit", "relay-soak"]) {
      expect(sectionNamed(name).querySelector(`.rail-overview-add[data-overview-add]`).getAttribute("aria-label"))
        .toBe(`Add an agent to ${name}`);
    }

    sectionNamed("skrift-fixes").querySelector(".rail-overview-open").click();
    await vi.waitFor(() => expect(sectionNames()).toEqual(["Project agents", "skrift-fixes"]));
    expect(sectionNamed("skrift-fixes").querySelector(".rail-overview-issue-number").textContent).toBe("#183");
    expect(document.activeElement).toBe(list());
    host().querySelector("#rail-panel .rail-overview-up").click();
    await vi.waitFor(() => expect(sectionNames()).toHaveLength(5));

    let arrived = navigated();
    sectionNamed("relay-soak").querySelector(".rail-overview-add").click();
    await arrived;
    expect(window.location.hash).toContain("/workspace/ws-soak/changes");
    expect(window.location.hash).toContain("newAgent=1");

    arrived = navigated();
    row("pr-reviewer").click();
    await arrived;
    expect(window.location.hash).toContain("/workspace/ws-review/changes");
    expect(window.location.hash).toContain("agent=pr-reviewer");
  });
});
