import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { selectAdapter, SPA_API_RANGE } from "../src/core/bridgeApi/index.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";

const fixture = (name) => JSON.parse(readFileSync(new URL(`../../fixtures/api/v1/writes/${name}.json`, import.meta.url), "utf8"));
const greeting = (over = {}) => ({ ...fixture("epoch-greeting").result, ack_confirm_methods: ["tasks.comment"], ...over });

describe("the coordinated write protocol adapter", () => {
  it("requires the coordinated release, including a real versioned greeting", () => {
    expect(SPA_API_RANGE).toBe(">=3.24.0 <4.0.0");
    expect(v1.range).toBe(SPA_API_RANGE);
    for (const api_version of ["2.99.0", "3.23.99", undefined]) {
      expect(selectAdapter({ api_version }).unsupported).toBe("bridge");
    }
    expect(selectAdapter(greeting()).major).toBe(3);
    expect(selectAdapter({ api_version: "4.0.0" }).unsupported).toBe("app");
  });

  it("keeps the 3.24 foundation greeting free of write metadata", async () => {
    const submit = vi.fn();
    const call = vi.fn(async () => ({ saved: true }));
    const adapter = selectAdapter(fixture("epoch-greeting").result).create(call, { writeOperations: { submit } });
    await expect(adapter.call("tasks.comment", { task_id: "task", body: "Hello" }))
      .resolves.toEqual({ saved: true });
    expect(adapter.capabilities.writes.ackConfirmMethods).toEqual([]);
    expect(submit).not.toHaveBeenCalled();
    expect(call.mock.calls[0][2]?.write).toBeUndefined();
  });

  it("submits an allowlisted write with its draft and caller options, returning acceptance", async () => {
    const call = vi.fn();
    const ack = fixture("ack");
    const submit = vi.fn(async () => ack.result);
    const request = fixture("request");
    const options = { operationId: ack.write.operation_id, draft: { body: request.params.body },
      owner: { kind: "task", id: request.params.task_id }, baseRevision: 7, after: ["previous"], priority: "foreground" };
    const adapter = selectAdapter(greeting()).create(call, { writeOperations: { submit } });
    await expect(adapter.call(request.method, request.params, options)).resolves.toEqual({
      ...ack.result, phase: "ack", operation_id: ack.write.operation_id,
    });
    expect(submit).toHaveBeenCalledExactlyOnceWith({ method: request.method, params: request.params, options,
      operationId: options.operationId, draft: options.draft, owner: options.owner,
      baseRevision: 7, after: ["previous"], priority: "foreground" });
    expect(call).not.toHaveBeenCalled();
    expect(request.write).not.toHaveProperty("mode");
    expect(greeting().capabilities).not.toContain("writes.ackConfirm");
  });

  it("uses thread.post's operation_id as the durable outer operation identity", async () => {
    const submit = vi.fn(async () => ({ accepted_at: "now" }));
    const params = { entity_id: "entity", operation_id: "thread-operation", body: "Hello" };
    const adapter = v1.create(vi.fn(), greeting({ ack_confirm_methods: ["thread.post"] }), { writeOperations: { submit } });
    await expect(adapter.call("thread.post", params, { operationId: "ignored" }))
      .resolves.toMatchObject({ phase: "ack", operation_id: "thread-operation" });
    expect(submit.mock.calls[0][0]).toMatchObject({ params, operationId: "thread-operation" });
  });

  it("generates one durable operation identity before submission when the caller has none", async () => {
    const submit = vi.fn(async () => ({}));
    const adapter = v1.create(vi.fn(), greeting(), { writeOperations: { submit } });
    const result = await adapter.call("tasks.comment", { task_id: "task", body: "Hello" });
    expect(result.operation_id).toMatch(/^[a-f0-9-]{36}$/);
    expect(submit.mock.calls[0][0].operationId).toBe(result.operation_id);
  });

  it("uses only the current greeting's epoch capability and exact method list", async () => {
    const call = vi.fn(async () => ({ unchanged: true }));
    const submit = vi.fn();
    for (const current of [greeting({ capabilities: [] }), greeting({ ack_confirm_methods: ["tasks.comments"] })]) {
      const adapter = v1.create(call, current, { writeOperations: { submit } });
      await expect(adapter.call("tasks.comment", {})).resolves.toEqual({ unchanged: true });
    }
    expect(submit).not.toHaveBeenCalled();
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("refuses an allowlisted write locally when its shared service was not installed", async () => {
    const call = vi.fn();
    const adapter = v1.create(call, greeting());
    await expect(adapter.call("tasks.comment", {})).rejects.toMatchObject({ code: "unavailable" });
    expect(call).not.toHaveBeenCalled();
  });

  it("keeps reads and exempt session controls on their ordered raw path", async () => {
    const exempt = ["session.hello", "rtc.candidate", "changes.subscribe", "writes.subscribe", "stream.start",
      "term.attach", "agent.attach", "term.input", "term.resize", "term.ack"];
    const call = vi.fn(async () => ({ ok: true }));
    const submit = vi.fn();
    const adapter = v1.create(call, greeting({ ack_confirm_methods: exempt }), { writeOperations: { submit } });
    for (const method of [...exempt, "tasks.get"]) await adapter.call(method, {}, { priority: "background" });
    expect(submit).not.toHaveBeenCalled();
    expect(call).toHaveBeenCalledTimes(exempt.length + 1);
    expect(adapter.capabilities.writes).toEqual({ epoch: true, ackConfirmMethods: [] });
  });

  it("keeps only actual exact method names from the advertised allowlist", () => {
    const flags = v1.capabilitiesOf(greeting({ ack_confirm_methods: ["tasks.comment", null, {}, "tasks.*", "tasks.comment"] }));
    expect(flags.writes).toEqual({ epoch: true, ackConfirmMethods: ["tasks.comment"] });
    expect(v1.capabilitiesOf(null).writes).toEqual({ epoch: false, ackConfirmMethods: [] });
  });

  it("normalizes shared-service refusals and recognizes write lifecycle pushes", async () => {
    const adapter = v1.create(vi.fn(), greeting(), { writeOperations: { submit: async () => {
      throw { error_code: "stale_body", error: "The task changed" };
    } } });
    await expect(adapter.call("tasks.comment", {})).rejects.toMatchObject({ code: "stale_body", message: "The task changed" });
    expect(v1.parseEvent(fixture("confirmed-success"))).not.toBeNull();
    expect(v1.parseEvent({ type: "bridge.restarting", bridge_epoch: greeting().bridge_epoch })).not.toBeNull();
  });
});
