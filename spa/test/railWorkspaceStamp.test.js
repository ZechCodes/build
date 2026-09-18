// @vitest-environment jsdom
// The workspace a message to the project's agent was sent from.
//
// The project's conversation is reachable from every workspace's rail, so the
// agent on the other end of it cannot tell where the user is standing. A
// message sent from a workspace rail says so: the workspace leads the message's
// viewing context, the composer shows it as a chip the reader cannot take off,
// and the sent message wears it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const feedSubscribers = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (listener) => {
    feedSubscribers.add(listener);
    listener({ items: [], projects: [], workspaces: [] });
    return () => feedSubscribers.delete(listener);
  },
  refreshFeed: async () => {},
  dropFeedDevice: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({ markSeen: async () => {} }));
vi.mock("../src/core/notify.js", () => ({ notifyError: () => {} }));
vi.mock("../src/core/surfaceTabs.js", () => ({ mountAgentTab: () => ({ dispose() {} }) }));
vi.mock("../src/core/agentCanvas.js", () => ({
  createPatternRenderer: () => ({
    destroy() {},
    isWorking: () => false,
    setDimmed() {},
    setInk() {},
    setWorking() {},
  }),
}));

const { resetApplication } = await import("../src/app.js");
const { adoptDeviceSession, contextFor } = await import("../src/core/deviceContexts.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { wipeCache } = await import("../src/core/localCache.js");

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{ id: "claude_adk", label: "Claude Code", models: [], efforts: [] }],
};

const DEVICE_ID = "device-1";
const PROJECT_ID = "proj-1";
const WORKSPACE_ID = "ws-3f2a91c4";
const OWNER = "run-project";
const STAMP = { kind: "workspace", workspace_id: WORKSPACE_ID, name: "wire-facade" };

const agent = (id) => ({
  id,
  ordinal: 1,
  topic: id === "pa-1" ? "Sort the workspaces" : "Fix login redirect",
  conversation_id: `conversation-${id}`,
  provider: "claude_adk",
  state: "live",
  unread_count: 0,
  working: false,
});

const flush = async () => {
  for (let count = 0; count < 8; count += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const host = () => document.querySelector("#agent-rail");
const panel = () => host().querySelector("#rail-panel");
const projectBubble = () => host().querySelector('[data-bubble="project"]');
const tray = () => panel().querySelector("#railinputcontext");
const messageChips = () => [...host().querySelectorAll(".message-viewing-context .viewing-context-label")]
  .map((chip) => chip.textContent);
const callsTo = (method) => calls.filter((call) => call.method === method);

let calls;
let posted;
let rail;

/** The project's conversation, carrying whatever has been sent into it. */
const projectPayload = () => ({
  entity_id: OWNER,
  run_id: OWNER,
  project_id: PROJECT_ID,
  agents: [agent("pa-1")],
  thread: { items: posted, sessions: [] },
});

const workspacePayload = () => ({
  workspace_id: WORKSPACE_ID,
  project_id: PROJECT_ID,
  name: "wire-facade",
  entity_id: "run-workspace",
  agents: [agent("wa-1")],
  directories: [],
  thread: { items: [], sessions: [] },
});

const mountRail = async (context) => {
  rail = mountAgentRail(host(), {
    deviceId: DEVICE_ID,
    projectId: PROJECT_ID,
    projectAgent: { projectId: PROJECT_ID },
    cacheScope: contextFor(DEVICE_ID).cacheScope,
    chatRepository: contextFor(DEVICE_ID).chatRepository,
    ...context,
  });
  await flush();
};

const mountWorkspaceRail = () =>
  mountRail({ kind: "workspace", workspaceId: WORKSPACE_ID });

/** The project's own page: the rail stands on the project's conversation from
 *  the start, and there is no workspace behind it to name. */
const mountProjectRail = () =>
  mountRail({ kind: "project", entityId: OWNER, projectAgent: { projectId: PROJECT_ID, entityId: OWNER } });

const send = async (body) => {
  panel().querySelector("#railinput").value = body;
  panel().querySelector("#railsend").click();
  await flush();
};

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  feedSubscribers.clear();
  calls = [];
  posted = [];
  adoptDeviceSession({
    deviceId: DEVICE_ID,
    call: async (method, params = {}) => {
      calls.push({ method, params });
      if (method === "models.list") return CATALOG;
      if (method === "workspace.get") return workspacePayload();
      if (method === "project.list") {
        return { projects: [{ project_id: PROJECT_ID, name: "build", entity_id: OWNER, run_id: OWNER }] };
      }
      if (method === "project.ensure_conversation") {
        return { project_id: PROJECT_ID, entity_id: OWNER, run_id: OWNER };
      }
      if (method === "thread.post") {
        posted.push({
          id: `m-${posted.length + 1}`,
          type: "message",
          data: {
            sequence: posted.length + 1,
            role: "user",
            body: params.body,
            created_at: "2026-09-18T12:00:00Z",
            viewing_context: params.viewing_context,
          },
        });
        return { posted_sequence: posted.length };
      }
      if (method === "run.get") return projectPayload();
      return {};
    },
  });
  // The stamp rides `viewing_context`, which the SPA sends only where the
  // bridge offers it.
  contextFor(DEVICE_ID).chatRepository.configureCapabilities({ message_context: { version: 1 } });
  await wipeCache();
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
});

afterEach(() => {
  rail?.dispose();
  rail = null;
  resetApplication();
  resetChangeEvents();
  vi.useRealTimers();
});

describe("a message to the project's agent from a workspace rail", () => {
  it("leads its viewing context with the workspace it was sent from", async () => {
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    await send("what is running here?");

    expect(callsTo("thread.post")).toHaveLength(1);
    expect(callsTo("thread.post")[0].params).toMatchObject({
      entity_id: OWNER,
      body: "what is running here?",
      viewing_context: { version: 1, items: [STAMP] },
    });
  });

  it("says so in the composer's tray, as a chip the reader cannot take off", async () => {
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    expect(tray().textContent).toContain("from wire-facade");
    expect(tray().querySelector("button")).toBeNull();
  });

  it("wears it on the message once it is sent", async () => {
    await mountWorkspaceRail();
    projectBubble().click();
    await flush();

    await send("what is running here?");

    expect(messageChips()).toEqual(["from wire-facade"]);
  });
});

describe("a message sent from nowhere in particular", () => {
  it("names no workspace on the project's own page", async () => {
    await mountProjectRail();

    await send("how are the workspaces doing?");

    expect(tray().textContent).toBe("");
    expect(callsTo("thread.post")[0].params.viewing_context).toBeUndefined();
  });

  it("names none in the workspace's own conversation, where the agent is already in it", async () => {
    await mountWorkspaceRail();

    await send("rebase onto main");

    expect(tray().textContent).toBe("");
    expect(callsTo("thread.post")[0].params.viewing_context).toBeUndefined();
  });
});
