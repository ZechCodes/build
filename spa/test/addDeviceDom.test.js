// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { openAddDevice } from "../src/sheets/addDevice.js";
import { lookupDevice, approveDevice } from "../src/api.js";

vi.mock("../src/api.js", () => ({ lookupDevice: vi.fn(), approveDevice: vi.fn() }));

beforeEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

it("a disposed lookup cannot paint or close the next pairing sheet", async () => {
  let finish;
  lookupDevice.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const dispose = openAddDevice(vi.fn());
  document.querySelector("#paircode").value = "first";
  const pending = document.querySelector("#plookup").onclick();
  dispose();
  openAddDevice(vi.fn());
  dispose();
  finish({ name: "Old machine", fingerprint: "old" });
  await pending;
  expect(document.querySelector("#pairbox").textContent).toBe("");
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
});

it("completed pairing still notifies its caller without closing a replacement sheet", async () => {
  lookupDevice.mockResolvedValue({ name: "Machine", fingerprint: "key" });
  let finish;
  approveDevice.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
  const onDone = vi.fn();
  const dispose = openAddDevice(onDone);
  document.querySelector("#paircode").value = "code";
  await document.querySelector("#plookup").onclick();
  const pending = document.querySelector("#papprove").onclick();
  dispose();
  openAddDevice(vi.fn());
  finish();
  await pending;
  expect(approveDevice).toHaveBeenCalledWith("CODE");
  expect(onDone).toHaveBeenCalledOnce();
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(true);
  expect(document.querySelector("#perr").textContent).toBe("");
});
