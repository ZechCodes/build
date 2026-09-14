// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
const { App, openSession, openBrowser } = vi.hoisted(() => ({
  App: { devices: [], viewDispose: null }, openSession: vi.fn(), openBrowser: vi.fn(),
}));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/connection.js", () => ({
  openDeviceSettingsSession: openSession,
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncHome: () => {},
  forgetHomeFollow: () => {},
  setConn: () => {},
  CONNECTION_STATUS: {},
}));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
import { renderDeviceSettings } from "../src/views/deviceSettings.js";
let session;
beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '<main id="root"></main><div id="scrim"><div id="sheet"></div></div>';
  App.devices = [{ id: "other", name: "Other machine", status: "online" }];
  App.route = { name: "device", id: "other" };
  session = { deviceId: "other", call: vi.fn().mockResolvedValue({ projects_dir: "/projects" }), close: vi.fn() };
  openSession.mockResolvedValue(session);
});
describe("device settings", () => {
  it("opens the named device and saves through the same connection as the folder browser", async () => {
    await renderDeviceSettings();
    expect(openSession).toHaveBeenCalledWith("other", { onLost: expect.any(Function) });
    expect(document.querySelector("h1").textContent).toContain("Other machine");
    document.querySelector("#device-projects-change").click();
    const options = openBrowser.mock.calls[0][0];
    expect(options.startPath).toBe("/projects");
    await options.callRpc("fs.list", { path: "/next" });
    expect(session.call).toHaveBeenLastCalledWith("fs.list", { path: "/next" });
    session.call.mockResolvedValueOnce({ projects_dir: "/next" });
    await options.onChoose("/next");
    expect(session.call).toHaveBeenLastCalledWith("settings.set", { projects_dir: "/next" });
    expect(document.querySelector("#device-projects-path").textContent).toBe("/next");
    expect(document.querySelector("#device-settings-status").textContent).toContain("Saved");
  });
  it("does not connect to an offline or unknown device", async () => {
    App.devices[0].status = "offline";
    await renderDeviceSettings();
    expect(openSession).not.toHaveBeenCalled();
    expect(document.querySelector("#device-projects-change").disabled).toBe(true);
    App.route.id = "unknown";
    await renderDeviceSettings();
    expect(openSession).not.toHaveBeenCalled();
    expect(document.querySelector("#root").textContent).toContain("Device not found");
  });
  it("closes a connection that finishes opening after leaving", async () => {
    let resolve;
    openSession.mockReturnValue(new Promise((done) => { resolve = done; }));
    const rendering = renderDeviceSettings();
    App.viewDispose();
    resolve(session);
    await rendering;
    expect(session.close).toHaveBeenCalled();
    expect(session.call).not.toHaveBeenCalled();
  });
  it("rejects a stale picker save after leaving the page", async () => {
    await renderDeviceSettings();
    document.querySelector("#device-projects-change").click();
    const options = openBrowser.mock.calls[0][0];
    App.viewDispose();
    await options.onChoose("/wrong");
    expect(session.call).not.toHaveBeenCalledWith("settings.set", expect.anything());
    expect(session.close).toHaveBeenCalled();
  });
  it("shows save errors without replacing the confirmed directory", async () => {
    await renderDeviceSettings();
    document.querySelector("#device-projects-change").click();
    document.querySelector("#sheet").innerHTML = '<div id="berr"></div>';
    session.call.mockRejectedValueOnce(new Error("Permission denied"));
    await openBrowser.mock.calls[0][0].onChoose("/restricted");
    expect(document.querySelector("#berr").textContent).toBe("Permission denied");
    expect(document.querySelector("#device-projects-path").textContent).toBe("/projects");
  });
});

it("disables folder selection after disconnect and ignores old connection callbacks after retry", async () => {
  await renderDeviceSettings();
  const onLost = openSession.mock.calls[0][1].onLost;
  onLost();
  expect(document.querySelector("#device-projects-change").disabled).toBe(true);
  expect(document.querySelector("#device-settings-retry").hidden).toBe(false);
  expect(session.close).toHaveBeenCalled();
  document.querySelector("#device-settings-retry").click();
  await new Promise((done) => setTimeout(done, 0));
  expect(document.querySelector("#device-projects-change").disabled).toBe(false);
  onLost();
  expect(document.querySelector("#device-projects-change").disabled).toBe(false);
});
it("closes the connection when loading settings fails", async () => {
  session.call.mockRejectedValueOnce(new Error("Settings unavailable"));
  await renderDeviceSettings();
  expect(session.close).toHaveBeenCalled();
  expect(document.querySelector("#device-projects-change").disabled).toBe(true);
  expect(document.querySelector("#device-settings-retry").hidden).toBe(false);
});
it.each(["resolve", "reject"])("ignores a stale save that %ss after reconnect, while allowing a new save", async (completion) => {
  await renderDeviceSettings();
  document.querySelector("#device-projects-change").click();
  let resolveSave, rejectSave;
  session.call.mockReturnValueOnce(new Promise((resolve, reject) => { resolveSave = resolve; rejectSave = reject; }));
  const staleSave = openBrowser.mock.calls[0][0].onChoose("/old");
  openSession.mock.calls[0][1].onLost();
  const newSession = { deviceId: "other", close: vi.fn(), call: vi.fn().mockResolvedValue({ projects_dir: "/new" }) };
  openSession.mockResolvedValueOnce(newSession);
  document.querySelector("#device-settings-retry").click();
  await new Promise((done) => setTimeout(done, 0));
  expect(document.querySelector("#device-projects-path").textContent).toBe("/new");
  document.querySelector("#device-projects-change").click();
  newSession.call.mockResolvedValueOnce({ projects_dir: "/latest" });
  await openBrowser.mock.calls[1][0].onChoose("/latest");
  expect(newSession.call).toHaveBeenLastCalledWith("settings.set", { projects_dir: "/latest" });
  if (completion === "resolve") resolveSave({ projects_dir: "/old" });
  else rejectSave(new Error("Old connection error"));
  await staleSave;
  expect(document.querySelector("#device-projects-path").textContent).toBe("/latest");
  expect(document.querySelector("#device-settings-status").textContent).toBe("Saved. New projects will use this folder.");
});
