// @vitest-environment jsdom
// The wire source is a Rust FrameHandler test, not a hand-written JS status.
// It performs a real RPC, decrypts the resulting push, and prints both frames.
import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { bridgeUpdateAddress } from "../src/core/bridgeUpdates.js";
import { dispatchChangeEvent } from "../src/core/changeEvents.js";
import { readCached, wipeCache } from "../src/core/localCache.js";
import { mountBridgeUpdatePanel } from "../src/core/bridgeUpdatePanel.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bridgeRoot = resolve(process.cwd(), "../bridge");
const probeBinary = process.env.BRIDGE_UPDATE_PROBE_BIN;
const probeCommand = probeBinary || "cargo";
const probeArgs = probeBinary
  ? ["update_status_rpc_push_probe", "--nocapture"]
  : ["test", "--lib", "update_status_rpc_push_probe", "--", "--nocapture"];

it("carries a real Rust update RPC and decrypted push into the cached settings panel", async () => {
  const output = execFileSync(probeCommand, probeArgs, {
    cwd: bridgeRoot, encoding: "utf8", timeout: 900_000, maxBuffer: 20 * 1024 * 1024,
  });
  const encoded = output.match(/BRIDGE_UPDATE_WIRE=(\{[^\r\n]+\})/)?.[1];
  expect(encoded, output).toBeTruthy();
  const { initial_reply: initialReply, reply, event } = JSON.parse(encoded);
  expect(initialReply.ok).toBe(true);
  expect(initialReply.result.state).toBe("idle");
  expect(initialReply.result.update_available).toBe(false);
  expect(reply.ok).toBe(true);
  expect(event.type).toBe("bridge.update_status");
  expect(event.running_version).toBe(reply.result.running_version);

  await wipeCache();
  document.body.innerHTML = '<div id="update-panel"></div>';
  const calls = [];
  const dispose = mountBridgeUpdatePanel(document.querySelector("#update-panel"), {
    deviceId: "rust-probe",
    callRpc: async (method, params) => {
      calls.push({ method, params });
      return initialReply.result;
    },
  });
  try {
    await expect.poll(() => document.querySelector(".bridge-update-state")?.textContent).toBe("Not checked yet.");
    expect(calls[0]).toEqual({ method: "bridge.update_status", params: {} });
    expect(document.querySelector("[data-bridge-install-now]").disabled).toBe(true);
    expect(dispatchChangeEvent(event, "rust-probe")).toBe(true);
    await expect.poll(async () => (await readCached(bridgeUpdateAddress("rust-probe")))?.value?.latest_release?.version).toBe(event.latest_release.version);
    expect((await readCached(bridgeUpdateAddress("rust-probe"))).value).toEqual(reply.result);
    await expect.poll(() => document.querySelector(".bridge-update-state")?.textContent).toContain(event.latest_release.version);
    expect(document.querySelector(".bridge-update-body").textContent).toContain(event.running_version);
    expect(document.querySelector("[data-bridge-install-now]").disabled).toBe(false);
  } finally {
    dispose();
  }
}, 930_000);
