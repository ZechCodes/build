// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { inboxRowHtml, PROJECT_AGENT, TRACKER_TASK, RECENT_AFTER_MS } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";
import { projectHeadHtml, workspaceProjectBlocks } from "../src/core/inboxProjects.js";

const project = { id: "build", name: "Build", deviceId: "laptop", projectKey: "laptop/build", entity_id: "project-run" };
const entry = (kind, over = {}) => ({ key: kind, kind, project: "Build", projectId: "build",
  projectKey: project.projectKey, deviceId: "laptop", name: "Current work", title: "Current work",
  facts: "Existing activity", anchorMs: 100, lastActivityMs: 200, unreadCount: 1,
  route: { name: "project", deviceId: "laptop", projectId: "build" }, ...over });
const dom = (html) => new DOMParser().parseFromString(html, "text/html");

describe("#629 inbox row layout", () => {
  it.each(["workspace", PROJECT_AGENT, TRACKER_TASK])("gives live %s a project line above its unchanged name and activity", (kind) => {
    const row = dom(inboxRowHtml(entry(kind)));
    const body = row.querySelector(".inbox-body");
    expect([...body.children].filter((child) => !child.matches(".warn")).map((child) => child.textContent.trim()))
      .toEqual(["Build", "Current work", "Existing activity"]);
    expect(body.querySelector(".inbox-project-line .inbox-tag").textContent).toBe("Build");
    expect(body.querySelector(".inbox-name .inbox-tag")).toBeNull();
    expect(row.querySelector(".inbox-facts-float")).toBeNull();
    if (kind === PROJECT_AGENT) expect(row.querySelector("button")).toBeNull();
  });

  it.each(["workspace", PROJECT_AGENT])("keeps Recent %s compact with its project above name and floating facts", (kind) => {
    for (const showProject of [true, false]) {
      const row = dom(inboxRowHtml(entry(kind), { quiet: true, showProject }));
      const name = row.querySelector(".inbox-name");
      const facts = row.querySelector(".inbox-facts-float");
      expect(facts.textContent).toBe("Existing activity");
      expect(facts.parentElement).toBe(name.parentElement);
      expect(row.querySelector(".inbox-project-line") !== null).toBe(showProject);
      expect(row.querySelector(".inbox-quiet-full")).toBeNull();
      expect(row.querySelector("[data-workspace-done]")).toBeNull();
    }
  });

  it("keeps empty Recent project activity compact using the existing fallback", () => {
    const row = dom(inboxRowHtml(entry(PROJECT_AGENT, { facts: "" }), { quiet: true }));
    expect(row.querySelector(".inbox-facts-float").textContent).toBe("Getting started");
    const task = dom(inboxRowHtml(entry(TRACKER_TASK, { facts: "" }), { quiet: true }));
    expect(task.querySelector(".inbox-facts")).toBeNull();
    expect(task.querySelector(".inbox-project-line")).not.toBeNull();
  });

  it.each([false, true])("gives a capture with a known project its own Inbox project line (Recent=%s)", (quiet) => {
    const capture = entry("capture", { captureId: "capture", captureState: "routed" });
    const row = dom(inboxRowHtml(capture, { quiet }));
    expect(row.querySelector(".inbox-project-line .inbox-tag").textContent).toBe("Build");
    expect(row.querySelector(".inbox-project-line").nextElementSibling.querySelector(".stitle").textContent).toBe("Current work");
    expect(row.querySelector("[data-capture-reroute]")).not.toBeNull();
    expect(dom(inboxRowHtml(capture, { quiet, showProject: false })).querySelector(".inbox-project-line")).toBeNull();
    expect(dom(inboxRowHtml({ ...capture, project: "", projectId: null }, { quiet })).querySelector(".inbox-project-line")).toBeNull();
  });

  it.each(["workspace", PROJECT_AGENT, TRACKER_TASK])("omits the project line for grouped %s entries, including Recent", (kind) => {
    for (const quiet of [false, true]) {
      const row = dom(inboxRowHtml(entry(kind), { showProject: false, quiet }));
      expect(row.querySelector(".inbox-project-line, .inbox-tag, .inbox-device")).toBeNull();
      expect(row.querySelector(".stitle").textContent).toBe("Current work");
    }
  });

  it("uses the pinned project agent topic and the workspace activity fallback without new actions", () => {
    const row = { kind: "branch", run_id: project.entity_id, projectKey: project.projectKey,
      agents: [{ id: "pin", topic: "Ship inbox layout" }] };
    const [agent] = projectAgentEntries([project], [row]);
    expect(agent.name).toBe("Ship inbox layout");
    expect(agent.project).toBe("Build");
    const rendered = dom(inboxRowHtml(agent));
    expect(rendered.querySelector(".inbox-facts").textContent).toBe("Getting started · 0 running");
    expect(rendered.querySelector("button")).toBeNull();
    const [fallback] = projectAgentEntries([project], [{ ...row, agents: [{ id: "pin", topic: "  " }] }]);
    expect(fallback.name).toBe("Build");
  });

  it("reads the exact pin's topic from a complete project roster", () => {
    const [agent] = projectAgentEntries([{ ...project, projectRoster: { pinned_agent_id: "pin", agents: [
      { id: "worker", topic: "Worker topic" }, { id: "pin", topic: "Coordinator topic" },
    ] } }]);
    expect(agent.name).toBe("Coordinator topic");
  });
});

it("sorts the project agent among its workspace siblings and puts it in Recent by the same cutoff", () => {
  const early = entry("workspace", { key: "early", anchorMs: 50 });
  const agent = entry(PROJECT_AGENT, { key: "agent", anchorMs: 100 });
  const late = entry("workspace", { key: "late", anchorMs: 150 });
  const blocksAt = (now) => workspaceProjectBlocks([late, agent, early], [project], [], null, now).blocks[0];
  const active = blocksAt(200 + RECENT_AFTER_MS - 1);
  expect(active.entries.map((row) => row.key)).toEqual(["early", "agent", "late"]);
  expect(active.unreadCount).toBe(3);
  const quiet = blocksAt(200 + RECENT_AFTER_MS);
  expect(quiet.entries).toEqual([]);
  expect(quiet.recent.map((row) => row.key)).toEqual(["early", "agent", "late"]);
  const head = dom(projectHeadHtml(active));
  expect(head.querySelector("[data-project-open], a")).toBeNull();
  expect(head.querySelector(".inbox-project-name").dataset.projectFold).toBe(project.projectKey);
  expect(head.querySelector(".inbox-project-name").getAttribute("aria-expanded")).toBe("true");
});
