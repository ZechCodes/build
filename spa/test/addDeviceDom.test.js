// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { openAddDevice } from "../src/sheets/addDevice.js";
import { lookupDevice, approveDevice } from "../src/api.js";

vi.mock("../src/api.js", () => ({ lookupDevice: vi.fn(), approveDevice: vi.fn() }));

beforeEach(() => {
  vi.resetAllMocks();
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
});

it("keeps the pairing title outside the scrolling settings body", () => {
  openAddDevice(vi.fn());
  const frame = document.querySelector("#sheet > .settings-sheet-frame");
  expect(frame.querySelector(":scope > .settings-sheet-header h3").textContent).toBe("Add a device");
  expect(frame.querySelector(":scope > .settings-sheet-body #paircode")).not.toBeNull();
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

it("looks the code up on Enter, the key the field labels Go", async () => {
  lookupDevice.mockResolvedValue({ name: "Machine", fingerprint: "key" });
  openAddDevice(vi.fn());
  const code = document.querySelector("#paircode");
  expect(code.getAttribute("enterkeyhint")).toBe("go");
  code.value = "wxyz-4f2k";
  code.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await vi.waitFor(() => expect(document.querySelector("#papprove")).not.toBeNull());
  expect(lookupDevice).toHaveBeenCalledWith("WXYZ-4F2K");
});

it("a code handed in by the approve link is filled in and looked up, never approved", async () => {
  lookupDevice.mockResolvedValue({ name: "Mac", fingerprint: "28e679939bc32c446627fac8a8dd58a5353e66b744a9a2dae91254f1da1b9027" });
  openAddDevice(vi.fn(), { code: "zsac-abu6" });
  expect(document.querySelector("#paircode").value).toBe("ZSAC-ABU6");
  await vi.waitFor(() => expect(document.querySelector("#papprove")).not.toBeNull());
  expect(lookupDevice).toHaveBeenCalledWith("ZSAC-ABU6");
  expect(approveDevice).not.toHaveBeenCalled();
});

it("shows the fingerprint in the short form the bridge printed, with the full one beneath", async () => {
  const fingerprint = "28e679939bc32c446627fac8a8dd58a5353e66b744a9a2dae91254f1da1b9027";
  lookupDevice.mockResolvedValue({ name: "Mac", fingerprint });
  openAddDevice(vi.fn(), { code: "ZSAC-ABU6" });
  await vi.waitFor(() => expect(document.querySelector("#papprove")).not.toBeNull());
  expect(document.querySelector("[data-fingerprint-short]").textContent).toBe("28e6 7993 9bc3 2c44");
  expect(document.querySelector("[data-fingerprint-full]").textContent).toBe(fingerprint);
});
