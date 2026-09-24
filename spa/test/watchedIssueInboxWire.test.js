// @vitest-environment jsdom
// #125, end to end: an agent's real MCP tools/call changes a watched issue,
// the bridge pushes it on the inbox subscription, and the rail shows the row
// while the issue needs the user and drops it when it does not.
//
// The wire is a Rust AppState run (bridge/src/app/tests/tracker_inbox_wire.rs,
// `watched_issue_inbox_wire_probe`): the MCP stdio server takes the tool call,
// the real change window is flushed at an `s-inbox` subscription, and the
// decrypted pushes are printed beside what `issues.list` and `issues.get`
// answered after each step. Here nothing is stood in for between that wire and
// the screen: the greeting arms the real change router, the real sync layer
// holds the real subscription, its `issues` applier re-reads the list into
// the real cache, and the mounted rail paints from that cache. The session's
// `call` answers with the bridge's own recorded answers for the current step.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bridgeRoot = resolve(process.cwd(), "../bridge");
const probeBinary = process.env.BRIDGE_INBOX_PROBE_BIN;
const probeArgs = probeBinary
  ? ["watched_issue_inbox_wire_probe", "--nocapture"]
  : ["test", "--lib", "watched_issue_inbox_wire_probe", "--", "--nocapture"];

function runProbe() {
  const output = execFileSync(probeBinary || "cargo", probeArgs, {
    cwd: bridgeRoot, encoding: "utf8", timeout: 900_000, maxBuffer: 20 * 1024 * 1024,
  });
  const encoded = output.match(/WATCHED_ISSUE_INBOX_WIRE=(\{[^\r\n]+\})/)?.[1];
  if (!encoded) throw new Error(`the probe printed no wire:\n${output.slice(-2000)}`);
  return JSON.parse(encoded);
}

const DEVICE = "inbox-wire-device";
const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];
const WAIT = { timeout: 10_000, interval: 20 };

let wire;
// What the bridge answers right now: the step the case has reached, and every
// issue's newest record as of that step.
const bridgeNow = { list: null, records: new Map() };
const asked = [];
let modules;

const rowsNamed = (issueId) => [...document.querySelectorAll("#inbox-list .inbox-entry")]
  .filter((row) => row.dataset.key === `tracker_issue:${issueId}`);
const rowFor = (issueId) => rowsNamed(issueId)[0] || null;
const listReads = () => asked.filter((one) => one.method === "issues.list").length;

/** The bridge's own answers, as of the current step. */
async function call(method, params = {}) {
  asked.push({ method, params });
  if (method === "session.hello") return structuredClone(wire.greeting);
  if (method === "issues.list") return structuredClone(bridgeNow.list);
  if (method === "issues.get") return structuredClone(bridgeNow.records.get(params.issue_id));
  if (method === "issues.columns") return { columns: [] };
  if (method === "board.list") return { items: [], runs: [], pending: [] };
  if (method === "project.list") return { projects: [{ project_id: wire.project_id, name: "repo", is_git: true }] };
  if (method === "workspace.list") return { workspaces: [] };
  return {};
}

/** Take the bridge to one recorded step, then deliver that step's pushes to
 *  the real router — the only thing this test does to the SPA. Answers once
 *  the push has made the sync layer read the list again, so what the rail
 *  shows next came through the push. */
async function reach(step) {
  const before = listReads();
  bridgeNow.list = step.list;
  bridgeNow.records.set(step.get.issue.id, step.get);
  for (const event of step.events) modules.changeEvents.dispatchChangeEvent(event, DEVICE);
  if (step.events.length) await vi.waitFor(() => expect(listReads()).toBeGreaterThan(before), WAIT);
}

beforeAll(async () => {
  wire = runProbe();
  for (const kase of [wire.review, wire.comment]) await reach({ ...kase.steps[0], events: [] });
  document.body.innerHTML = bodyHtml;
  const { App } = await import("../src/app.js");
  App.route = { name: "inbox" };
  App.devices = [{ id: DEVICE, name: "Laptop", status: "online" }];
  App.selectedDeviceId = DEVICE;
  App.deviceFilter = null;
  const deviceContexts = await import("../src/core/deviceContexts.js");
  const { greetLiveBridge } = await import("../src/connection.js");
  const changeEvents = await import("../src/core/changeEvents.js");
  const cacheSync = await import("../src/core/cacheSync.js");
  const taskFeed = await import("../src/core/taskFeed.js");
  const inboxView = await import("../src/core/inboxView.js");
  modules = { changeEvents, cacheSync, taskFeed, inboxView, deviceContexts };
  const context = deviceContexts.adoptDeviceSession({
    deviceId: DEVICE, call, close: () => {}, peer: () => {}, onCarrier: () => {},
    installAdapter: (selection) => selection.create(call),
  });
  await greetLiveBridge(context);
  inboxView.setInboxView("inbox");
  inboxView.mountInboxList();
  cacheSync.startCacheSync();
  await taskFeed.startFeed();
  // The first pass ends by taking out the inbox subscription; a push sent
  // before the bridge holds it is a push nobody would have been sent.
  await vi.waitFor(() => expect(asked.some((one) => one.method === "changes.subscribe"
    && one.params.subscription_id === wire.subscription_id)).toBe(true), WAIT);
  await changeEvents.subscriptionsSettled();
}, 930_000);

afterAll(() => {
  modules?.inboxView.unmountInboxList();
  modules?.cacheSync.stopCacheSync();
  modules?.taskFeed.stopFeed();
  modules?.changeEvents.resetChangeEvents();
  modules?.deviceContexts.resetDeviceContexts();
});

const detailReads = (issueId) =>
  asked.filter((one) => one.method === "issues.get" && one.params.issue_id === issueId).length;

/** Reach a step that should draw no row, and hold that until the issue's own
 *  record has been read again: a row the new timeline would add is added by
 *  then, so its absence means something. */
async function reachQuietly(step, issueId) {
  const before = detailReads(issueId);
  await reach(step);
  await vi.waitFor(() => expect(detailReads(issueId)).toBeGreaterThan(before), WAIT);
  expect(rowsNamed(issueId)).toHaveLength(0);
}

describe("a watched issue in the inbox, over the real wire", () => {
  it("names the narrow Needs you rule in the greeting (#144)", () => {
    expect(wire.greeting.capabilities).toContain("issues.commentUserNotifies");
  });

  it("stays out while agents move it to In review, appears once it is assigned to the user, leaves at Done", async () => {
    const { issue_id: issueId, steps: [filed, inReview, assigned, done] } = wire.review;
    // Filed and watched, in Backlog: nothing for the user yet.
    expect(filed.list.issues.find((one) => one.id === issueId)).toMatchObject({ watched: true, status: "backlog" });
    expect(rowFor(issueId)).toBe(null);

    // In review between agents is not the user's (#144).
    await reach(inReview);
    expect(rowsNamed(issueId)).toHaveLength(0);

    await reach(assigned);
    await expect.poll(() => rowFor(issueId)?.querySelector(".inbox-facts")?.textContent, WAIT).toBe("Assigned to you");
    const number = assigned.get.issue.number;
    expect(rowFor(issueId).querySelector(".stitle").textContent).toBe(`#${number} ${assigned.get.issue.title}`);

    await reach(done);
    await expect.poll(() => rowsNamed(issueId).length, WAIT).toBe(0);
  });

  it("stays out for agents' own comments, appears when an agent asks the user, and leaves once it is read", async () => {
    const { issue_id: issueId, steps: [, chatter, commented, read] } = wire.comment;
    expect(rowFor(issueId)).toBe(null);

    await reachQuietly(chatter, issueId);

    await reach(commented);
    await expect.poll(() => rowFor(issueId)?.querySelector(".inbox-facts")?.textContent, WAIT).toBe("New comment");
    // The question counts; the agents' own comment before it does not.
    expect(rowFor(issueId).querySelector(".inbox-unread")?.textContent).toBe("1");

    await reach(read);
    await expect.poll(() => rowsNamed(issueId).length, WAIT).toBe(0);
  });
});
