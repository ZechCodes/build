// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, expect, it, vi } from "vitest";
import { openAddDevice } from "../src/sheets/addDevice.js";
import { lookupDevice, approveDevice } from "../src/api.js";
import { pairingsConnecting, resetPendingPairing } from "../src/core/pendingPairing.js";

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

// #321: the page remembers which device it approved, so it can read the
// account for it every second and name it while it comes up.
it("remembers the approved device as connecting, and nothing on a refusal", async () => {
  resetPendingPairing();
  lookupDevice.mockResolvedValue({ device_id: "d1", name: "Machine", fingerprint: "key" });
  approveDevice.mockRejectedValueOnce(new Error("approve failed"));
  openAddDevice(vi.fn());
  document.querySelector("#paircode").value = "code";
  await document.querySelector("#plookup").onclick();
  await document.querySelector("#papprove").onclick();
  expect(pairingsConnecting()).toEqual([]);

  approveDevice.mockResolvedValue(undefined);
  await document.querySelector("#papprove").onclick();
  expect(pairingsConnecting()).toMatchObject([{ deviceId: "d1", name: "Machine" }]);
  resetPendingPairing();
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
  expect(document.querySelector("[data-fingerprint-short]").textContent).toBe("28e6 7993 9bc3 2c44 6627 fac8 a8dd 58a5");
  expect(document.querySelector("[data-fingerprint-full]").textContent).toBe(fingerprint);
});

// Anyone can send a pairing link: an attacker runs `build-bridge pair` on their
// own machine and sends the link to a signed-in victim (#319 review). A sheet
// a link opened says so, and says plainly what approving does.
const LINK_SUBTITLE = "A pairing link opened this.";
const LINK_WARNING =
  "Only approve if you just ran the installer or build-bridge pair on a machine you own. Approving gives that machine access to your account.";

it("a sheet a link opened says so and warns before anything can be approved", () => {
  lookupDevice.mockReturnValue(new Promise(() => {}));
  openAddDevice(vi.fn(), { code: "ZSAC-ABU6", fromLink: true });
  const sheet = document.querySelector("#sheet");
  expect(sheet.querySelector(".sub").textContent).toBe(LINK_SUBTITLE);
  expect(sheet.textContent).not.toContain("Enter the pairing code your bridge printed");
  expect(sheet.querySelector("[data-link-warning]").textContent.replace(/\s+/g, " ").trim()).toBe(LINK_WARNING);
});

// A warning, not an error: its own class, styled from theme tokens, with no
// inline style and nothing borrowed from the error line (#320).
it("the link warning is styled as a warning from theme tokens", () => {
  lookupDevice.mockReturnValue(new Promise(() => {}));
  openAddDevice(vi.fn(), { code: "ZSAC-ABU6", fromLink: true });
  const warning = document.querySelector("[data-link-warning]");
  expect(warning.className).toBe("addwarn");
  expect(warning.hasAttribute("style")).toBe(false);
  const css = readFileSync(resolve("src/styles.css"), "utf8");
  const rule = css.match(/^\.addwarn \{([^}]*)\}/m)?.[1];
  expect(rule).toContain("color:var(--amber)");
  expect(rule).toContain("background:var(--amber-bg)");
  expect(rule).not.toMatch(/#[0-9a-f]{3,8}\b|rgb|hsl/i);
});

it("a sheet opened by hand keeps its own subtitle and carries no link warning", () => {
  openAddDevice(vi.fn());
  const sheet = document.querySelector("#sheet");
  expect(sheet.textContent).toContain("Enter the pairing code your bridge printed");
  expect(sheet.querySelector("[data-link-warning]")).toBeNull();
});
