/** @vitest-environment jsdom */
import { afterEach, expect, it, vi } from "vitest";
import {
  adoptBridgeSelection, adoptDeviceSession, greetingInFlight,
  releaseGreeting, resetDeviceContexts, whenGreeted,
} from "../src/core/deviceContexts.js";
import { fakeSession } from "./deviceSessionFixture.js";

afterEach(resetDeviceContexts);

it("gives every greeting its own authority, and transfers existing waits to the newest", async () => {
  const context = adoptDeviceSession(fakeSession("dev-a"));
  const released = vi.fn();
  const adoption = context.greeted;
  adoption.then(released);
  const first = greetingInFlight(context);
  expect(context.greeted).toBe(adoption);
  const second = greetingInFlight(context);
  expect(second).not.toBe(first);
  releaseGreeting(context, first);
  await Promise.resolve();
  expect(released).not.toHaveBeenCalled();
  adoptBridgeSelection(context, { version: "1.22.0" }, {}, second);
  await vi.waitFor(() => expect(released).toHaveBeenCalledTimes(1));
});

it("does not install or dispatch on an obsolete greeting's compatible answer", async () => {
  const context = adoptDeviceSession(fakeSession("dev-a"));
  const first = greetingInFlight(context);
  const second = greetingInFlight(context);
  const dispatch = vi.fn();
  const waiting = whenGreeted(context, dispatch);
  adoptBridgeSelection(context, { version: "1.22.0" }, {}, first);
  await Promise.resolve();
  expect(context.apiVersion).toBe(null);
  expect(dispatch).not.toHaveBeenCalled();
  adoptBridgeSelection(context, { version: "99.0.0", unsupported: "app" }, null, second);
  expect(await waiting).toBe(null);
  expect(dispatch).not.toHaveBeenCalled();
});

it("a released wait without a compatible verdict grants no dispatch authority", async () => {
  const context = adoptDeviceSession(fakeSession("dev-a"));
  const first = greetingInFlight(context);
  const dispatch = vi.fn();
  const waiting = whenGreeted(context, dispatch);
  releaseGreeting(context, first);
  expect(await waiting).toBe(null);
  expect(dispatch).not.toHaveBeenCalled();
});

it("revokes a sent request's authority as soon as a newer greeting starts", async () => {
  const context = adoptDeviceSession(fakeSession("dev-a"));
  const first = greetingInFlight(context);
  adoptBridgeSelection(context, { version: "1.22.0" }, {}, first);
  const asking = await whenGreeted(context, () => Promise.resolve("answer"));
  expect(asking.stands()).toBe(true);
  const second = greetingInFlight(context);
  expect(asking.stands()).toBe(false);
  adoptBridgeSelection(context, { version: "1.22.0" }, {}, second);
  expect(asking.stands()).toBe(false);
  expect((await whenGreeted(context, () => null)).stands()).toBe(true);
});

it.each(["project.ensure_conversation", "workspace.ensure_conversation"])(
  "%s waits for the latest verdict through both the surface and repository callers",
  async (method) => {
    const session = fakeSession("dev-a");
    const context = adoptDeviceSession(session);
    adoptBridgeSelection(context, { version: "1.22.0" }, {});
    const pending = greetingInFlight(context);
    const sends = [context.rpc, context.chatRepository.currentCall()].map((call) =>
      call(method, {}).catch((error) => error));
    await Promise.resolve();
    await Promise.resolve();
    expect(session.call).not.toHaveBeenCalled();
    adoptBridgeSelection(context, { version: "99.0.0", unsupported: "app" }, null, pending);
    expect((await Promise.all(sends)).every((answer) => answer instanceof Error)).toBe(true);
    expect(session.call).not.toHaveBeenCalled();
  },
);

it("does not return a minted owner after its greeting has been superseded", async () => {
  const session = fakeSession("dev-a");
  let answer;
  session.call.mockImplementation(() => new Promise((resolve) => { answer = resolve; }));
  const context = adoptDeviceSession(session);
  adoptBridgeSelection(context, { version: "1.22.0" }, {});
  const request = context.rpc("project.ensure_conversation", {}).catch((error) => error);
  await vi.waitFor(() => expect(answer).toBeTypeOf("function"));
  const newer = greetingInFlight(context);
  adoptBridgeSelection(context, { version: "99.0.0", unsupported: "app" }, null, newer);
  answer({ entity_id: "superseded-owner" });
  expect(await request).toBeInstanceOf(Error);
});
