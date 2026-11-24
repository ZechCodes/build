// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../src/appState.js";
import { uiAddress } from "../src/core/localUiState.js";
import * as uiStore from "../src/core/localUiStore.js";

const listeners = vi.hoisted(() => new Set());
vi.mock("../src/devices.js", () => ({
  onDevicesChanged: (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
}));

const account = "10000000-0000-4000-8000-000000000049";
const another = "10000000-0000-4000-8000-000000000050";
const notify = () => { for (const listener of listeners) listener(); };
const change = (accountId, promotionRevision, tunnelAccess = true) => {
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  App.deviceTransportPolicy = { accountId, promotionRevision, tunnelAccess };
  notify();
};
const acknowledgement = (accountId, revision) =>
  uiAddress({ entityId: accountId, view: "account", kind: "hosted-https-notice", sub: String(revision) });

afterEach(() => {
  listeners.clear();
  document.head.replaceChildren();
  App.deviceTransportPolicy = null;
  App.devices = [];
  vi.restoreAllMocks();
});

it("acknowledges immediately for one account and promotion, independent of assignment rotation", async () => {
  const { mountHostedTransportNotice } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  change(account, 3);
  const dispose = mountHostedTransportNotice(host);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  expect(host.textContent).toContain("Cloudflare can read your code, prompts and terminal traffic on this connection.");
  await host.querySelector("button").onclick();
  expect(host.hidden).toBe(true);

  App.devices = [{ id: "dev", transport: { assignment_revision: 39 } }];
  notify();
  expect(host.hidden).toBe(true);
  expect((await uiStore.readUiRecord(acknowledgement(account, 3)))?.value).toBe(true);

  change(another, 3);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  expect(await uiStore.readUiRecord(acknowledgement(another, 3))).toBeUndefined();

  change(account, 4);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  expect(await uiStore.readUiRecord(acknowledgement(account, 4))).toBeUndefined();
  dispose();
});

it("keeps the acknowledged notice hidden after a sign-out wipe and a settled remount", async () => {
  const { mountHostedTransportNotice } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  const revision = 8;
  const address = acknowledgement(account, revision);
  change(account, revision);
  const unmountBeforeWipe = mountHostedTransportNotice(host);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  await host.querySelector("button").onclick();
  unmountBeforeWipe();

  await uiStore.wipeUiRecords();
  expect((await uiStore.readUiRecord(address))?.value).toBe(true);

  const read = vi.spyOn(uiStore, "readUiRecord");
  const dispose = mountHostedTransportNotice(host);
  await vi.waitFor(() => expect(read).toHaveBeenCalledExactlyOnceWith(address));
  await read.mock.results[0].value;
  expect(host.hidden).toBe(true);
  dispose();
});

it("clears the notice when tunnel access is withdrawn at the same promotion revision", async () => {
  const { mountHostedTransportNotice } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  change(account, 12);
  const dispose = mountHostedTransportNotice(host);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  change(account, 12, false);
  expect(host.hidden).toBe(true);
  expect(host.querySelector("button").onclick).toBeNull();
  dispose();
});

it("restores an Electron acknowledgement into the renderer cache before painting", async () => {
  const { mountHostedTransportNotice } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  const revision = 17;
  const hostedNotice = vi.fn(async () => ({ ok: true, value: true }));
  window.buildBridgeHttp = { hostedNotice };
  change(account, revision);
  const dispose = mountHostedTransportNotice(host);
  try {
    await vi.waitFor(async () => expect((await uiStore.readUiRecord(acknowledgement(account, revision)))?.value).toBe(true));
    expect(host.hidden).toBe(true);
    expect(hostedNotice).toHaveBeenCalledExactlyOnceWith({ accountId: account, promotionRevision: revision });
  } finally {
    delete window.buildBridgeHttp;
    dispose();
  }
});
