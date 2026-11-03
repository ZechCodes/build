import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

const scope = { deviceId: "choice-admission-race", projectId: "proj-1" };
const dimensions = ["state", "status", "label", "assignee"];
const values = { state: ["open"], status: ["ready"], label: ["returned"], assignee: ["none"] };
const page = (dimension, choices = values[dimension]) => ({ project_id: scope.projectId, dimension, choices });
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

let createReads, readChoices, controller, enabled, session;
const options = (callRpc, onError = () => {}) => ({ scope, host: {}, callRpc, onError,
  capabilities: () => ({ filterChoices: true }), sessionOf: () => session, enabled: () => enabled });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  enabled = true; session = {};
  ({ createTaskPaneChoiceReads: createReads } = await import("../src/core/taskPaneSummaryReads.js"));
  ({ readFilterChoices: readChoices } = await import("../src/core/taskFilterChoices.js"));
});

afterEach(() => {
  vi.restoreAllMocks();
  controller?.dispose();
});

/** The page merge has refused admission, but IndexedDB has not completed its
 * transaction or resolved landFilterChoices yet. Return demand lands exactly
 * in that interval, rather than after the discarded promise has settled. */
function afterRejectedFirstAdmission(returnToList) {
  const original = IDBObjectStore.prototype.get;
  let armed = true;
  vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    const request = original.call(this, key);
    if (this.transaction.mode !== "readwrite" || !armed || !String(key).endsWith("|tracker-filter-choices|state")) return request;
    armed = false;
    let complete = false, callback;
    this.transaction.addEventListener("complete", () => { complete = true; }, { once: true });
    Object.defineProperty(request, "onsuccess", {
      configurable: true,
      get: () => callback,
      set(admit) {
        callback = (...args) => {
          admit(...args);
          returnToList(complete);
        };
      },
    });
    return request;
  });
}

it.each([1, 3])("serves %i return demands after refusal but before its transaction completes with one fresh round", async (presses) => {
  const pending = new Map(dimensions.map((dimension) => [dimension, deferred()]));
  const rpc = vi.fn((_method, { dimension }) => rpc.mock.calls.length <= 4
    ? pending.get(dimension).promise : Promise.resolve(page(dimension)));
  controller = createReads(options(rpc));
  await controller.ready;
  const first = controller.refresh();
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(4));
  enabled = false;
  const returns = [];
  let completedAtReturn, callsAtReturn;
  afterRejectedFirstAdmission((complete) => {
    completedAtReturn = complete;
    enabled = true;
    for (let press = 0; press < presses; press += 1) returns.push(controller.ensure());
    callsAtReturn = rpc.mock.calls.length;
  });
  for (const [dimension, reply] of pending) reply.resolve(page(dimension, dimension === "state" ? ["closed"] : values[dimension]));
  await first;
  await Promise.all(returns);
  expect(completedAtReturn).toBe(false);
  expect(returns).toHaveLength(presses);
  expect(callsAtReturn).toBe(4);
  await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(8));
  await vi.waitFor(async () => expect(await readChoices(scope, "state")).toMatchObject({
    choices: ["open"], complete: true, refreshing: false,
  }));
  expect(rpc.mock.calls.map(([, params]) => params.dimension)).toEqual([...dimensions, ...dimensions]);
});

it("does not reissue first pages on ensure after an RPC error", async () => {
  const failure = new Error("offline"), onError = vi.fn();
  const rpc = vi.fn(async (_method, { dimension }) => {
    if (dimension === "label") throw failure;
    return page(dimension);
  });
  controller = createReads(options(rpc, onError));
  await controller.ready;
  await controller.refresh();
  await Promise.all([controller.ensure(), controller.ensure(), controller.ensure()]);
  expect(onError).toHaveBeenCalledExactlyOnceWith(failure);
  expect(rpc).toHaveBeenCalledTimes(4);
});
