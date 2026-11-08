import { describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import manifest from "../../fixtures/api/http/resource-manifest.json";
import { createHttpRoutes } from "../src/core/httpRoutes.js";

describe("reviewed REST manifest consumption", () => {
  it("pins the coordinator-released bytes and exact operation IDs", () => {
    const bytes = readFileSync(new URL("../../fixtures/api/http/resource-manifest.json", import.meta.url));
    expect(createHash("sha256").update(bytes).digest("hex")).toBe("05819d239f2e2cdd98c0a8350a9b67d907a06e41692d8876f68680022afe12fe");
    expect(createHttpRoutes(manifest).operationIds()).toEqual(manifest.transport.operation_ids);
    expect(createHttpRoutes(manifest).operationIds()).toContain("tasks_review_retryDelivery");
    // M1.1 adds the environment list route before capability activation.
    expect(manifest.operations).toHaveLength(160);
    for (const operation of ["environment_list", "tasks_review_snapshot", "tasks_review_act", "tasks_review_complete", "sync_head", "sync_snapshot", "sync_changes", "secrets_create", "secrets_list", "secrets_revoke", "secrets_audit"]) {
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

  it("caps sealed uploads and retains metadata-only query placement", () => {
    const operation = manifest.operations.find(({ method }) => method === "secrets.create");
    expect(operation).toMatchObject({ verb: "POST", path: "/v1/secrets", requestMaxBytes: 65536 });
    expect(manifest.operations.find(({ method }) => method === "secrets.list").path).toBe("/v1/secret-queries");
    expect(manifest.operations.find(({ method }) => method === "secrets.revoke").path).toBe("/v1/secrets/{secret_id}/revocations");
    expect(manifest.operations.find(({ method }) => method === "secrets.audit").path).toBe("/v1/secrets/{secret_id}/audit-queries");
  });

  it("keeps terminal/signaling/QA exclusions out of the HTTP caller", () => {
    const routes = createHttpRoutes(manifest);
    for (const { method } of manifest.exclusions) expect(routes.has(method)).toBe(false);
    expect(manifest.operations.find((operation) => operation.method === "session.hello").requires_events).toBe(true);
  });
});
