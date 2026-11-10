// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
vi.mock("../src/core/localCache.js", () => import("./memoryCache.js"));
const state = vi.hoisted(() => ({ secrets: true }));
vi.mock("../src/app.js", () => ({ go: vi.fn(), App: { devices: [] } }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: () => ({ environments: { secrets: state.secrets },
  secrets: { attachments: true, sealedUpload: true, metadataEvents: true, environmentScope: state.secrets } }), onBridgeGreeted: () => () => {} }));
vi.mock("../src/core/environmentWrites.js", () => ({ environmentAction: async (_device, _flag, send) => {
  if (!state.secrets) throw new Error("Update Build"); return send();
} }));
import { createEnvironmentSettingsSecrets } from "../src/core/environmentSettingsSecrets.js";
import { createSecretMetadataCache } from "../src/core/secretMetadataCache.js";
import { readCached, writeCached } from "../src/core/localCache.js";
import { takeEnvironmentSecretNavigation } from "../src/core/environmentSecretNavigation.js";
import { notifyNavigationCommitted } from "../src/core/navigationLifetime.js";

const deviceId = "settings-secrets", environmentId = "env-settings";
const grant = { secret_id: "sec_env", request_id: "req", name: "DEPLOY_TOKEN", ttl_seconds: null,
  grant: { scope: "environment", environment_id: environmentId },
  scope: { owner_id: "owner", conversation_id: "agent", generation: "thread", draft_id: null },
  created_at: 1800000000, attached_at: 1800000001, expires_at: null, revoked_at: null, revision: 1, status: "available" };
const project = { project_id: "p1", name: "Release", environment_id: environmentId, conversations: [{ conversation_id: "coordinator" }] };
const projectSecret = { ...grant, secret_id: "sec_project", grant: { scope: "project", project_id: "p1" } };
const caches = [], sections = [];
const support = () => ({ attachments: true, sealedUpload: true, metadataEvents: true, environmentScope: state.secrets });
const seed = async (entry, scope) => {
  const cache = createSecretMetadataCache({ deviceId, call: vi.fn(), capabilities: support }); caches.push(cache);
  await cache.upsert(entry, scope);
};
async function mount(projects = [project], callRpc = vi.fn(() => new Promise(() => {}))) {
  await writeCached({ deviceId, entityId: "", kind: "environment-support" }, { secrets: state.secrets });
  await writeCached({ deviceId, entityId: "", kind: "projects" }, projects);
  const navigate = vi.fn((route, options) => { options?.beforeCommit?.(); notifyNavigationCommitted(); return true; });
  const section = createEnvironmentSettingsSecrets({ deviceId, environmentId, callRpc, navigate }); sections.push(section);
  section.mount(document.querySelector("main"));
  await vi.waitFor(() => expect(document.querySelector("[data-environment-secrets]") !== null).toBe(state.secrets));
  return { section, callRpc, navigate };
}
beforeEach(() => { state.secrets = true; document.body.innerHTML = "<main></main>"; });
afterEach(() => { sections.splice(0).forEach(section => section.dispose()); caches.splice(0).forEach(cache => cache.dispose()); document.body.innerHTML = ""; });

it("lists cached environment metadata while offline and shows same-name project overrides", async () => {
  await seed(grant, { environmentId }); await seed(projectSecret, { projectId: "p1" });
  await mount();
  await vi.waitFor(() => expect(document.querySelector("main").textContent).toContain("DEPLOY_TOKEN"));
  expect(document.querySelector("main").textContent).toContain("overrides environment");
  expect(document.querySelector("main").textContent).toContain("Release");
});

it("hides the whole section on an old bridge even with cached secret metadata", async () => {
  state.secrets = false; await seed(grant, { environmentId });
  const { callRpc } = await mount();
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(document.querySelector("main").textContent).toBe(""); expect(callRpc).not.toHaveBeenCalled();
});

it("revokes immediately with the environment selector and paints the cache acknowledgment", async () => {
  await seed(grant, { environmentId });
  const callRpc = vi.fn(async method => {
    if (method === "secrets.revoke") return { ...grant, status: "revoked", revision: 2, revoked_at: 1800000010 };
    return new Promise(() => {});
  });
  await mount([project], callRpc);
  await vi.waitFor(() => expect(document.querySelector("[data-secret-revoke]")).not.toBeNull());
  document.querySelector("[data-secret-revoke]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("secrets.revoke", { scope: { environment_id: environmentId }, secret_id: grant.secret_id, operation_id: expect.any(String) }));
  await vi.waitFor(() => expect(document.querySelector("main").textContent).toContain("revoked"));
});

it("audits with the environment selector and caches only explicit audit metadata", async () => {
  await seed(grant, { environmentId });
  const callRpc = vi.fn(async method => method === "secrets.audit" ? { events: [{ sequence: 1, secret_id: grant.secret_id,
    agent_id: "agent", occurred_at: 1800000002, outcome: "released", value: "must-not-persist", sealed_payload: "must-not-persist" }], next: null, server_now: 1800000002 } : new Promise(() => {}));
  await mount([project], callRpc);
  await vi.waitFor(() => expect(document.querySelector("[data-secret-audit]")).not.toBeNull()); document.querySelector("[data-secret-audit]").click();
  await vi.waitFor(() => expect(callRpc).toHaveBeenCalledWith("secrets.audit", { scope: { environment_id: environmentId }, secret_id: grant.secret_id, after: 0, limit: 100 }));
  await vi.waitFor(() => expect(document.querySelector("main").textContent).toContain("released"));
  const record = await readCached({ deviceId, entityId: grant.secret_id, kind: "secret-audit", sub: environmentId });
  expect(JSON.stringify(record)).not.toContain("must-not-persist");
});

it("Add secret navigates directly for one active member and performs no secret write", async () => {
  const { callRpc, navigate } = await mount([project, { ...project, project_id: "archived", archived_at: 123 }, { ...project, project_id: "foreign", environment_id: "foreign" }]);
  document.querySelector("[data-secret-add]").click();
  expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ name: "project", projectId: "p1", deviceId, agent: "coordinator" }), expect.any(Object));
  expect(takeEnvironmentSecretNavigation({ kind: "project", deviceId, projectId: "p1" })).toEqual({ deviceId, projectId: "p1", environmentId });
  expect(callRpc.mock.calls.some(([method]) => ["secrets.create", "thread.post"].includes(method))).toBe(false);
});

it("Add secret lets the user choose among several cached active members", async () => {
  const { navigate } = await mount([project, { ...project, project_id: "p2", name: "Hotfix" }]);
  document.querySelector("[data-secret-add]").click();
  expect(navigate).not.toHaveBeenCalled();
  const choices = document.querySelectorAll("[data-secret-project]"); expect(choices).toHaveLength(2);
  choices[1].click(); expect(navigate).toHaveBeenCalledWith(expect.objectContaining({ projectId: "p2" }), expect.any(Object));
});

it("disables Add secret with a plain explanation when there are no active member projects", async () => {
  await mount([{ ...project, archived_at: 123 }]);
  expect(document.querySelector("[data-secret-add]").disabled).toBe(true);
  expect(document.querySelector("main").textContent).toContain("An environment secret is added from a project in this environment.");
});

it("refreshes cached environment membership when a task rail learns its project later", async () => {
  const { watchSecretEnvironment } = await import("../src/core/secretEnvironmentContext.js");
  await writeCached({ deviceId, entityId: "", kind: "environment-support" }, { secrets: true });
  await writeCached({ deviceId, entityId: "", kind: "projects" }, [project]);
  let projectId = null;
  const changed = vi.fn();
  const watcher = watchSecretEnvironment({ deviceId, projectId: () => projectId, onChange: changed });
  try {
    await watcher.refresh(); expect(changed).toHaveBeenLastCalledWith(null);
    projectId = "p1"; await watcher.refresh(); expect(changed).toHaveBeenLastCalledWith(environmentId);
    await writeCached({ deviceId, entityId: "", kind: "environment-support" }, { secrets: false });
    await watcher.refresh(); expect(changed).toHaveBeenLastCalledWith(null);
  } finally { watcher.dispose(); }
});
