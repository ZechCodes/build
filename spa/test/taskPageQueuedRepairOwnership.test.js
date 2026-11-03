// @vitest-environment jsdom
// Real context adoption leaves an interval before greeting's normal refresh.
// A repair rejected in that interval must not consume the new owner's attempt.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { fakeSession } from "./deviceSessionFixture.js";
import { task } from "./trackerWireFixture.js";

const deviceId = "queued-page-repair", projectId = "proj-1", taskId = "task-42";
const scope = { deviceId, projectId };
const row = task({ id: taskId, project_id: projectId, number: 42, title: "Held title", body: "Held body" });
const answer = (title = row.title) => ({ task: { ...row, title }, timeline: [] });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
let page, runtime, requests;

beforeEach(() => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="queued-page"></div>';
  page = null;
  runtime = null;
  requests = [];
});

afterEach(async () => {
  page?.dispose();
  for (const request of requests) request.resolve(answer("Disposed reply"));
  await Promise.allSettled(runtime?.reads || []);
  runtime?.changes.resetChangeEvents();
  runtime?.contexts.resetDeviceContexts();
  vi.restoreAllMocks();
});

function sessionWithHeldReads() {
  const session = fakeSession(deviceId);
  const gets = [];
  session.call.mockImplementation((method) => {
    if (method === "session.hello") return Promise.resolve({
      api_version: "3.24.0", push_events: true, events: ["changes"], capabilities: [],
    });
    if (method !== "tasks.get") return Promise.resolve({});
    const request = deferred();
    gets.push(request);
    requests.push(request);
    return request.promise;
  });
  return { session, gets };
}

async function mountRealPage() {
  runtime = {
    tracker: await import("../src/core/trackerCache.js"),
    content: await import("../src/core/taskContentAuthority.js"),
    local: await import("../src/core/localCache.js"),
    contexts: await import("../src/core/deviceContexts.js"),
    changes: await import("../src/core/changeEvents.js"),
    reads: [], asks: [],
  };
  // Observe the actual scheduler; all queueing, ownership checks, and readers
  // execute unchanged. Promises let assertions await consumption, not a delay.
  const trailing = await import("../src/core/trailingRead.js"), original = trailing.trailingRead;
  vi.spyOn(trailing, "trailingRead").mockImplementation((reader, options) => {
    if (reader.name !== "readTask") return original(reader, options);
    const scheduled = original((...args) => {
      const done = Promise.resolve(reader(...args));
      runtime.reads.push(done);
      return done;
    }, options);
    return (...args) => {
      const done = scheduled(...args);
      runtime.asks.push(done);
      return done;
    };
  });
  const address = runtime.tracker.taskAddress(deviceId, projectId, taskId);
  const contentRead = await runtime.content.captureTaskContentRead(address, { revalidate: "full" });
  await runtime.tracker.writeTaskRecord(deviceId, projectId, taskId,
    runtime.tracker.taskRecord(row, []), { contentRead });
  expect(runtime.content.taskFullRefreshKey((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task)).toBeNull();
  const writer = vi.spyOn(runtime.tracker, "writeTaskRecord");
  const coherentReads = vi.spyOn(runtime.local, "readCachedMany");
  const old = sessionWithHeldReads();
  const context = runtime.contexts.adoptDeviceSession(old.session);
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(document.querySelector("#queued-page"), {
    ...scope, taskId, projectKey: `${deviceId}|${projectId}`, callRpc: context.rpc,
    catalog: () => ({ providers: [] }), refreshCatalog: async () => ({ providers: [] }),
    feed: () => ({ items: [], workspaces: [] }), navigate: vi.fn(),
  });
  await vi.waitFor(() => expect(old.gets).toHaveLength(1));
  expect(runtime.asks).toHaveLength(1);
  return { address, old, context, writer, coherentReads };
}

async function retireBody(mounted) {
  await runtime.content.mergeTaskContentRecordsTogether(scope, [mounted.address], ([record]) => ({
    values: [{ ...record.value, task: runtime.content.retireTaskContent(record.value.task, "body") }], retire: true,
  }));
  const record = await runtime.tracker.readTaskRecord(deviceId, projectId, taskId);
  const key = runtime.content.taskFullRefreshKey(record.task);
  expect(key).toEqual(expect.any(String));
  return key;
}

async function reannounceSameKey(mounted, key) {
  const before = mounted.coherentReads.mock.calls.length;
  // A real cache announcement, retaining the exact record and retirement K.
  await runtime.local.mergeCachedAtomically(mounted.address, (record) => record);
  await vi.waitFor(() => expect(mounted.coherentReads.mock.calls.length).toBeGreaterThan(before));
  const observed = mounted.coherentReads.mock.results.slice(before);
  await Promise.all(observed.map((result) => result.value));
  expect(runtime.content.taskFullRefreshKey((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task)).toBe(key);
}

async function replaceWithQueuedRepair() {
  const mounted = await mountRealPage();
  const key = await retireBody(mounted);
  await vi.waitFor(() => expect(runtime.asks).toHaveLength(2));
  expect(runtime.reads).toHaveLength(1); // K is queued behind the old RPC.
  const replacement = sessionWithHeldReads();
  expect(runtime.contexts.adoptDeviceSession(replacement.session)).toBe(mounted.context);
  expect(mounted.context.session).toBe(replacement.session);
  expect(mounted.context.adapter).toBeNull();
  expect(runtime.contexts.canAnswer(mounted.context)).toBe(true);
  mounted.old.gets[0].resolve(answer("Obsolete old reply"));
  await runtime.asks[1]; // The real queued reader has consumed its old owner.
  expect(runtime.reads).toHaveLength(2);
  expect(replacement.gets).toHaveLength(0);
  expect(mounted.writer).not.toHaveBeenCalled();
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.title).toBe(row.title);
  expect(document.querySelector(".task-page-title")?.textContent).not.toBe("Obsolete old reply");
  return { ...mounted, replacement, key };
}

it("admits the same repair key for a real replacement session after rejecting the old queued owner before greeting", async () => {
  const mounted = await replaceWithQueuedRepair();
  await reannounceSameKey(mounted, mounted.key);
  await vi.waitFor(() => expect(mounted.replacement.gets).toHaveLength(1));
  expect(mounted.replacement.session.call.mock.calls.some(([method]) => method === "session.hello")).toBe(false);
  expect(mounted.context.adapter).toBeNull();
  mounted.replacement.gets[0].resolve(answer("Current pre-greeting repair"));
  await runtime.asks.at(-1);
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.title).toBe("Current pre-greeting repair");
  expect(mounted.old.gets).toHaveLength(1);
  expect(mounted.replacement.gets).toHaveLength(1);
});

it("recovers the queued repair through a real replacement greeting normal refresh", async () => {
  const mounted = await replaceWithQueuedRepair();
  // This is an independent positive: greeting's normal demand can cover K.
  // It does not establish that a pre-greeting same-key repair was admitted.
  await runtime.changes.greetBridge(mounted.replacement.session.call, {
    deviceId, isCurrent: () => mounted.context.session === mounted.replacement.session,
    install: (selection) => {
      const adapter = selection.create(mounted.replacement.session.call);
      runtime.contexts.adoptBridgeSelection(mounted.context, selection, adapter);
      return adapter;
    },
  });
  await vi.waitFor(() => expect(mounted.replacement.gets).toHaveLength(1));
  expect(mounted.replacement.session.call.mock.calls.filter(([method]) => method === "session.hello")).toHaveLength(1);
  mounted.replacement.gets[0].resolve(answer("Greeting repaired title"));
  await runtime.asks.at(-1);
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.title).toBe("Greeting repaired title");
  expect(mounted.replacement.gets).toHaveLength(1);
  expect(mounted.old.gets).toHaveLength(1);
});

it("coalesces an admitted repair key through refusal and admits a genuine later retirement for the same owner", async () => {
  const mounted = await mountRealPage();
  mounted.old.gets[0].resolve(answer());
  await runtime.asks[0];
  const key = await retireBody(mounted);
  await vi.waitFor(() => expect(mounted.old.gets).toHaveLength(2));
  await reannounceSameKey(mounted, key);
  expect(runtime.asks).toHaveLength(2);
  expect(mounted.old.gets).toHaveLength(2);
  mounted.old.gets[1].reject(new Error("Refused current-owner repair"));
  await runtime.asks[1];
  await reannounceSameKey(mounted, key);
  expect(runtime.asks).toHaveLength(2);
  expect(mounted.old.gets).toHaveLength(2);
  const nextKey = await retireBody(mounted);
  expect(nextKey).not.toBe(key);
  await vi.waitFor(() => expect(mounted.old.gets).toHaveLength(3));
  mounted.old.gets[2].resolve(answer("Later retirement repaired"));
  await runtime.asks[2];
  expect((await runtime.tracker.readTaskRecord(deviceId, projectId, taskId)).task.title).toBe("Later retirement repaired");
  expect(mounted.old.gets).toHaveLength(3);
});

it("coalesces the same accepted repair key while its actual content basis capture is awaiting dispatch", async () => {
  const mounted = await mountRealPage();
  mounted.old.gets[0].resolve(answer());
  await runtime.asks[0];
  const allocated = deferred(), release = deferred();
  const capture = runtime.content.captureTaskContentRead;
  let heldOnce = false;
  vi.spyOn(runtime.content, "captureTaskContentRead").mockImplementation(async (...args) => {
    const contentRead = await capture(...args);
    if (!heldOnce && args[0]?.kind === mounted.address.kind && args[0]?.sub === taskId) {
      heldOnce = true;
      allocated.resolve(contentRead);
      await release.promise;
    }
    return contentRead;
  });
  try {
    const key = await retireBody(mounted);
    const contentRead = await allocated.promise;
    expect(contentRead.address).toEqual(mounted.address);
    expect(contentRead.readOrder).toBeGreaterThan(0);
    expect(contentRead.ticket.storeGeneration).toEqual(expect.any(String));
    expect(contentRead.ticket.scopeGeneration).toEqual(expect.any(String));
    expect(contentRead.revalidate).toBe("full");
    expect(mounted.old.gets).toHaveLength(1); // Only startup crossed the wire.
    expect(runtime.contexts.contextFor(deviceId)).toBe(mounted.context);
    expect(mounted.context.session).toBe(mounted.old.session);
    await reannounceSameKey(mounted, key);
    expect(mounted.old.gets).toHaveLength(1); // Same K arrived before dispatch.

    // Answer every real subsequent transport dispatch so waiting for the
    // scheduler cannot hide a duplicate behind an unresolved test promise.
    const transport = mounted.old.session.call.getMockImplementation();
    mounted.old.session.call.mockImplementation((...args) => {
      const response = transport(...args);
      if (args[0] === "tasks.get") mounted.old.gets.at(-1).resolve(answer("Captured repair"));
      return response;
    });
    release.resolve();
    await Promise.all(runtime.asks);
    expect(mounted.writer.mock.calls[1]?.[4]?.contentRead).toBe(contentRead);
    // One initial get plus ONE repair; a same-owner duplicate K is not a push.
    expect(mounted.old.gets).toHaveLength(2);
  } finally {
    release.resolve();
  }
});
