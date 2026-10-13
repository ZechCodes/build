// @vitest-environment jsdom
import { beforeEach, expect, it } from "vitest";
import { adoptBridgeSelection, adoptDeviceSession, greetingInFlight, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";

beforeEach(() => resetDeviceContexts());
const select = (context, waitingOn, authority) => adoptBridgeSelection(context,
  { version: "3.21.0", unsupported: null }, { capabilities: { tasks: { waitingOn } } }, authority);

it("recognizes waiting-on by announcement rather than version", () => {
  expect(capabilitiesOf({ api_version: "3.21.0", capabilities: [] }).tasks.waitingOn).toBe(false);
  expect(capabilitiesOf({ api_version: "3.21.0", capabilities: ["tasks.waitingOn"] }).tasks.waitingOn).toBe(true);
});

it("never sends new dependency verbs or parameters before the current greeting", async () => {
  const session = fakeSession("waiting-dispatch");
  const context = adoptDeviceSession(session);
  const params = { task_id: "t", waiting_on: ["a"] };
  const request = context.rpc("tasks.waiting_on.add", params);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(session.call).not.toHaveBeenCalled();
  select(context, true);
  await request;
  expect(session.call).toHaveBeenCalledWith("tasks.waiting_on.add", params);
});

it("blocks every new shape on an old current bridge and leaves legacy assignments alone", async () => {
  const session = fakeSession("old-waiting-dispatch");
  const context = adoptDeviceSession(session);
  select(context, false);
  for (const [method, params] of [
    ["tasks.waiting_on.add", { task_id: "t", waiting_on: ["a"] }],
    ["tasks.waiting_on.remove", { task_id: "t", waiting_on: ["a"] }],
    ["tasks.waiting_on.blocks", { task_id: "t" }],
    ["tasks.create", { project_id: "p", title: "Work", waiting_on: [] }],
    ["tasks.assign", { task_id: "t", retry_deferred_id: "saved-intent" }],
  ]) await expect(context.rpc(method, params)).rejects.toThrow(/Update.*bridge/);
  expect(session.call).not.toHaveBeenCalled();
  const legacy = { task_id: "t", assignee: { kind: "user" } };
  await context.rpc("tasks.assign", legacy);
  expect(session.call).toHaveBeenCalledWith("tasks.assign", legacy);
});

it("a re-greeting to an old bridge supersedes cached capable dispatch authority", async () => {
  const session = fakeSession("re-greet-waiting-dispatch");
  const context = adoptDeviceSession(session);
  select(context, true);
  const authority = greetingInFlight(context);
  const request = context.rpc("tasks.assign", { task_id: "t", retry_deferred_id: "saved-intent" });
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(session.call).not.toHaveBeenCalled();
  select(context, false, authority);
  await expect(request).rejects.toThrow(/Update.*bridge/);
  expect(session.call).not.toHaveBeenCalled();
});
