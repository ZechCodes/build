// @vitest-environment jsdom
// What one machine's greeting settles, and where it is written.
//
// Every bridge is greeted on its own session, and what that greeting picked is
// a fact about THAT machine: the adapter its session installed, the API version
// it reported, and which side is behind when no adapter here speaks to it. The
// connection layer hands all three to the device's context, because that is
// what every surface reads to decide whether the machine can be asked anything.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../src/api.js", () => ({
  fetchGatewayToken: async () => "tok",
  fetchIceServers: async () => [],
  fetchDevices: async () => [],
}));
vi.mock("../src/core/session.js", () => ({
  openSession: async () => {
    throw new Error("no rendezvous in this suite");
  },
}));
vi.mock("../src/core/peerLink.js", () => ({
  openPeerLink: async () => {
    throw new Error("no peer path in jsdom");
  },
}));
vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice: () => {},
  resetTerminalManager: () => {},
  terminalDeviceId: () => null,
  terminalManager: () => null,
  subscribeTerminalStatus: () => () => {},
  // Minting a terminal session is the connection layer's (spec rule 5); no
  // suite here opens one.
  provideTerminalSessions: () => {},
}));
vi.mock("../src/core/composeView.js", () => ({ flushCaptures: async () => {} }));

const { resetApplication } = await import("../src/app.js");
const { adoptDeviceSession, canAnswer, contextFor, resetDeviceContexts } = await import(
  "../src/core/deviceContexts.js"
);
const { resetChangeEvents } = await import("../src/core/changeEvents.js");
const { greetLiveBridge } = await import("../src/connection.js");
const { clearConnectionDiagnosticHistory, connectionDiagnosticHistory } = await import(
  "../src/core/connectionDiagnostics.js"
);

/** A bridge that answers one greeting and installs whatever the selection
 *  picked, exactly as core/session.js does. */
function bridgeAnswering(deviceId, greeting) {
  let installed = null;
  return {
    deviceId,
    call: vi.fn(async (method) => (method === "session.hello" ? greeting : {})),
    installAdapter: vi.fn((selection) => {
      installed = selection.unsupported ? null : selection.create(async () => ({}));
      return installed;
    }),
    adapter: () => installed,
    onPush: vi.fn(() => () => {}),
    onCarrier: vi.fn(),
    peer: vi.fn(),
    close: vi.fn(),
  };
}

const greet = async (deviceId, greeting) => {
  const session = bridgeAnswering(deviceId, greeting);
  const context = adoptDeviceSession(session);
  await greetLiveBridge(context);
  return { context, session };
};

beforeEach(() => {
  resetApplication();
  resetChangeEvents();
  resetDeviceContexts();
});

afterEach(() => {
  resetChangeEvents();
  resetDeviceContexts();
});

describe("what a greeting settles on the device it greeted", () => {
  it("writes the adapter and the API version onto that machine's context", async () => {
    const { context, session } = await greet("dev-a", { api_version: "2.0.0", push_events: true });
    expect(session.installAdapter).toHaveBeenCalledTimes(1);
    expect(context.adapter).toBe(session.adapter());
    expect(context.apiVersion).toBe("2.0.0");
    expect(context.unsupported).toBe(null);
    expect(canAnswer(context)).toBe(true);
  });

  it("names the side that is behind, and stops that machine answering", async () => {
    const { context } = await greet("dev-b", { api_version: "4.0.0", push_events: true });
    expect(context.adapter).toBe(null);
    expect(context.unsupported).toBe("app");
    expect(canAnswer(context)).toBe(false);
  });

  it("does not release a replacement barrier when an old re-greeting captured none", async () => {
    const { context, session } = await greet("dev-a", { api_version: "2.0.0" });
    let finishOld;
    session.call.mockImplementationOnce(() => new Promise((resolve) => { finishOld = resolve; }));
    const oldGreeting = greetLiveBridge(context);

    let finishReplacement;
    const hello = new Promise((resolve) => { finishReplacement = resolve; });
    const replacement = bridgeAnswering("dev-a", hello);
    expect(adoptDeviceSession(replacement)).toBe(context);
    let barrierReleased = false;
    context.greeted.then(() => { barrierReleased = true; });
    const newGreeting = greetLiveBridge(context);

    finishOld({ api_version: "2.0.0" });
    await oldGreeting;

    expect(barrierReleased).toBe(false);
    expect(replacement.installAdapter).not.toHaveBeenCalled();

    finishReplacement({ api_version: "2.0.0" });
    await newGreeting;
    expect(barrierReleased).toBe(true);
    expect(replacement.installAdapter).toHaveBeenCalledTimes(1);
  });
});

// ---- #30: the reconnect settles what the dead path left uncertain -----------

describe("a reconnect's greeting resolving the posts the last session stranded", () => {
  /** What a bridge that keeps a post-operation ledger says in its greeting. */
  const POST_OPERATIONS_GREETING = {
    api_version: "2.0.0",
    capabilities: ["threads.postOperations"],
    thread_post_operations: { version: 1, status_method: "thread.operation" },
  };

  /** A device whose repository is holding one post uncertain, as a path that
   *  died mid-send leaves it. */
  const strandedDevice = async (deviceId = "dev-c") => {
    const first = bridgeAnswering(deviceId, POST_OPERATIONS_GREETING);
    const context = adoptDeviceSession(first);
    await greetLiveBridge(context);
    const controller = context.chatRepository.controller({
      entityId: "run-1",
      agentId: "agent-1",
      conversationId: "conversation-1",
    });
    // The send goes out on a wire that stops carrying: the post is uncertain.
    first.call.mockImplementation(async () => {
      throw Object.assign(new Error("thread.post timed out"), { timedOut: true, uncertain: true, deadline: "path" });
    });
    const submission = controller.captureSubmission({ body: "stranded", attachments: [] });
    await expect(controller.post(submission)).rejects.toThrow("timed out");
    expect(controller.recoveries().map((one) => one.status)).toEqual(["uncertain"]);
    return { context, controller };
  };

  it("asks the operation ledger about it without anybody pressing Check delivery", async () => {
    const { context, controller } = await strandedDevice();
    const asked = [];
    const reconnected = bridgeAnswering("dev-c", POST_OPERATIONS_GREETING);
    reconnected.call.mockImplementation(async (method, params = {}) => {
      asked.push(method);
      if (method === "session.hello") {
        return POST_OPERATIONS_GREETING;
      }
      return {
        operation_id: params.operation_id,
        entity_id: params.entity_id,
        agent_id: params.agent_id,
        conversation_id: "conversation-1",
        choice_revision: 0,
        posted_sequence: 4,
        status: "delivered",
      };
    });
    expect(adoptDeviceSession(reconnected)).toBe(context);

    await greetLiveBridge(context);
    await Promise.resolve(); // the resolution is not awaited by the greeting
    await Promise.resolve();

    expect(asked).toContain("thread.operation");
    expect(controller.recoveries()).toEqual([]);
  });

  it("releases the attachment fetches the dead path ate, and records that it did", async () => {
    const { context } = await strandedDevice("dev-e");
    const { threadState } = context.chatRepository.history("conversation-1");
    threadState.deferAttachment("shots/big.png");
    const reconnected = bridgeAnswering("dev-e", { api_version: "2.0.0" });
    expect(adoptDeviceSession(reconnected)).toBe(context);
    clearConnectionDiagnosticHistory();

    await greetLiveBridge(context);

    expect(threadState.attachmentDeferred("shots/big.png")).toBe(false);
    // A figure still on "loading" after a reconnect has two possible causes —
    // nothing released it, or nothing repainted — and this is what tells them
    // apart in a reader's report.
    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "attachments-released"))
      .toMatchObject([{ connection: "dev-e:attachments", conversations: 1, paths: 1 }]);
  });

  it("says nothing about a reconnect that had no pictures waiting", async () => {
    const { context } = await strandedDevice("dev-f");
    const reconnected = bridgeAnswering("dev-f", { api_version: "2.0.0" });
    expect(adoptDeviceSession(reconnected)).toBe(context);
    clearConnectionDiagnosticHistory();

    await greetLiveBridge(context);

    expect(connectionDiagnosticHistory().filter((entry) => entry.event === "attachments-released")).toEqual([]);
  });

  it("does not hold the app back on it: the greeting settles first", async () => {
    const { context } = await strandedDevice("dev-d");
    let releaseLedger;
    const reconnected = bridgeAnswering("dev-d", {});
    reconnected.call.mockImplementation(async (method) => {
      if (method === "session.hello") {
        return POST_OPERATIONS_GREETING;
      }
      return new Promise((resolve) => { releaseLedger = resolve; });
    });
    expect(adoptDeviceSession(reconnected)).toBe(context);

    // A ledger that never answers must not keep the machine from being usable.
    await greetLiveBridge(context);

    expect(context.apiVersion).toBe("2.0.0");
    expect(canAnswer(context)).toBe(true);
    releaseLedger?.({});
  });
});
