// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { compactionSupportAddress } from "../src/core/compactionSupport.js";
import { notePush } from "../src/core/pushFence.js";
import { deviceSettingsAddress, projectSettingsAddress, workspaceSettingsAddress } from "../src/core/settingsRecords.js";
import { mountScopedCompactionEditor, scopedCompactionEditorHtml } from "../src/core/scopedCompactionEditor.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const providers = ["claude", "claude_adk", "codex", "codex_app_server", "pi"];
const addresses = { device: () => deviceSettingsAddress("device"), project: () => projectSettingsAddress("device", "owner"), workspace: () => workspaceSettingsAddress("device", "owner") };
const observation = (tokens, kind = "device") => ({ tokens, source: { kind } });
const settings = (tokens = 150000) => ({ name: "Owner", project_id: "owner", workspace_id: "owner", compaction_thresholds: { codex: tokens }, compaction: {
  inherited: Object.fromEntries(providers.map((provider) => [provider, observation(200000)])),
  effective: Object.fromEntries(providers.map((provider) => [provider, observation(provider === "codex" ? tokens : 200000)])),
} });
let controller;
const row = (provider = "codex") => document.querySelector(`[data-compaction-provider="${provider}"]`);
const input = () => row().querySelector("input");
const status = () => row().querySelector('[role="status"]');
const draft = (value) => { input().value = value; input().dispatchEvent(new Event("input", { bubbles: true })); };
const save = () => row().querySelector('[data-compaction-action="save"]').click();
async function mount({ scope = "project", value = settings(), support = true, callRpc = vi.fn() } = {}) {
  if (value) await writeCached(addresses[scope](), value);
  await writeCached(compactionSupportAddress("device"), { harnessThresholds: support });
  document.body.innerHTML = scopedCompactionEditorHtml({ scope, prefix: "test" });
  controller = mountScopedCompactionEditor(document.body, { scope, ownerId: "owner", deviceId: "device", prefix: "test", callRpc });
  await controller.read();
  return callRpc;
}
beforeEach(async () => { await wipeCache(); });
afterEach(() => { controller?.dispose(); controller = null; document.body.innerHTML = ""; });

it("paints all five harness rows from cache without asking the wire", async () => {
  const call = await mount();
  expect(document.querySelectorAll("[data-compaction-provider]")).toHaveLength(5);
  expect(input().value).toBe("150000");
  expect(row().textContent).toContain("Inherit — 200k tokens from device");
  expect(call).not.toHaveBeenCalled();
});

it("keeps the same cold editor nodes when settings arrive", async () => {
  await mount({ value: null });
  const held = input();
  expect(held.value).toBe("");
  await writeCached(addresses.project(), settings());
  await vi.waitFor(() => expect(input().value).toBe("150000"));
  expect(input()).toBe(held);
});

it("retains unsaved text, focus, and selection across push and greeting writes", async () => {
  await mount();
  const held = input();
  draft("345678"); held.focus(); held.setSelectionRange(2, 4);
  await writeCached(addresses.project(), settings(250000));
  await writeCached(compactionSupportAddress("device"), { harnessThresholds: true });
  await controller.read();
  expect(input()).toBe(held);
  expect(input().value).toBe("345678");
  expect(document.activeElement).toBe(held);
  expect([held.selectionStart, held.selectionEnd]).toEqual([2, 4]);
});

it.each(["device", "project", "workspace"])("saves %s thresholds through the supplied adapter and paints its answer", async (scope) => {
  const call = vi.fn(async () => settings(175000));
  await mount({ scope, callRpc: call });
  draft("180000"); save();
  const method = { device: "settings.set", project: "project.set_compaction", workspace: "workspace.set_compaction" }[scope];
  const owner = scope === "device" ? {} : { [`${scope}_id`]: "owner" };
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith(method, { ...owner, compaction_thresholds: { codex: 180000 } }));
  await vi.waitFor(() => expect(input().value).toBe("175000"));
  expect((await readCached(addresses[scope]())).value.compaction_thresholds.codex).toBe(175000);
});

it("sends null for inherit, without translating it to zero", async () => {
  const answer = settings(); delete answer.compaction_thresholds.codex;
  const call = await mount({ callRpc: vi.fn(async () => answer) });
  row().querySelector('[data-compaction-action="inherit"]').click();
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("project.set_compaction", { project_id: "owner", compaction_thresholds: { codex: null } }));
  await vi.waitFor(() => expect(input().value).toBe(""));
});

it.each(["0", "-1", "1.5", "9007199254740992", "1e5", ""])("rejects unsafe token text %j before dispatch", async (value) => {
  const call = await mount(); draft(value); save();
  expect(status().textContent).toContain("positive whole number");
  expect(call).not.toHaveBeenCalled();
});

it("shows decimal-string observations exactly and never rounds them into an editable number", async () => {
  const value = settings();
  value.compaction_thresholds.codex = { tokens_decimal: "18446744073709551615" };
  value.compaction.effective.codex = { tokens_decimal: "18446744073709551615", source: { kind: "project" } };
  await mount({ value });
  expect(row().textContent.replaceAll(",", "")).toContain("18446744073709551615");
  expect(input().value).toBe("18446744073709551615");
  save(); expect(status().textContent).toContain("positive whole number");
});

it("retains a pending draft while a newer push wins over the late save answer", async () => {
  let finish;
  await mount({ callRpc: () => new Promise((resolve) => { finish = resolve; }) });
  draft("180000"); save();
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  await writeCached(addresses.project(), settings(250000));
  await controller.read(); expect(input().value).toBe("180000");
  finish(settings(180000));
  await vi.waitFor(() => expect(input().value).toBe("250000"));
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(250000);
});

it("does not write an answer after invalidation even before the fresh read arrives", async () => {
  let finish;
  await mount({ callRpc: () => new Promise((resolve) => { finish = resolve; }) });
  draft("180000"); save(); await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  notePush(addresses.project()); finish(settings(180000));
  await vi.waitFor(() => expect(status().textContent).toContain("Saved"));
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(150000);
});

it("keeps the authoritative cache and retryable draft after failure", async () => {
  await mount({ callRpc: async () => { throw new Error("refused"); } });
  draft("180000"); save();
  await vi.waitFor(() => expect(status().textContent).toContain("could not"));
  expect(input().value).toBe("180000");
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(150000);
});

it("retains an in-flight edit made after save started", async () => {
  let finish;
  await mount({ callRpc: () => new Promise((resolve) => { finish = resolve; }) });
  draft("180000"); save(); await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  draft("190000"); finish(settings(180000));
  await vi.waitFor(() => expect(status().textContent).toContain("Saved"));
  expect(input().value).toBe("190000");
});

it("uses the cached old-bridge support record and retains settings", async () => {
  const call = await mount({ support: false });
  expect(document.body.textContent).toContain("Update the bridge to set compaction defaults by harness.");
  expect(input().value).toBe("150000");
  expect(row().querySelector('[data-compaction-action="save"]').disabled).toBe(true);
  expect(call).not.toHaveBeenCalled();
});

it("ignores a late response after the editor is disposed", async () => {
  let finish;
  await mount({ callRpc: () => new Promise((resolve) => { finish = resolve; }) });
  draft("180000"); save(); await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  controller.dispose(); finish(settings(180000));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(150000);
});

it("preserves a focused clean input's caret when a new cached value arrives", async () => {
  await mount();
  const held = input(); held.focus(); held.setSelectionRange(2, 4, "backward");
  await writeCached(addresses.project(), settings(250000));
  await controller.read();
  expect(input().value).toBe("250000");
  expect(document.activeElement).toBe(held);
  expect([held.selectionStart, held.selectionEnd, held.selectionDirection]).toEqual([2, 4, "backward"]);
});

it("shows the adapter's current-greeting rejection even while cached support is true", async () => {
  const message = "Update the bridge to set compaction defaults by harness.";
  await mount({ callRpc: async () => { throw Object.assign(new Error(message), { code: "unknown_method" }); } });
  draft("180000"); save();
  await vi.waitFor(() => expect(status().textContent).toBe(message));
  expect(input().value).toBe("180000");
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(150000);
});

it("displays actionable adapter messages as text", async () => {
  const message = "<b>Compaction defaults need a valid project</b>";
  await mount({ callRpc: async () => { throw new Error(message); } });
  draft("180000"); save();
  await vi.waitFor(() => expect(status().textContent).toContain(message));
  expect(status().querySelector("b")).toBeNull();
});

it("retires subscriptions and pending saves when its section is removed", async () => {
  let finish;
  await mount({ callRpc: () => new Promise((resolve) => { finish = resolve; }) });
  draft("180000"); save(); await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  const section = document.querySelector("[data-compaction-editor]");
  const held = input(); section.remove();
  await Promise.resolve(); finish(settings(180000));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await readCached(addresses.project())).value.compaction_thresholds.codex).toBe(150000);
  await writeCached(addresses.project(), settings(250000));
  await writeCached(compactionSupportAddress("device"), { harnessThresholds: false });
  await controller.read();
  expect(held.value).toBe("180000");
  expect(held.readOnly).toBe(false);
});

it("keeps a synchronously transplanted section subscribed", async () => {
  await mount();
  const section = document.querySelector("[data-compaction-editor]");
  section.remove(); document.body.append(section);
  await Promise.resolve();
  await writeCached(addresses.project(), settings(250000));
  await controller.read();
  expect(input().value).toBe("250000");
});

it("declares numeric keyboard and autofill traits for each token field", async () => {
  await mount();
  for (const field of document.querySelectorAll("[data-compaction-tokens]")) {
    expect(Object.fromEntries(["autocomplete", "autocorrect", "autocapitalize", "spellcheck", "inputmode", "enterkeyhint"]
      .map((name) => [name, field.getAttribute(name)]))).toEqual({
      autocomplete: "off", autocorrect: "off", autocapitalize: "off", spellcheck: "false", inputmode: "numeric", enterkeyhint: "done",
    });
  }
});
