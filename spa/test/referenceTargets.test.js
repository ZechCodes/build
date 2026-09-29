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

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";
import { threadHtml } from "../src/core/thread.js";
import { taskPageHtml } from "../src/core/trackerTaskRender.js";

const place = { deviceId: "dev-1", projectId: "proj-1", projectKey: "dev-1/proj-1", projectName: "Build" };
const tasks = [{ id: "task-01M2A", number: 42, title: "Tasks list shows open tasks by default" }];
const workspaces = [{ id: "ws-1", workspace_id: "ws-1", name: "tasks-spa", projectKey: "dev-1/proj-1" }];

const hostOf = (html) => {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
};
const anchors = (html) => [...hostOf(html).querySelectorAll("a")].map((a) => ({ href: a.getAttribute("href"), text: a.textContent, title: a.getAttribute("title") }));

describe("what a reference names", () => {
  it("leaves a deleted project agent's prose reference without a destination", () => {
    const identities = { "project-01M2SCB": { agent_id: "project-01M2SCB", available: false } };
    holdReferenceSources({ feed: { projects: [], workspaces, items: [] }, tasks: {} });
    const html = markdownHtml("Ask @agent:project-01M2SCB.", { place, identities });
    holdReferenceSources({});
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).textContent).toContain("@agent:project-01M2SCB");
  });
});

// Every form, through the surfaces — which since #229 all ask the one index
// (core/referenceIndex.js) rather than a resolver each was handed.
const FORMS = [
  { name: "a task", wrote: "Rolled #42 this morning.", reads: "#42 Tasks list shows open tasks by default" },
  { name: "a workspace", wrote: "Cut on @workspace:tasks-spa.", reads: "tasks-spa" },
  { name: "an agent", wrote: "Handed to @agent:agent-01M2A.", reads: "tasks-spa · Agent 1" },
  { name: "a file", wrote: "See [[tasks-spa:spa/src/core/thread.js#L42]].", reads: "spa/src/core/thread.js:42" },
];

const indexed = () => holdReferenceSources({
  feed: {
    projects: [{ id: "proj-1", deviceId: "dev-1", projectKey: "dev-1/proj-1", name: "Build" }],
    workspaces,
    items: [{ projectKey: "dev-1/proj-1", entity_id: "ws-1", agents: [{ id: "agent-01M2A" }] }],
  },
  tasks: { "dev-1/proj-1": tasks },
});

describe("a reference in a chat message", () => {
  beforeEach(indexed);
  afterEach(() => holdReferenceSources({}));
  const said = (body) => threadHtml(
    { id: "c-1", items: [{ type: "message", data: { id: "m-1", sequence: 1, role: "agent", body } }] },
    { place },
  );

  for (const { name, wrote, reads } of FORMS) {
    it(`links ${name}`, () => {
      const [anchor] = anchors(said(wrote));
      expect(anchor?.text).toBe(reads);
      expect(anchor?.href).toMatch(/^#\//);
    });
  }

  it("marks a reference the index looked for and did not find, as the words that were typed", () => {
    const html = said("Try @workspace:no-such-workspace and #9999.");
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).textContent).toContain("@workspace:no-such-workspace");
    expect(hostOf(html).textContent).toContain("#9999");
    expect(hostOf(html).querySelectorAll(".md-ref-missing")).toHaveLength(2);
  });

  // A message explaining the syntax is mostly examples.
  it("leaves a reference inside a code span literal", () => {
    const html = said("Write `#42` to point at a task.");
    expect(anchors(html)).toEqual([]);
    expect(hostOf(html).querySelector("code").textContent).toBe("#42");
  });

  // #229: the screenshot. A Skrift agent's message named a workspace of its
  // own project; it did resolve, but read as its own brackets in body colour.
  it("labels a commit by its workspace and short SHA, the brackets on the hover", () => {
    const [anchor] = anchors(said("Ready at [[tasks-spa:commit:c9875571]], but held."));
    expect(anchor.text).toBe("tasks-spa · c9875571");
    expect(anchor.title).toContain("[[tasks-spa:commit:c9875571]]");
  });

  it("links a workspace of another project the account holds", () => {
    holdReferenceSources({
      feed: { projects: [], items: [], workspaces: [{ id: "ws-9", workspace_id: "ws-9", name: "skrift-0-2-1-validation", projectKey: "dev-1/proj-2" }] },
      tasks: {},
    });
    const [anchor] = anchors(said("See [[skrift-0-2-1-validation:commit:c9875571]]."));
    expect(anchor?.href).toContain("/project/proj-2/workspace/ws-9/");
  });
});

describe("a reference in a comment on the task page", () => {
  beforeEach(indexed);
  afterEach(() => holdReferenceSources({}));
  const task = { id: "task-1", number: 63, title: "A task", state: "open", status: "in_progress", labels: [], links: {} };
  const page = (body) => taskPageHtml(task, {
    columns: [], rows: [{ type: "comment", key: "c1", actor: { kind: "user" }, body, at: "2026-09-21T00:00:00Z" }],
    links: [], draft: "", labelsDraft: "", busy: false, sending: false,
    agentLabels: {}, projectName: "Build", deviceId: "dev-1", projectId: "proj-1",
  });

  for (const { name, wrote, reads } of FORMS) {
    it(`links ${name}`, () => {
      const [anchor] = anchors(page(wrote));
      expect(anchor?.text).toBe(reads);
      expect(anchor?.href).toMatch(/^#\//);
    });
  }

  it("leaves an unknown workspace unlinked", () => {
    expect(anchors(page("On @workspace:no-such-workspace."))).toEqual([]);
  });

  it("leaves a reference inside a code span literal", () => {
    expect(anchors(page("Write `@agent:agent-01M2A` to point at one."))).toEqual([]);
  });

  // A task carries every actor on it; an agent the feed has lost is still
  // named by the task's own identities.
  it("resolves an agent through the task's identities", () => {
    const html = taskPageHtml(task, {
      columns: [], rows: [{ type: "comment", key: "c1", actor: { kind: "user" }, body: "Ask @agent:agent-XYZ.", at: "2026-09-21T00:00:00Z" }],
      links: [], draft: "", labelsDraft: "", busy: false, sending: false,
      agentLabels: {}, projectName: "Build", deviceId: "dev-1", projectId: "proj-1",
      identities: { "agent-XYZ": { agent_id: "agent-XYZ", available: true, workspace_id: "ws-1", name: "Quill" } },
    });
    expect(anchors(html)[0]?.text).toBe("Quill");
  });
});

describe("the task's own body", () => {
  beforeEach(indexed);
  afterEach(() => holdReferenceSources({}));
  const task = { id: "task-1", number: 63, title: "A task", state: "open", status: "in_progress", labels: [], links: {}, body: "Follows #42." };
  it("links what it names", () => {
    const html = taskPageHtml(task, {
      columns: [], rows: [], links: [], draft: "", labelsDraft: "", busy: false, sending: false,
      agentLabels: {}, projectName: "Build", deviceId: "dev-1", projectId: "proj-1",
    });
    expect(anchors(html)[0]?.text).toBe("#42 Tasks list shows open tasks by default");
  });
});

// Before the index has read anything, every form is the words that were typed.
describe("the renderer before the index is filled", () => {
  it("leaves every form as prose", () => {
    holdReferenceSources({});
    for (const { wrote } of FORMS) expect(markdownHtml(wrote, { place })).not.toContain("<a ");
  });
});
