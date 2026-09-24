// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, forceCloseDatabase } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const feedSubscribers = new Set();
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (listener) => {
    feedSubscribers.add(listener);
    listener({ items: [], projects: [] });
    return () => feedSubscribers.delete(listener);
  },
  refreshFeed: async () => [],
  dropFeedDevice: () => {},
  startFeed: () => {},
  stopFeed: () => {},
}));
vi.mock("../src/core/inboxView.js", () => ({ markSeen: async () => {} }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({
  notifyError: (...args) => notifyError(...args),
}));
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
const { adoptBridgeSelection, adoptDeviceSession, canAnswer, contextFor, setContextOffline } = await import("../src/core/deviceContexts.js");
const { createAgentSelection } = await import("../src/core/agentSelection.js");
const { mountAgentRail, resetAgentRailMemory } = await import("../src/core/agentRail.js");
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { resetOptimistic } = await import("../src/core/optimistic.js");
const { wipeCache } = await import("../src/core/localCache.js");
const { writeRailWorkItem } = await import("./railCacheFixture.js");

const CATALOG = {
  default_provider: "claude_adk",
  providers: [{
    id: "claude_adk",
    label: "Claude Code",
    models: [
      { id: "claude-opus-5", label: "Claude Opus 5", supports_effort: true },
      { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", supports_effort: false },
    ],
    efforts: ["low", "high"],
  }],
};

/** The head says the topic the agent named its work with, so these cases give
 *  each fixture agent one — that name is how they pin which conversation the
 *  panel is open on. */
const TOPICS = { "agent-a": "Fix login redirect", "agent-b": "Polish the rail" };

const agent = (id, ordinal, over = {}) => ({
  id,
  ordinal,
  topic: TOPICS[id] || "",
  conversation_id: `conversation-${id}`,
  provider: "claude_adk",
  state: "live",
  unread_count: 0,
  working: false,
  ...over,
});

const branchPayload = () => ({
  kind: "branch",
  project_id: "project-1",
  branch: "build/isolation",
  run_id: "run-1",
  worktree_id: "worktree-1",
  agents: [agent("agent-a", 1), agent("agent-b", 2)],
  run: { run_id: "run-1", thread: { items: [], sessions: [] } },
});

// The rail settles over the disk: its row, and the conversation in it —
// every record it opens is a turn.
const flush = async () => {
  for (let count = 0; count < 12; count += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

const host = () => document.querySelector("#agent-rail");
const input = () => host().querySelector("#railinput");
const bubble = (agentId) => host().querySelector(`[data-bubble="agent"][data-agent="${agentId}"]`);
const addBubble = () => host().querySelector('[data-bubble="add"]');
const writeDraft = (body) => {
  input().value = body;
  input().dispatchEvent(new Event("input", { bubbles: true }));
};

let payload;
let calls;
let rail;
let call;

// The one machine these cases work.
const DEVICE_ID = "device-1";

/** That machine's context: what the rail is handed to write into. */
const device = () => contextFor(DEVICE_ID);

/** Its bridge answers with this from now on. Adopting a session for a device
 *  the registry already holds is that bridge reconnecting, so this is how a
 *  case hands the rail a new transport mid-run. */
// Landed and greeted: a machine is asked for its catalog only once it has said
// which API it speaks.
const bridgeAnswersWith = (answer) =>
  adoptBridgeSelection(adoptDeviceSession({ deviceId: DEVICE_ID, call: answer }), { version: "1.22.0" }, null);

const mountBranch = async () => {
  // The work item is read off this machine's disk, so that is where a case
  // puts it before the rail goes up.
  await writeRailWorkItem(payload, { deviceId: DEVICE_ID });
  rail = mountAgentRail(host(), {
    kind: "branch",
    deviceId: DEVICE_ID,
    projectId: "project-1",
    branch: "build/isolation",
    autofocusComposer: true,
    cacheScope: device().cacheScope,
    chatRepository: device().chatRepository,
  });
  return flush();
};

beforeEach(async () => {
  document.body.innerHTML = '<div id="agent-rail"></div>';
  localStorage.clear();
  resetAgentRailMemory();
  resetOptimistic();
  resetChangeEvents();
  feedSubscribers.clear();
  notifyError.mockClear();
  payload = branchPayload();
  calls = [];
  call = vi.fn(async (method, params = {}) => {
    calls.push({ method, params });
    if (method === "models.list") return CATALOG;
    if (method === "branch.get") return payload;
    if (method === "issue.get") return payload;
    if (method === "thread.post") return { posted_sequence: 7 };
    if (method === "agent.choose") {
      return {
        entity_id: params.entity_id,
        agent_id: params.agent_id,
        model: params.model,
        effort: params.effort,
        choice_revision: params.expected_choice_revision + 1,
      };
    }
    return {};
  });
  bridgeAnswersWith(call);
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

describe("agent rail chat ownership", () => {
  it("keeps a named cached conversation mounted through offline and a closed database, then paints later cache writes", async () => {
    payload.run.thread.items = [
      { type: "message", data: { sequence: 1, role: "agent", body: "Cached first reply" } },
    ];
    let openedDb;
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      openedDb = this;
      return originalTransaction.apply(this, args);
    });
    try {
      await mountBranch();
      expect(host().querySelector(".rail-who").title).toBe("Fix login redirect");
      expect(host().textContent).toContain("Cached first reply");
      expect(openedDb).toBeDefined();

      setContextOffline(DEVICE_ID, { offline: true });
      forceCloseDatabase(openedDb);
      await flush();
      expect(canAnswer(device())).toBe(false);
      expect(host().querySelector(".rail-who").title).toBe("Fix login redirect");
      expect(host().textContent).toContain("Cached first reply");

      setContextOffline(DEVICE_ID, { offline: false });
      expect(canAnswer(device())).toBe(true);
      payload.agents[0].topic = "Fixed login redirect";
      payload.run.thread.items.push(
        { type: "message", data: { sequence: 2, role: "agent", body: "Refilled after reconnect" } },
      );
      await writeRailWorkItem(payload, { deviceId: DEVICE_ID });
      await flush();

      await vi.waitFor(() => {
        expect(host().querySelector(".rail-who").title).toBe("Fixed login redirect");
        expect(host().textContent).toContain("Refilled after reconnect");
      });
    } finally {
      transaction.mockRestore();
    }
  });

  it("keeps a late rejection with its original agent and leaves the selected draft and focus alone", async () => {
    let rejectPost;
    let postCount = 0;
    const baseCall = call;
    call = vi.fn(async (method, params = {}) => {
      if (method !== "thread.post") return baseCall(method, params);
      calls.push({ method, params });
      postCount += 1;
      if (postCount > 1) return { posted_sequence: 8 };
      return new Promise((_, reject) => (rejectPost = reject));
    });
    bridgeAnswersWith(call);
    await mountBranch();

    bubble("agent-b").click();
    await flush();
    writeDraft("the original B message");
    host().querySelector("#railsend").click();
    await flush();
    writeDraft("B has a newer draft");

    bubble("agent-a").click();
    await flush();
    writeDraft("A stays in focus");
    const focusedInput = input();
    focusedInput.focus();
    rejectPost(new Error("B refused the message"));
    await flush();

    expect(input()).toBe(focusedInput);
    expect(document.activeElement).toBe(focusedInput);
    expect(input().value).toBe("A stays in focus");
    expect(host().querySelector(".rail-who").getAttribute("title")).toBe("Fix login redirect");

    bubble("agent-b").click();
    await flush();
    expect(input().value).toBe("B has a newer draft");
    expect(host().querySelector(".chat-recovery-entry").textContent).toContain("Message not sent");

    const firstPost = calls.filter((entry) => entry.method === "thread.post")[0];
    host().querySelector(".chat-recovery-entry button").click();
    await flush();
    const posts = calls.filter((entry) => entry.method === "thread.post");
    expect(posts).toHaveLength(2);
    expect(posts[1].params).toMatchObject({
      entity_id: "run-1",
      agent_id: "agent-b",
      conversation_id: "conversation-agent-b",
      body: "the original B message",
      operation_id: firstPost.params.operation_id,
    });
    expect(input().value).toBe("B has a newer draft");
    expect(host().querySelector(".chat-recovery-entry")).toBeNull();
  });

  it("retains controller drafts across remounts and returns to the selected agent composer", async () => {
    await mountBranch();
    bubble("agent-b").click();
    await flush();
    writeDraft("agent B draft");
    addBubble().click();
    await flush();
    writeDraft("new agent draft");

    bubble("agent-b").click();
    await flush();
    expect(input().value).toBe("agent B draft");
    addBubble().click();
    await flush();
    expect(input().value).toBe("new agent draft");

    rail.dispose();
    rail = null;
    document.body.innerHTML = '<div id="agent-rail"></div>';
    await mountBranch();
    expect(input().value).toBe("agent B draft");
    writeDraft("message after returning");
    host().querySelector("#railsend").click();
    await flush();
    expect(calls.filter((entry) => entry.method === "agent.add")).toHaveLength(0);
    expect(calls.filter((entry) => entry.method === "thread.post").at(-1).params).toMatchObject({
      entity_id: "run-1",
      agent_id: "agent-b",
      conversation_id: "conversation-agent-b",
      body: "message after returning",
    });
    bubble("agent-a").click();
    await flush();
    bubble("agent-b").click();
    await flush();
    expect(input().value).toBe("");
  });

  it("restores the remembered conversation and draft without posting during hydration", async () => {
    await mountBranch();
    bubble("agent-b").click();
    await flush();
    writeDraft("agent B draft");
    rail.dispose();
    rail = null;
    document.body.innerHTML = '<div id="agent-rail"></div>';

    // The pin, work item, conversation and draft are separate cached records.
    // Their readbacks may settle together; a remount must still choose agent B
    // before any send and restore the draft against that conversation.
    rail = mountAgentRail(host(), {
      kind: "branch",
      deviceId: DEVICE_ID,
      projectId: "project-1",
      branch: "build/isolation",
      autofocusComposer: true,
      cacheScope: device().cacheScope,
      chatRepository: device().chatRepository,
    });

    await vi.waitFor(() => expect(input()?.value).toBe("agent B draft"));
    expect(host().querySelector("#rail-body").textContent).toContain("No conversation yet.");
    expect(calls.filter((entry) => entry.method === "agent.add")).toHaveLength(0);
    expect(calls.filter((entry) => entry.method === "thread.post")).toHaveLength(0);

    expect(input().disabled).toBe(false);
    writeDraft("send after hydration");
    host().querySelector("#railsend").click();
    await flush();

    expect(calls.filter((entry) => entry.method === "agent.add")).toHaveLength(0);
    expect(calls.find((entry) => entry.method === "thread.post").params).toMatchObject({
      entity_id: "run-1",
      agent_id: "agent-b",
      conversation_id: "conversation-agent-b",
      body: "send after hydration",
    });
  });

  it("keeps the provisional composer and its next draft while agent creation is pending", async () => {
    payload = { ...branchPayload(), agents: [] };
    let resolveAdd;
    const baseCall = call;
    call = vi.fn(async (method, params = {}) => {
      if (method !== "agent.add") return baseCall(method, params);
      calls.push({ method, params });
      return new Promise((resolve) => (resolveAdd = resolve));
    });
    bridgeAnswersWith(call);
    await mountBranch();

    const provisionalInput = input();
    provisionalInput.focus();
    writeDraft("create this agent");
    host().querySelector("#railsend").click();
    await flush();
    writeDraft("send this when ready");

    expect(input()).toBe(provisionalInput);
    expect(document.activeElement).toBe(provisionalInput);
    expect(input().value).toBe("send this when ready");
    host().querySelector("#railsend").click();
    await flush();

    resolveAdd({ entity_id: "run-1", agent: agent("created-agent", 1) });
    await flush();
    expect(calls.filter((entry) => entry.method === "agent.add")).toHaveLength(1);
    expect(calls.filter((entry) => entry.method === "thread.post").map((entry) => entry.params.body)).toEqual([
      "create this agent",
      "send this when ready",
    ]);
  });

  it("leaves the selected agent composer alone when another agent finishes creation", async () => {
    let resolveAdd;
    const baseCall = call;
    call = vi.fn(async (method, params = {}) => {
      if (method !== "agent.add") return baseCall(method, params);
      calls.push({ method, params });
      return new Promise((resolve) => (resolveAdd = resolve));
    });
    bridgeAnswersWith(call);
    await mountBranch();

    addBubble().click();
    await flush();
    writeDraft("create another agent");
    host().querySelector("#railsend").click();
    await flush();
    bubble("agent-b").click();
    await flush();
    writeDraft("agent B stays here");
    const selectedInput = input();
    selectedInput.focus();

    resolveAdd({ entity_id: "run-1", agent: agent("created-agent", 3) });
    await flush();

    expect(input()).toBe(selectedInput);
    expect(document.activeElement).toBe(selectedInput);
    expect(input().value).toBe("agent B stays here");
    expect(bubble("agent-b").classList.contains("active")).toBe(true);
  });

  it("retries an uncertain agent creation with its original creation identity", async () => {
    payload = { ...branchPayload(), agents: [] };
    let creationAttempts = 0;
    const baseCall = call;
    call = vi.fn(async (method, params = {}) => {
      if (method !== "agent.add") return baseCall(method, params);
      calls.push({ method, params });
      creationAttempts += 1;
      if (creationAttempts === 1) {
        throw Object.assign(new Error("connection lost after agent.add"), { uncertain: true });
      }
      return { entity_id: "run-1", agent: agent("created-agent", 1) };
    });
    bridgeAnswersWith(call);
    await mountBranch();

    writeDraft("start the agent");
    host().querySelector("#railsend").click();
    await flush();
    const recovery = host().querySelector(".chat-recovery-entry");
    expect(recovery.textContent).toContain("Agent creation uncertain");
    expect(recovery.querySelector("button")).not.toBeNull();

    const reconnectedCall = vi.fn(async (method, params = {}) => {
      if (method === "agent.add") {
        calls.push({ method, params });
        return { entity_id: "run-1", agent: agent("created-agent", 1) };
      }
      return baseCall(method, params);
    });
    bridgeAnswersWith(reconnectedCall);
    rail.dispose();
    rail = null;
    document.body.innerHTML = '<div id="agent-rail"></div>';
    await mountBranch();

    host().querySelector(".chat-recovery-entry button").click();
    await flush();

    const adds = calls.filter((entry) => entry.method === "agent.add");
    expect(adds).toHaveLength(2);
    expect(adds[1].params.creation_id).toBe(adds[0].params.creation_id);
    expect(calls.find((entry) => entry.method === "thread.post").params).toMatchObject({
      entity_id: "run-1",
      agent_id: "created-agent",
      body: "start the agent",
    });
    expect(reconnectedCall).toHaveBeenCalledWith("thread.post", expect.any(Object));
    expect(reconnectedCall).toHaveBeenCalledWith("agent.start", { id: "run-1", agent_id: "created-agent" });
  });

  it("uses the reconnected call for every step of an uncertain creation retry", async () => {
    payload = { ...branchPayload(), agents: [] };
    const originalCall = call;
    const oldMethods = [];
    const oldCall = vi.fn(async (method, params = {}) => {
      oldMethods.push(method);
      if (method === "agent.add") {
        calls.push({ method, params });
        throw Object.assign(new Error("connection lost after agent.add"), { uncertain: true });
      }
      return originalCall(method, params);
    });
    bridgeAnswersWith(oldCall);
    await mountBranch();

    writeDraft("start after reconnect");
    host().querySelector("#railsend").click();
    await flush();
    const firstAdd = calls.find((entry) => entry.method === "agent.add");
    expect(host().querySelector(".chat-recovery-entry button")).not.toBeNull();

    const newMethods = [];
    const newCall = vi.fn(async (method, params = {}) => {
      newMethods.push(method);
      calls.push({ method, params });
      if (method === "agent.add") return { entity_id: "run-1", agent: agent("created-agent", 1) };
      if (method === "thread.post") return { posted_sequence: 9 };
      if (method === "agent.start") return { agent_id: "created-agent" };
      return originalCall(method, params);
    });
    bridgeAnswersWith(newCall);
    host().querySelector(".chat-recovery-entry button").click();
    await flush();

    const retriedAdd = calls.filter((entry) => entry.method === "agent.add").at(-1);
    const retriedPost = calls.filter((entry) => entry.method === "thread.post").at(-1);
    expect(retriedAdd.params.creation_id).toBe(firstAdd.params.creation_id);
    expect(retriedPost.params.operation_id).toBe(firstAdd.params.creation_id.replace(/^creation:/, ""));
    expect(newMethods).toEqual(expect.arrayContaining(["agent.add", "thread.post", "agent.start"]));
    expect(oldMethods.filter((method) => method === "agent.add")).toHaveLength(1);
    expect(oldMethods).not.toContain("thread.post");
    expect(oldMethods).not.toContain("agent.start");
  });

  it("does not reopen a provisional composer when its late failure lands after an agent switch", async () => {
    let rejectCreation;
    const originalCall = call;
    const heldCall = vi.fn(async (method, params = {}) => {
      if (method !== "agent.add") return originalCall(method, params);
      calls.push({ method, params });
      return new Promise((_, reject) => (rejectCreation = reject));
    });
    bridgeAnswersWith(heldCall);
    await mountBranch();

    bubble("agent-b").click();
    await flush();
    addBubble().click();
    await flush();
    writeDraft("make another agent");
    host().querySelector("#railsend").click();
    await flush();

    bubble("agent-b").click();
    await flush();
    writeDraft("agent B remains selected");
    const selectedInput = input();
    rejectCreation(Object.assign(new Error("connection lost after agent.add"), { uncertain: true }));
    await flush();

    expect(input()).toBe(selectedInput);
    expect(input().value).toBe("agent B remains selected");
    expect(host().querySelector(".rail-who").getAttribute("title")).toBe("Polish the rail");
    expect(host().querySelector(".rail-newagent")).toBeNull();
    expect(bubble("agent-b").classList.contains("active")).toBe(true);
  });

  it("uses an issue execution context for the picker, post, and shared surface selection", async () => {
    const selection = createAgentSelection();
    payload = {
      kind: "issue",
      project_id: "project-1",
      issue_id: "issue-1",
      agents: [agent("issue-agent", 1)],
      thread: {
        items: [
          { type: "message", data: { sequence: 11, role: "agent", body: "already read" } },
          { type: "message", data: { sequence: 12, role: "agent", body: "new implementation reply" } },
        ],
        sessions: [],
      },
      execution_context: {
        entity_id: "run-live",
        agent_id: "execution-agent",
        conversation_id: "conversation-issue-agent",
        agent: agent("execution-agent", 1, {
          conversation_id: "conversation-issue-agent",
          choice_revision: 0,
          read_through_sequence: 11,
          unread_count: 1,
        }),
      },
    };
    await writeRailWorkItem(payload, { deviceId: DEVICE_ID });
    rail = mountAgentRail(host(), {
      kind: "issue",
      deviceId: DEVICE_ID,
      projectId: "project-1",
      issueId: "issue-1",
      autofocusComposer: true,
      selection,
      cacheScope: device().cacheScope,
      chatRepository: device().chatRepository,
    });
    await flush();

    const unreadLine = host().querySelector(".thread-unread-line");
    expect(unreadLine).toBeTruthy();
    expect(unreadLine.nextElementSibling.textContent).toContain("new implementation reply");

    host().querySelector(".composer-model .caret").click();
    host().querySelector('[data-action="model:claude-opus-5"]').click();
    await flush();
    writeDraft("continue the implementation");
    host().querySelector("#railsend").click();
    await flush();

    expect(calls.find((entry) => entry.method === "agent.choose").params).toMatchObject({
      entity_id: "run-live",
      agent_id: "execution-agent",
    });
    expect(calls.find((entry) => entry.method === "thread.post").params).toMatchObject({
      entity_id: "run-live",
      agent_id: "execution-agent",
      conversation_id: "conversation-issue-agent",
    });
    expect(selection.scope()).toEqual({ agent_id: "execution-agent" });
  });
});
