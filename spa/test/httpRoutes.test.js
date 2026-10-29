import { describe, expect, it } from "vitest";
import { createHttpRoutes } from "../src/core/httpRoutes.js";

const manifest = { version: 1, operations: [
  { method: "tasks.get", operationId: "getTask", verb: "POST", path: "/v1/tasks/{task_id}/queries", pathParams: ["task_id"], queryParams: [], body: "remaining" },
  { method: "ping", operationId: "ping", verb: "GET", path: "/v1/health", pathParams: [], queryParams: [], body: "remaining" },
], exclusions: [{ method: "term.input", reason: "WebRTC only" }] };

describe("explicit REST route adapter", () => {
  it("encodes path segments and keeps typed validation fields in JSON", () => {
    expect(createHttpRoutes(manifest).request("tasks.get", { task_id: "a/b?#", filters: { state: ["open"] }, unknown: 7 })).toEqual({
      operationId: "getTask", method: "POST", path: "/v1/tasks/a%2Fb%3F%23/queries", body: { filters: { state: ["open"] }, unknown: 7 },
    });
  });
  it("refuses unknown methods, missing selectors and GET bodies locally", () => {
    const routes = createHttpRoutes(manifest);
    for (const [method, params] of [["term.input", {}], ["arbitrary", {}], ["tasks.get", {}], ["ping", { extra: true }]]) {
      expect(() => routes.request(method, params)).toThrow();
    }
  });
  it("does not let a manifest create a proxy, query tunnel or duplicate entry", () => {
    for (const change of [{ path: "https://evil.example" }, { path: "/v1/rpc?method=x" }, { verb: "CONNECT" }, { pathParams: ["missing"] }]) {
      expect(() => createHttpRoutes({ ...manifest, operations: [{ ...manifest.operations[0], ...change }] })).toThrow();
    }
    expect(() => createHttpRoutes({ ...manifest, operations: [...manifest.operations, manifest.operations[0]] })).toThrow();
  });
});
