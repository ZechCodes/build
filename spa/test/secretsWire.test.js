// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { sealSecretUpload, encryptFrame, decryptEnvelope } from "@build/secure-transport";
import { onSecretsChanged, secretsChangedOn } from "../src/core/secretMetadataEvents.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";
import { bridgeCapabilities, dispatchChangeEvent, greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";

it("resolves secret sealing and the original transport API through the package entry", () => {
  for (const method of [sealSecretUpload, encryptFrame, decryptEnvelope]) expect(method).toBeTypeOf("function");
});

it("keeps secret features off until the current greeting names each one", () => {
  expect(bridgeCapabilities("ungreeted-secrets").secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
  expect(v1.capabilitiesOf({ api_version: "3.36.0", capabilities: [] }).secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
  for (const feature of ["attachments", "sealedUpload", "metadataEvents"]) {
    const flags = v1.capabilitiesOf({ api_version: "3.36.0", capabilities: [`secrets.${feature}`] }).secrets;
    expect(flags[feature]).toBe(true);
    expect(Object.values(flags).filter(Boolean)).toHaveLength(1);
  }
  expect(v1.capabilitiesOf({
    api_version: "3.34.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  }).secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
});

it("recognizes the metadata invalidation without treating it as a reply", () => {
  const event = { type: "secrets.changed", scope: "project", project_id: "project-wire", revision: 1, secret_ids: ["sec_synthetic"] };
  expect(v1.parseEvent(event)).toBe(event);
  expect(dispatchChangeEvent(event, "ungreeted-secrets")).toBe(true);
});

afterEach(() => resetChangeEvents());
const projectEvent = { type: "secrets.changed", scope: "project", project_id: "project-wire", revision: 2, secret_ids: ["sec_synthetic"] };

it("routes project hints without their originating conversation and fences current greeting authority", async () => {
  const hear = vi.fn(), foreign = vi.fn();
  const stop = onSecretsChanged("project-device", hear), stopForeign = onSecretsChanged("other-device", foreign);
  dispatchChangeEvent(projectEvent, "project-device");
  expect(hear).not.toHaveBeenCalled();
  let current = true;
  await greetBridge(async () => ({ api_version: "3.36.0", push_events: true, capabilities: ["secrets.metadataEvents"], events: ["secrets.changed"] }),
    { deviceId: "project-device", isCurrent: () => current });
  dispatchChangeEvent({ ...projectEvent, name: "never forwarded", value: "synthetic value", sealed_payload: "synthetic envelope" }, "project-device");
  expect(hear).toHaveBeenCalledWith({ scope: "project", project_id: "project-wire", revision: 2, secret_ids: ["sec_synthetic"] });
  expect(foreign).not.toHaveBeenCalled();
  current = false;
  dispatchChangeEvent({ ...projectEvent, revision: 3 }, "project-device");
  expect(hear).toHaveBeenCalledTimes(1);
  stop(); stopForeign();
});

it("keeps reordered hints as refresh requests and refuses malformed scope metadata", () => {
  const hear = vi.fn(), stop = onSecretsChanged("hint-device", hear);
  const agent = { type: "secrets.changed", scope: "agent", entity_id: "owner", conversation_id: "conversation", thread_id: "thread", generation: "thread", revision: 4, secret_ids: ["sec_synthetic"] };
  secretsChangedOn("hint-device", agent);
  secretsChangedOn("hint-device", { ...agent, revision: 3 });
  expect(hear.mock.calls.map(([hint]) => hint.revision)).toEqual([4, 3]);
  for (const event of [{ ...agent, generation: "foreign-thread" }, { ...agent, thread_id: "" }, { ...agent, revision: 0 }, { ...agent, secret_ids: Array(101).fill("sec_synthetic") }, { ...agent, secret_ids: [] }, { ...agent, scope: "unknown" }]) {
    secretsChangedOn("hint-device", event);
  }
  expect(hear).toHaveBeenCalledTimes(2);
  secretsChangedOn("hint-device", { ...agent, secret_ids: [], truncated: true });
  expect(hear).toHaveBeenLastCalledWith({ scope: "agent", entity_id: "owner", conversation_id: "conversation", thread_id: "thread", generation: "thread", revision: 4, secret_ids: [], truncated: true });
  expect(hear).toHaveBeenCalledTimes(3);
  stop();
  secretsChangedOn("hint-device", agent);
  expect(hear).toHaveBeenCalledTimes(3);
});

it("turns a bounded device overflow hint into a full metadata refresh", () => {
  const hear = vi.fn();
  const dispose = onSecretsChanged("overflow-device", hear);
  secretsChangedOn("overflow-device", { type: "secrets.changed", scope: "device", revision: 1001, secret_ids: [], truncated: true, name: "discarded", value: "discarded" });
  expect(hear).toHaveBeenCalledWith({ scope: "device", revision: 1001, secret_ids: [], truncated: true });
  dispose();
});


it("advertises environment secrets independently and projects environment hints without values", () => {
  expect(v1.capabilitiesOf({ api_version: "3.40.0", capabilities: ["secrets.environmentScope"] }).environments.secrets).toBe(true);
  expect(v1.capabilitiesOf({ api_version: "3.39.0", capabilities: [] }).environments.secrets).toBe(false);
  const hear = vi.fn(), stop = onSecretsChanged("environment-wire", hear);
  secretsChangedOn("environment-wire", { scope: "environment", environment_id: "env-wire", revision: 3,
    secret_ids: ["sec_env"], value: "never forward", name: "never forward" });
  expect(hear).toHaveBeenCalledWith({ scope: "environment", environment_id: "env-wire", revision: 3, secret_ids: ["sec_env"] });
  secretsChangedOn("environment-wire", { scope: "environment", environment_id: "", revision: 4, secret_ids: ["sec_env"] });
  expect(hear).toHaveBeenCalledOnce(); stop();
});
