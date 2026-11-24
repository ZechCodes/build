// @vitest-environment jsdom
import "fake-indexeddb/auto";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../src/appState.js";
import { readUiRecord, wipeUiRecords } from "../src/core/localUiStore.js";

const listeners = vi.hoisted(() => new Set());
vi.mock("../src/devices.js", () => ({
  onDevicesChanged: (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
}));

const account = "10000000-0000-4000-8000-000000000049";
const another = "10000000-0000-4000-8000-000000000050";
const change = (accountId, promotionRevision, tunnelAccess = true) => {
  document.head.innerHTML = `<meta name="build-account-id" content="${accountId}">`;
  App.deviceTransportPolicy = { accountId, promotionRevision, tunnelAccess };
  for (const listener of listeners) listener();
};

afterEach(() => {
  listeners.clear();
  document.head.replaceChildren();
  App.deviceTransportPolicy = null;
});

it("acknowledges immediately for one account and promotion, independent of assignment rotation", async () => {
  const { mountHostedTransportNotice, hostedNoticeAddress } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  change(account, 3);
  let dispose = mountHostedTransportNotice(host);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  expect(host.textContent).toContain("Cloudflare can read your code, prompts and terminal traffic on this connection.");
  host.querySelector("button").click();
  await vi.waitFor(() => expect(host.hidden).toBe(true));
  expect((await readUiRecord(hostedNoticeAddress(account, 3))).value).toBe(true);
  dispose();
  await wipeUiRecords();
  dispose = mountHostedTransportNotice(host);
  App.devices = [{ id: "dev", transport: { assignment_revision: 39 } }];
  change(account, 3);
  await vi.waitFor(() => expect(host.hidden).toBe(true));
  change(another, 3);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  change(account, 3);
  await vi.waitFor(() => expect(host.hidden).toBe(true));
  change(account, 4);
  await vi.waitFor(() => expect(host.hidden).toBe(false));
  App.deviceTransportPolicy = null;
  for (const listener of listeners) listener();
  expect(host.hidden).toBe(true);
  dispose();
});

it("restores an Electron acknowledgement into the renderer cache before painting", async () => {
  const { mountHostedTransportNotice, hostedNoticeAddress } = await import("../src/core/hostedTransportNotice.js");
  const host = document.createElement("section");
  const revision = 17;
  const hostedNotice = vi.fn(async () => ({ ok: true, value: true }));
  window.buildBridgeHttp = { hostedNotice };
  change(account, revision);
  const dispose = mountHostedTransportNotice(host);
  try {
    await vi.waitFor(async () => expect((await readUiRecord(hostedNoticeAddress(account, revision)))?.value).toBe(true));
    expect(host.hidden).toBe(true);
    expect(hostedNotice).toHaveBeenCalledExactlyOnceWith({ accountId: account, promotionRevision: revision });
  } finally {
    delete window.buildBridgeHttp;
    dispose();
  }
});
