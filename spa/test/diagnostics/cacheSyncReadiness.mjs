// Opt-in diagnostics only. The normal fixture does not import this controller.
import { it, expect } from "vitest";
import { openSync, closeSync, writeFileSync, fsyncSync, renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";

const required = new Set(["s-inbox", "s-background"]);
const seedAddress = { deviceId: "dev-1", entityId: "run-1", kind: "file", sub: "src/a.js" };
const seedValue = { file: { path: "src/a.js" }, openedAt: 1 };
let emergencyCleanup = () => {};

// The opt-in transform calls this before the normal fixture's afterEach
// releases any gate. No observer from a timed-out body may cross teardown.
export function readinessAfterEach() { emergencyCleanup(); }

function capture(name) {
  const events = [];
  const identities = new WeakMap();
  let lost = 0, bytes = 0, nextId = 0;
  let ended = false, incomplete = false;
  const identity = (value) => {
    if (!value || typeof value !== "object") return null;
    if (!identities.has(value)) identities.set(value, ++nextId);
    return identities.get(value);
  };
  const event = (type, data = {}) => {
    if (ended) return;
    const entry = { sequence: events.length + lost + 1, at: performance.now(), type, ...data };
    const size = Buffer.byteLength(JSON.stringify(entry));
    if (events.length >= 128 || bytes + size > 28 * 1024) { lost += 1; return; }
    bytes += size;
    events.push(entry);
  };
  const finish = (error) => {
    if (ended) return;
    event("end-observation", { error: error ? String(error) : null });
    ended = true;
    const artifact = { name, mode: process.env.READINESS_MODE, head: process.env.READINESS_HEAD,
      nodeVersion: process.version, instrumentationComplete: !incomplete && lost === 0, lost, events };
    const path = join(process.env.READINESS_ARTIFACT_DIR, `${name}.json`);
    const payload = JSON.stringify(artifact);
    const durable = (filePath, content) => {
      const file = openSync(filePath, "w");
      try { writeFileSync(file, content); fsyncSync(file); } finally { closeSync(file); }
    };
    const flushDirectory = () => {
      const directory = openSync(process.env.READINESS_ARTIFACT_DIR, "r");
      try { fsyncSync(directory); } finally { closeSync(directory); }
    };
    try {
      durable(`${path}.tmp`, payload);
      renameSync(`${path}.tmp`, path);
      flushDirectory();
      durable(`${path}.receipt`, JSON.stringify({ sha256: createHash("sha256").update(payload).digest("hex"), persisted: true }));
      flushDirectory();
    } catch (captureError) {
      // A parseable prefix is not proof of persistence. Publish no success
      // receipt if either file/directory flush failed, even in expected RED.
      rmSync(`${path}.receipt`, { force: true });
      rmSync(path, { force: true });
      console.error("INCONCLUSIVE capture persistence failure", captureError);
      throw captureError;
    }
    if (lost) throw new Error(`INCONCLUSIVE: lost ${lost} readiness trace events`);
  };
  event("capture-registered", { limit: 128, byteLimit: 28 * 1024 });
  return { event, identity, finish, markIncomplete() { incomplete = true; } };
}

// Local cancellation fencing for timed-out diagnostic bodies. It cannot
// cancel an admitted IDB transaction; it prevents subsequent test actions.
function diagnosticScope(environment, trace) {
  let aborted = false, rejectCancellation;
  const cancelled = new Promise((_, reject) => { rejectCancellation = reject; });
  void cancelled.catch(() => {});
  const current = () => { if (aborted) throw new Error("Diagnostic observation ended"); };
  const guarded = (object, preservePromises = false) => new Proxy(object, {
    get(target, key) {
      const value = target[key];
      if (typeof value !== "function") return value;
      return (...args) => {
        current();
        const result = value.apply(target, args);
        if (preservePromises || !result?.then) return result;
        return Promise.race([result, cancelled]).then((answer) => { current(); return answer; });
      };
    },
  });
  const wrapped = guarded({ ...environment, cache: guarded(environment.cache),
    changeEvents: guarded(environment.changeEvents), sync: guarded(environment.sync, true) });
  return { environment: wrapped, abort() {
    if (aborted) return;
    aborted = true;
    trace.markIncomplete();
    trace.event("controller-aborted-before-finally");
    rejectCancellation(new Error("Diagnostic test ended before controller completion"));
    environment.sync.stopCacheSync();
  } };
}

function observe(env, wire, trace) {
  const held = new Set();
  let releaseInbox;
  const inbox = new Promise((resolve) => { releaseInbox = resolve; });
  const stopWire = wire.onSubscription(({ phase, request, error }) => {
    trace.event(`subscription-${phase}`, { request: trace.identity(request),
      session: trace.identity(request.session), spec: request.spec, error: error ? String(error) : null });
  });
  const stopManager = env.changeEvents.onSubscriptionHeld((device, id) => {
    if (device !== "dev-1" || !required.has(id)) return;
    held.add(id);
    const acknowledgment = wire.subscriptions.get(id);
    trace.event("manager-held", { id, session: trace.identity(acknowledgment?.session),
      spec: acknowledgment?.spec });
    if (id === "s-inbox") releaseInbox();
  });
  return { held, inbox, dispose: () => { stopWire(); stopManager(); } };
}

async function completePasses(env, initial, trace) {
  let pass = initial;
  while (pass) {
    const id = trace.identity(pass);
    trace.event("pass-observed", { pass: id });
    const result = await pass;
    trace.event("pass-completed", { pass: id, result });
    expect(result).toBe(true);
    pass = env.sync.passInFlight("dev-1");
  }
}

async function seed(env, trace) {
  trace.event("seed-attempt");
  await env.cache.writeCached(seedAddress, seedValue);
  const record = await canonicalSeed(env);
  expect(record?.value).toEqual(seedValue);
  expect(typeof record?.write).toBe("string");
  trace.event("seed-committed", { address: seedAddress, write: record.write,
    valueHash: createHash("sha256").update(JSON.stringify(record.value)).digest("hex") });
}

async function canonicalSeed(env) {
  const outcome = await env.cache.readCachedOutcome(seedAddress);
  expect(outcome.committed, "canonical seed verification read did not commit").toBe(true);
  return outcome.result;
}

async function originalStandDown(env, wire, fixture, trace, release) {
  const answerFile = wire.hold("fs.read");
  release.push(answerFile);
  fixture.assertCurrent();
  trace.event("first-intended-push");
  await env.flush([{ entity_id: "run-1", files: { paths: ["src/a.js"] } }], "s-background");
  fixture.assertCurrent();
  // These are the original four count/eviction/hand-back assertions.
  expect(env.calls("fs.read")).toHaveLength(1);
  await env.flush([{ entity_id: "board", state: { revision: 1, removed: ["run-1"] } }]);
  expect(await env.cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
  env.setBoard([]);
  env.sync.startCacheSync();
  await env.settle();
  answerFile();
  await env.settle();
  expect(await env.cache.cachedAddresses({ deviceId: "dev-1", entityId: "run-1" })).toEqual([]);
  expect(env.calls("fs.read")).toHaveLength(1);
  trace.event("all-original-assertions-passed");
}

async function delayedLanding(env, trace, release, cleanup) {
  const wire = env.subscribingBridge();
  wire.answering("fs.read", (params) => ({ path: params.path, size: 3, content_b64: "bmV3" }));
  await env.greet();
  env.setBoard([env.branchItem()]);
  const observer = observe(env, wire, trace);
  cleanup.push(observer.dispose);
  const background = wire.hold("changes.subscribe", (params) => params.subscription_id === "s-background");
  release.push(background);
  const fixture = env.beginPushFixture(wire);
  cleanup.push(fixture.dispose);
  trace.event("initial-pass-captured", { pass: trace.identity(fixture.initial),
    session: trace.identity(env.currentContext()?.session) });
  let ready = false;
  // Observe rejection immediately; retain the original error via the awaited promise.
  const readiness = fixture.ready.then((seal) => {
    ready = true;
    trace.event("barrier-ready", { session: trace.identity(seal.session), generation: seal.generation,
      passes: seal.passes.map(trace.identity) });
    return seal;
  });
  void readiness.catch(() => {});
  try {
    await completePasses(env, fixture.initial, trace);
    await observer.inbox;
    await completePasses(env, env.sync.passInFlight("dev-1"), trace);
    await env.called(env.call(), (mock) => mock.mock.calls.some(([method, params]) =>
      method === "changes.subscribe" && params.subscription_id === "s-background"));
    expect(observer.held.has("s-background")).toBe(false);
    expect(wire.held.has("s-background")).toBe(true);
    expect(env.sync.passInFlight("dev-1")).toBeNull();
    expect(ready).toBe(false);
    trace.event("initial-complete-before-background-ack", { generation: wire.subscriptionGeneration });
    const red = process.env.READINESS_MODE === "red";
    if (red) {
      // Deliberately select a805's barrier semantics on the matched candidate
      // fixture; this is an overlay experiment, not an exact a805 gate run.
      await env.passesSettled(env.sync, "dev-1");
      await seed(env, trace);
      trace.event("a805-barrier-unqualified-seed", { managerHeld: [...observer.held] });
    } else expect(await canonicalSeed(env)).toBeUndefined();

    const successorRead = wire.hold("git.unpushed", (params) => params.run_id === "run-1");
    release.push(successorRead);
    const previousReads = env.calls("git.unpushed").length;
    background();
    trace.event("background-response-released");
    await env.called(env.call(), () => env.calls("git.unpushed").length > previousReads);
    expect(observer.held.has("s-background")).toBe(true);
    const successor = env.sync.passInFlight("dev-1");
    expect(successor).not.toBeNull();
    expect(ready).toBe(false);
    trace.event("successor-held", { pass: trace.identity(successor), generation: wire.subscriptionGeneration });
    const beforeReplay = wire.subscriptionGeneration;
    // Real manager replay, during barrier formation: no synthetic held event
    // or forced pass result. The read gate stays closed throughout replay.
    await env.greet();
    await env.changeEvents.subscriptionsSettledFor("dev-1");
    expect(wire.subscriptionGeneration).toBeGreaterThan(beforeReplay);
    expect(ready).toBe(false);
    trace.event("generation-invalidated-during-formation", { before: beforeReplay, after: wire.subscriptionGeneration });
    if (!red) expect(await canonicalSeed(env)).toBeUndefined();
    successorRead();
    trace.event("successor-read-released");
    await completePasses(env, successor, trace);
    await readiness;
    fixture.assertCurrent();
    trace.event("coverage-revalidated", { generation: wire.subscriptionGeneration });
    if (red) throw new Error("EXPECTED_READINESS_RED: a805 barrier admitted seed before manager coverage");
    await seed(env, trace);
    fixture.assertCurrent();
    await originalStandDown(env, wire, fixture, trace, release);
  } finally { fixture.dispose(); observer.dispose(); }
}

async function invalidationAfterCommit(env, trace, release, cleanup) {
  const wire = env.subscribingBridge();
  await env.greet();
  env.setBoard([env.branchItem()]);
  const observer = observe(env, wire, trace);
  cleanup.push(observer.dispose);
  const fixture = env.beginPushFixture(wire);
  cleanup.push(fixture.dispose);
  void fixture.ready.catch(() => {});
  try {
    const seal = await fixture.ready;
    fixture.assertCurrent();
    trace.event("barrier-ready", { session: trace.identity(seal.session), generation: seal.generation,
      passes: seal.passes.map(trace.identity) });
    await seed(env, trace);
    const before = wire.subscriptionGeneration;
    await env.greet();
    await env.changeEvents.subscriptionsSettledFor("dev-1");
    expect(wire.subscriptionGeneration).toBeGreaterThan(before);
    expect(() => fixture.assertCurrent()).toThrow();
    trace.event("invalidation-detected-after-seed-commit", { seedWasPrevented: false });
    // Drain observed repair work so cleanup never strands a deferred pass.
    await completePasses(env, env.sync.passInFlight("dev-1"), trace);
  } finally { fixture.dispose(); observer.dispose(); }
}

export function readinessCases(env) {
  for (const [name, run] of [["delayed-landing", delayedLanding], ["invalidation-after-commit", invalidationAfterCommit]]) {
    it(`909 readiness diagnostic ${name}`, async () => {
      const trace = capture(name);
      const release = [];
      const cleanup = [];
      const environment = env();
      const scope = diagnosticScope(environment, trace);
      let finished = false;
      emergencyCleanup = () => {
        if (!finished) scope.abort();
        for (const open of release.splice(0).reverse()) open();
        for (const dispose of cleanup.splice(0).reverse()) dispose();
        if (!finished) {
          try { trace.finish(new Error("INCONCLUSIVE: controller outlived test teardown")); }
          catch (error) { console.error(error); }
        }
      };
      let failure;
      try {
        trace.event("fixture-hooks-registered", { sourceHash: environment.sourceHash });
        await run(scope.environment, trace, release, cleanup);
      }
      catch (error) { failure = error; trace.event("raw-test-error", { error: String(error) }); throw error; }
      finally {
        finished = true;
        emergencyCleanup();
        // Capture failure is separate; never replace the original test error.
        try { trace.finish(failure); } catch (error) { if (failure) console.error(error); else throw error; }
      }
    });
  }
}
