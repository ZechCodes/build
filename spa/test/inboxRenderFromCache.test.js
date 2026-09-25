// @vitest-environment jsdom
// #140, the #136 review's reproducer: a machine that is still connecting — the
// session-less stand-in a cold reload makes — paints its cached rows exactly as
// it will once its session lands. The greyed row and its word are marks beside
// what the cache holds; which controls there are, and whether they are live, is
// not connection state's to decide. The block's menu is held open, so Hide —
// which lives in it — is counted too.
import { it, expect, vi } from "vitest";
import "fake-indexeddb/auto";
import { App } from "../src/app.js";
import { knownDeviceContext, adoptDeviceSession, canAnswer, contextFor, resetDeviceContexts } from "../src/core/deviceContexts.js";
import { paintDeviceState } from "../src/core/inboxDevices.js";
import { deviceTags, projectHeadHtml } from "../src/core/inboxProjects.js";

it("inbox controls paint identically from fixed cached rows while connecting and connected", () => {
  resetDeviceContexts();
  App.devices = [{ id: "dev-1", name: "Desktop", status: "online" }];
  const cachedProject = { project_id: "p1", projectKey: "dev-1/p1", name: "Cached project", deviceId: "dev-1" };
  const cachedRow = { key: "dev-1/ws-1", deviceId: "dev-1", name: "Cached workspace" };
  const list = document.createElement("div");
  document.body.append(list);
  const paint = () => {
    const offline = new Set(App.devices.map((d) => d.id).filter((id) => !canAnswer(contextFor(id))));
    const tags = deviceTags([cachedProject], App.devices, offline).get(cachedProject.projectKey);
    const block = { ...cachedProject, ...tags, key: "project:dev-1/p1", entries: [cachedRow], recent: [] };
    list.innerHTML = `<div class="inbox-project" data-project="dev-1/p1">${projectHeadHtml(block, { openMenuKey: block.key })}</div>
      <div class="inbox-entry" data-key="dev-1/ws-1"><div class="inbox-body"><div class="inbox-line">${cachedRow.name}</div></div><button data-workspace-done>Done</button><div class="inbox-menu"><button class="mi">Archive</button></div></div>`;
    paintDeviceState(list, { entryFor: () => cachedRow, blockFor: () => block });
    return {
      disabledControls: [...list.querySelectorAll("[disabled]")].map((b) => b.className || "workspace-done"),
      hideProjectButtons: list.querySelectorAll("[data-project-hide]").length,
    };
  };
  knownDeviceContext("dev-1");
  const connecting = paint();
  adoptDeviceSession({ deviceId: "dev-1", call: vi.fn(async () => ({})), close() {}, peer() {}, onCarrier() {} });
  const connected = paint();
  expect(connecting.disabledControls).toEqual(connected.disabledControls);
  expect(connecting.hideProjectButtons).toBe(connected.hideProjectButtons);
  expect(connected.hideProjectButtons).toBe(1);
});
