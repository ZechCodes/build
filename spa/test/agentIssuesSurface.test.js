/** @vitest-environment jsdom */
// The agent's issues as one of its surfaces (#34): the pill, its count, and
// the fold that puts what the agent is finished with out of the way.
//
// Zech, on the entry as it rolled: "Right now the in review issues on an agent
// are just noise taking up space on the chat. I had envisioned the issues
// activity to be the same UX as agents/workflows/tasks/shells. So it's only
// visible when the user wants it to be and there's a clear pattern for done
// work."
//
// So the test is not "does it draw the right rows" — the surfaces layer draws
// them — it is "is this the same surface the other four are". Everything below
// asks that.

import { describe, expect, it } from "vitest";
import {
  ISSUES_ENTRY_KIND,
  SURFACE_KINDS,
  runningAndCompletedRows,
  surfaceKindLabel,
  surfaceMenuOptions,
  surfacePills,
  surfaceRows,
  surfaceStateMark,
} from "../src/core/agentSurfacesModel.js";
import { mountSurfaceViewer } from "../src/core/agentSurfaces.js";
import { agentIssueEntries } from "../src/core/trackerAgentIssues.js";
import { issue } from "./trackerWireFixture.js";

const ME = "agent-01M2ME";
const THEM = "agent-01M2THEM";
const mine = (over = {}) => issue({ assignee: { kind: "agent", agent_id: ME }, ...over });

/** A project list with one issue in each standing an agent can be in. */
const EVERY_STANDING = [
  mine({ number: 1, id: "i1", title: "Working on it", status: "in_progress" }),
  mine({ number: 2, id: "i2", title: "Not started", status: "ready" }),
  issue({ number: 3, id: "i3", title: "Following it", assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
  mine({ number: 4, id: "i4", title: "Handed back", status: "in_review" }),
  mine({ number: 5, id: "i5", title: "Finished", status: "done" }),
  mine({ number: 6, id: "i6", title: "Shut", status: "backlog", state: "closed" }),
];

const surfacesWith = (issues, agentId = ME) => ({ [ISSUES_ENTRY_KIND]: agentIssueEntries(issues, agentId) });

describe("issues are a surface kind like the others", () => {
  it("is one of the kinds, and is called Issues", () => {
    expect(SURFACE_KINDS).toContain(ISSUES_ENTRY_KIND);
    expect(surfaceKindLabel(ISSUES_ENTRY_KIND)).toBe("Issues");
  });

  it("puts a pill up, counting what the agent is actually working on", () => {
    const [pill] = surfacePills(surfacesWith(EVERY_STANDING));
    expect(pill.kind).toBe(ISSUES_ENTRY_KIND);
    expect(pill.label).toBe("Issues");
    // One in progress. Assigned, tracked and the three it is finished with are
    // in the surface, not on the pill — a count is a call to look.
    expect(pill.count).toBe(1);
  });

  // The pill is absent when the agent holds and tracks nothing, which is what
  // keeps it off a conversation that has nothing to do with issues.
  it("puts no pill up for an agent with nothing", () => {
    expect(surfacePills({ [ISSUES_ENTRY_KIND]: [] })).toEqual([]);
    expect(surfacePills({})).toEqual([]);
  });

  // An agent is not "running" an issue, it is working on one.
  it("says what it is doing in the ⋮ menu, in its own words", () => {
    const [option] = surfaceMenuOptions(surfacesWith(EVERY_STANDING));
    expect(option).toEqual({ id: ISSUES_ENTRY_KIND, label: "Issues", description: "1 in progress" });
  });
});

describe("what sits above the fold and what sits under it", () => {
  const rows = () => surfaceRows(ISSUES_ENTRY_KIND, surfacesWith(EVERY_STANDING));

  it("reads in progress, then assigned, then tracked", () => {
    const { running } = runningAndCompletedRows(rows());
    expect(running.map((row) => row.state)).toEqual(["in_progress", "assigned", "tracked"]);
  });

  // The whole of the complaint. In review is finished work FOR THE AGENT: it
  // has said the work is ready to be looked at and has nothing more to do. The
  // issue is still open, and the Issues tab still lists it as open.
  it("folds In review together with Done and Closed", () => {
    const { completed } = runningAndCompletedRows(rows());
    expect(completed.map((row) => row.state)).toEqual(["in_review", "done", "closed"]);
  });

  it("marks the three folded states as finished and nothing else", () => {
    for (const state of ["in_review", "done", "closed"]) {
      expect(surfaceStateMark(ISSUES_ENTRY_KIND, state).mark).toBe("ok");
    }
    expect(surfaceStateMark(ISSUES_ENTRY_KIND, "in_progress").mark).toBe("running");
    expect(surfaceStateMark(ISSUES_ENTRY_KIND, "assigned").mark).toBe("pending");
    expect(surfaceStateMark(ISSUES_ENTRY_KIND, "tracked").mark).toBe("pending");
  });

  it("says an issue the way a person says it out loud", () => {
    const [first] = rows();
    expect(first.subject).toBe("#1 Working on it");
  });
});

describe("the viewer it opens", () => {
  const mount = () => {
    document.body.innerHTML = `<div class="surface-host"></div>`;
    const host = document.querySelector(".surface-host");
    const viewer = mountSurfaceViewer(host, ISSUES_ENTRY_KIND, { onOpenThreadItem: () => {} });
    viewer.set(surfacesWith(EVERY_STANDING));
    return { host, viewer };
  };

  const labels = (host, selector) =>
    [...host.querySelectorAll(`${selector} .surface-row-label`)].map((one) => one.textContent.trim());

  it("draws the open work, with the finished work behind one fold", () => {
    const { host, viewer } = mount();
    expect(labels(host, ".surface-running")).toEqual(["#1 Working on it", "#2 Not started", "#3 Following it"]);
    expect(labels(host, ".surface-completed")).toEqual(["#4 Handed back", "#5 Finished", "#6 Shut"]);
    viewer.dispose();
  });

  it("comes up with the fold shut, under one count", () => {
    const { host, viewer } = mount();
    const fold = host.querySelector("details");
    expect(fold.open).toBe(false);
    expect(fold.querySelector("summary").textContent).toContain("3");
    viewer.dispose();
  });

  // The mark beside a row is a colour, and four of the six states share two
  // colours between them, so each row says its standing in words too.
  it("says each row's standing in words", () => {
    const { host, viewer } = mount();
    const stats = [...host.querySelectorAll(".surface-row-stat")].map((one) => one.textContent.trim());
    expect(stats).toEqual(["In progress", "Assigned", "Tracking", "In review", "Done", "Closed"]);
    viewer.dispose();
  });
});
