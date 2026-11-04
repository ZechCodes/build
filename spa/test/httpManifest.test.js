import { describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import manifest from "../../fixtures/api/http/resource-manifest.json";
import { createHttpRoutes } from "../src/core/httpRoutes.js";

describe("reviewed REST manifest consumption", () => {
  it("pins the coordinator-released bytes and exact operation IDs", () => {
    const bytes = readFileSync(new URL("../../fixtures/api/http/resource-manifest.json", import.meta.url));
    expect(createHash("sha256").update(bytes).digest("hex")).toBe("4069a7f03574f870c2de796d1a6486dc24edc01c86974b7460053fee1c6da996");
    expect(createHttpRoutes(manifest).operationIds()).toEqual(manifest.transport.operation_ids);
    expect(createHttpRoutes(manifest).operationIds()).toContain("tasks_review_retryDelivery");
    // Wire 3.34 changes device result rows, not route placement or operation IDs.
    // The reviewed manifest byte hash and count stay unchanged; OpenAPI refreshes hello.
    expect(manifest.operations).toHaveLength(153);
    for (const operation of ["tasks_review_snapshot", "tasks_review_act", "tasks_review_complete", "sync_head", "sync_snapshot", "sync_changes"]) {
      expect(createHttpRoutes(manifest).operationIds()).toContain(operation);
    }
  });

  it.each(manifest.operations)("preserves $method placement and byte limits", (operation) => {
    const params = Object.fromEntries(operation.pathParams.map((key) => [key, "value/a?#"]));
    if (operation.verb !== "GET") params.unknown_for_typed_validation = { preserve: true };
    const route = createHttpRoutes(manifest).request(operation.method, params);
    expect(route.operationId).toBe(operation.operationId);
    expect(route.method).toBe(operation.verb);
    expect(route.path).toBe(operation.path.replace(/\{[^}]+\}/g, "value%2Fa%3F%23"));
    expect(route.requestMaxBytes).toBe(operation.requestMaxBytes);
    expect(route.responseMaxBytes).toBe(operation.responseMaxBytes);
    expect(route.params).toEqual(params);
    if (operation.verb !== "GET") expect(route.body).toEqual({ unknown_for_typed_validation: { preserve: true } });
    else {
      expect(Object.hasOwn(route, "body")).toBe(false);
      expect(() => createHttpRoutes(manifest).request(operation.method, { ...params, extra: true })).toThrow();
    }
  });

  it("keeps terminal/signaling/QA exclusions out of the HTTP caller", () => {
    const routes = createHttpRoutes(manifest);
    for (const { method } of manifest.exclusions) expect(routes.has(method)).toBe(false);
    expect(manifest.operations.find((operation) => operation.method === "session.hello").requires_events).toBe(true);
  });
});
