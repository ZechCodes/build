/** @vitest-environment jsdom */
// #63.2: the references an agent writes become links, in chat and on the task
// page.
//
// #56 landed the syntax and the routing with the resolver left injectable and
// no caller passing one, so every reference rendered as the words that were
// typed. This wires the two ends together: the resolver is built from the
// caches each surface already paints from (core/referenceTargets.js), and the
// conversation and the task page hand it to the renderer.
//
// The behaviour that must survive: a reference nothing answers for stays
// prose, and a reference inside a code span stays literal. A message that
// EXPLAINS this syntax is mostly examples, and they have to read as what an
// agent should type.

import { describe, expect, it } from "vitest";
import { referenceLinks } from "../src/core/referenceTargets.js";
import { renderMarkdown } from "../src/core/markdown.js";
import { threadHtml } from "../src/core/thread.js";
import { taskPageHtml } from "../src/core/trackerTaskRender.js";

const place = { deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1/proj-1", projectName: "Build" };
const tasks = [{ id: "task-01M2A", number: 42, title: "Tasks list shows open tasks by default" }];
const workspaces = [{ id: "ws-1", workspace_id: "ws-1", name: "tasks-spa", projectKey: "dev-1/proj-1" }];
const agentGroups = [{ workspaceId: "ws-1", name: "tasks-spa", agents: [{ id: "agent-01M2A", label: "tasks-spa · Agent 1" }] }];
const links = () => referenceLinks({ place, tasks, workspaces, agentGroups });

const hostOf = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const anchors = (html) => [...hostOf(html).querySelectorAll("a")].map((a) => ({ href: a.getAttribute("href"), text: a.textContent, title: a.getAttribute("title") }));

describe("what a reference names", () => {
  it("answers nothing at all without a place to route from", () => {
    expect(referenceLinks({ place: { projectId: "proj-1" }, tasks })).toBeNull();
  });

  it("finds a task by its number", () => {
    expect(links().task(42)).toEqual({ deviceId: "dev-1", projectId: "proj-1", taskId: "task-01M2A", title: tasks[0].title });
    expect(links().task(9999)).toBeNull();
  });

  it("finds a workspace by the name a reader writes, or by its id", () => {
    expect(links().workspace("tasks-spa")?.workspaceId).toBe("ws-1");
    expect(links().workspace("TASKS-SPA")?.workspaceId).toBe("ws-1");
    expect(links().workspace("ws-1")?.workspaceId).toBe("ws-1");
    expect(links().workspace("no-such-workspace")).toBeNull();
  });

  it("finds an agent in the workspace it stands in", () => {
    expect(links().agent("agent-01M2A")).toMatchObject({ workspaceId: "ws-1", agentId: "agent-01M2A" });
    expect(links().agent("agent-NOPE")).toBeNull();
  });

  // It stands in no workspace, so it is reached on the project's own page —
  // and it is never in the picker's groups, which is why it is asked about by
  // its id rather than looked up.
  it("knows the project's own agent without a workspace", () => {
    expect(links().agent("project-01M2SCB")).toEqual({ deviceId: "dev-1", projectId: "proj-1", agentId: "project-01M2SCB" });
  });

  it("leaves a deleted project agent's prose reference without a destination", () => {
    const refLinks = referenceLinks({ place, identities: {
      "project-01M2SCB": { agent_id: "project-01M2SCB", available: false },
    } });
    const html = threadHtml(
      { id: "c-1", items: [{ type: "message", data: { id: "m-1", sequence: 1,
        role: "agent", body: "Ask @agent:project-01M2SCB." } }] },
      { place, refLinks },
    );
    expect(refLinks.agent("project-01M2SCB")).toBeNull();
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).textContent).toContain("@agent:project-01M2SCB");
  });
});

// Every form, through the renderer the surfaces call.
const FORMS = [
  { name: "a task", wrote: "Rolled #42 this morning.", reads: "#42" },
  { name: "a workspace", wrote: "Cut on @workspace:tasks-spa.", reads: "@workspace:tasks-spa" },
  { name: "an agent", wrote: "Handed to @agent:agent-01M2A.", reads: "@agent:agent-01M2A" },
  { name: "a file", wrote: "See [[tasks-spa:spa/src/core/thread.js#L42]].", reads: "[[tasks-spa:spa/src/core/thread.js#L42]]" },
];

describe("a reference in a chat message", () => {
  const said = (body) => threadHtml(
    { id: "c-1", items: [{ type: "message", data: { id: "m-1", sequence: 1, role: "agent", body } }] },
    { place, refLinks: links() },
  );

  for (const { name, wrote, reads } of FORMS) {
    it(`links ${name}`, () => {
      const [anchor] = anchors(said(wrote));
      expect(anchor?.text).toBe(reads);
      expect(anchor?.href).toMatch(/^#\//);
    });
  }

  it("leaves a reference nothing answers for as the words that were typed", () => {
    const html = said("Try @workspace:no-such-workspace and #9999.");
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).textContent).toContain("@workspace:no-such-workspace");
    expect(hostOf(html).textContent).toContain("#9999");
  });

  // A message explaining the syntax is mostly examples.
  it("leaves a reference inside a code span literal", () => {
    const html = said("Write `#42` to point at a task.");
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).querySelector("code").textContent).toBe("#42");
  });

  it("expands nothing at all when the surface passes no resolver", () => {
    const html = threadHtml(
      { id: "c-1", items: [{ type: "message", data: { id: "m-1", sequence: 1, role: "agent", body: "Rolled #42." } }] },
      { place },
    );
    expect(anchors(html)).toEqual([]);
  });
});

describe("a reference in a comment on the task page", () => {
  const task = { id: "task-1", number: 63, title: "A task", state: "open", status: "in_progress", labels: [], links: {} };
  const page = (body) => taskPageHtml(task, {
    columns: [], rows: [{ type: "comment", key: "c1", actor: { kind: "user" }, body, at: "2026-09-21T00:00:00Z" }],
    links: [], draft: "", labelsDraft: "", busy: false, sending: false,
    agentLabels: {}, projectName: "Build", refLinks: links(),
  });

  for (const { name, wrote, reads } of FORMS) {
    it(`links ${name}`, () => {
      const [anchor] = anchors(page(wrote));
      expect(anchor?.text).toBe(reads);
      expect(anchor?.href).toMatch(/^#\//);
    });
  }

  it("leaves an unknown workspace as text", () => {
    expect(anchors(page("On @workspace:no-such-workspace."))).toEqual([]);
  });

  it("leaves a reference inside a code span literal", () => {
    expect(anchors(page("Write `@agent:agent-01M2A` to point at one."))).toEqual([]);
  });
});

describe("the task's own body", () => {
  const task = { id: "task-1", number: 63, title: "A task", state: "open", status: "in_progress", labels: [], links: {}, body: "Follows #42." };
  it("links what it names", () => {
    const html = taskPageHtml(task, {
      columns: [], rows: [], links: [], draft: "", labelsDraft: "", busy: false, sending: false,
      agentLabels: {}, projectName: "Build", refLinks: links(),
    });
    expect(anchors(html)[0]?.text).toBe("#42");
  });
});

// The renderer's own contract, unchanged: no resolver, no expansion.
describe("the renderer without a resolver", () => {
  it("leaves every form as prose", () => {
    for (const { wrote } of FORMS) expect(renderMarkdown(wrote)).not.toContain("<a ");
  });
});
