// @vitest-environment jsdom
// The issue route's host (views/issueView.js): which machine it talks to, and
// what it writes back into the URL. The surface itself is core/issueView.js and
// is tested against an injected caller in issueViewDom.test.js.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const stage = (id, title) => ({ id, title, state: "planned", approval: "planned", execution: "pending", open_comments: 0, comments: [] });

const issuePayload = {
  issue_id: "issue-1",
  plan_id: "issue-1",
  project_id: "p1",
  project: "Build",
  goal: "Rebuild the issue view",
  state: "plan_review",
  base_branch: "main",
  stages: [{ id: "s1", state: "planned" }],
  implementation_lineage: [],
  thread: { items: [], thread_last_sequence: 1 },
};

let App;
let renderIssue;
let resetDeviceContexts;
let theirCall;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/device/dev-2/project/p1/issue/issue-1";
  ({ App } = await import("../src/app.js"));
  ({ renderIssue } = await import("../src/views/issueView.js"));
  const contexts = await import("../src/core/deviceContexts.js");
  resetDeviceContexts = contexts.resetDeviceContexts;
  App.devices = [
    { id: "dev-1", name: "This device", status: "online" },
    { id: "dev-2", name: "Desktop", status: "online" },
  ];
  App.selectedDeviceId = "dev-1"; // home is this machine; the link is the other one
  App.call = vi.fn(async () => ({}));
  theirCall = vi.fn(async (method) => {
    if (method === "issue.get") return issuePayload;
    if (method === "issue.stages") return { stages: [stage("s1", "Wire"), stage("s2", "Paint")] };
    if (method === "issue.stage_doc") return { stage_id: "s1", contents: "# Wire" };
    if (method === "issue.doc") return { contents: "# The whole plan" };
    if (method === "board.list") return { items: [] };
    if (method === "project.list") return { projects: [] };
    return {};
  });
  const session = (deviceId, call) => ({ deviceId, call, close: () => {}, peer: () => {}, onCarrier: () => {} });
  contexts.adoptDeviceSession(session("dev-1", (...args) => App.call(...args)));
  contexts.adoptDeviceSession(session("dev-2", theirCall));
  App.route = { name: "issue", deviceId: "dev-2", projectId: "p1", id: "issue-1" };
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
  resetDeviceContexts();
});

describe("an issue on another device", () => {
  const reached = (call, method) => call.mock.calls.some(([name]) => name === method);

  it("calls the route device's call for entity.seen and the issue read", async () => {
    await renderIssue();
    await flush();

    expect(reached(theirCall, "entity.seen")).toBe(true);
    expect(reached(theirCall, "issue.get")).toBe(true);
    expect(reached(App.call, "entity.seen")).toBe(false);
    expect(reached(App.call, "issue.get")).toBe(false);
  });

  it("syncHash keeps the device segment when the open stage changes", async () => {
    await renderIssue();
    await flush();

    expect(location.hash).toBe("#/device/dev-2/project/p1/issue/issue-1/stage/s1");
    expect(App.route.deviceId).toBe("dev-2");

    document.querySelector('.stagerow[data-stage="s2"]').click();
    await flush();

    expect(location.hash).toBe("#/device/dev-2/project/p1/issue/issue-1/stage/s2");
    expect(App.route.deviceId).toBe("dev-2");
  });
});

// A link can name a machine this client has no session with. There is nothing
// to read and nothing to write until it answers, so the surface says so by name
// rather than painting an empty issue.
describe("an issue on a device this client has not opened", () => {
  it("names the device and asks it nothing", async () => {
    App.devices = [...App.devices, { id: "dev-3", name: "Desktop", status: "offline" }];
    App.route = { name: "issue", deviceId: "dev-3", projectId: "p1", id: "issue-1" };

    await renderIssue();
    await flush();

    expect(document.getElementById("root").textContent).toContain("Desktop isn't connected");
    expect(App.call).not.toHaveBeenCalled();
    expect(theirCall).not.toHaveBeenCalled();
  });
});
