// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
const { App, openSession, openBrowser } = vi.hoisted(() => ({
  App: { devices: [], viewDispose: null }, openSession: vi.fn(), openBrowser: vi.fn(),
}));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/connection.js", () => ({ openDeviceSettingsSession: openSession }));
vi.mock("../src/sheets/browser.js", () => ({ openBrowser }));
import { renderDeviceSettings } from "../src/views/deviceSettings.js";
let session;
beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '<main id="root"></main><div id="scrim"><div id="sheet"></div></div>';
  App.devices = [{ id: "other", name: "Other machine", status: "online" }];
  App.route = { name: "device", id: "other" };
  session = {
    deviceId: "other",
    call: vi.fn(async (method) => {
      if (method === "settings.get") return {
        projects_dir: "/projects",
        isolation: "worktree",
        isolation_available: { rift: true },
        default_harness: "claude",
        agent_modes: { claude: "headless", codex: "headless" },
        triage_enabled: true,
      };
      if (method === "models.list") return { providers: [{ id: "claude", label: "Claude Code" }] };
      return {};
    }),
    close: vi.fn(),
  };
  openSession.mockResolvedValue(session);
});
describe("device settings", () => {
  it("shows bridge-owned preferences on the named device and saves isolation through its pinned session", async () => {
    await renderDeviceSettings();

    expect(document.querySelector("#root").textContent).toContain("Work isolation");
    expect(document.querySelector("#root").textContent).toContain("Agent modes");
    expect(document.querySelector("#root").textContent).toContain("Fallback agent");
    expect(document.querySelector("#root").textContent).toContain("Diff triage");
    const select = document.querySelector("[data-isolation=select]");
    select.value = "rift";
    select.dispatchEvent(new Event("change"));
    await new Promise((done) => setTimeout(done, 0));

    expect(session.call).toHaveBeenCalledWith("settings.set", { isolation: "rift" });
  });

  it("uses the named device's Rift capability rather than the active application session", async () => {
    App.call = vi.fn().mockResolvedValue({ isolation_available: { rift: true } });
    session.call.mockImplementation(async (method) => {
      if (method === "settings.get") return {
        projects_dir: "/projects",
        isolation: "worktree",
        isolation_available: { rift: false, reason: "Rift CLI was not found" },
        default_harness: "claude",
        agent_modes: { claude: "headless", codex: "headless" },
        triage_enabled: true,
      };
      if (method === "models.list") return { providers: [{ id: "claude", label: "Claude Code" }] };
      return {};
    });

    await renderDeviceSettings();

    const rift = [...document.querySelector("[data-isolation=select]").options].find(({ value }) => value === "rift");
    expect(rift.disabled).toBe(true);
    expect(document.querySelector("[data-isolation=lock]").textContent).toContain("Rift CLI was not found");
    expect(App.call).not.toHaveBeenCalled();
  });

  it("does not let a detached preference control call its old device", async () => {
    await renderDeviceSettings();
    const stale = document.querySelector("[data-isolation=select]");
    App.viewDispose();

    stale.value = "rift";
    stale.dispatchEvent(new Event("change"));
    await new Promise((done) => setTimeout(done, 0));

    expect(session.call).not.toHaveBeenCalledWith("settings.set", expect.anything());
  });

  it.each(["resolve", "reject"])(
    "keeps reconnected isolation authoritative when an old save %ss late",
    async (completion) => {
      await renderDeviceSettings();
      const oldSession = session;
      const oldSelect = document.querySelector("[data-isolation=select]");
      let resolveSave;
      let rejectSave;
      oldSession.call.mockReturnValueOnce(new Promise((resolve, reject) => {
        resolveSave = resolve;
        rejectSave = reject;
      }));
      oldSelect.value = "rift";
      oldSelect.dispatchEvent(new Event("change"));

      openSession.mock.calls[0][1].onLost();
      const newSettings = {
        projects_dir: "/new",
        isolation: "worktree",
        isolation_available: { rift: false, reason: "Rift is absent on the reconnected device" },
        default_harness: "claude",
        agent_modes: { claude: "headless", codex: "headless" },
        triage_enabled: false,
      };
      const newSession = {
        deviceId: "other",
        close: vi.fn(),
        call: vi.fn(async (method) =>
          method === "models.list" ? { providers: [{ id: "claude", label: "Claude Code" }] } : newSettings),
      };
      openSession.mockResolvedValueOnce(newSession);
      document.querySelector("#device-settings-retry").click();
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => setTimeout(done, 0));
      const callsBeforeLateSave = newSession.call.mock.calls.length;

      if (completion === "resolve") resolveSave({ isolation: "rift", isolation_available: { rift: true } });
      else rejectSave(new Error("old save failed"));
      await new Promise((done) => setTimeout(done, 0));
      await new Promise((done) => setTimeout(done, 0));

      const currentSelect = document.querySelector("[data-isolation=select]");
      expect(currentSelect).not.toBe(oldSelect);
      expect(currentSelect.value).toBe("worktree");
      expect([...currentSelect.options].find(({ value }) => value === "rift").disabled).toBe(true);
      expect(document.querySelector("[data-isolation=lock]").textContent).toContain(
        "Rift is absent on the reconnected device",
      );
      expect(newSession.call).toHaveBeenCalledTimes(callsBeforeLateSave);
    },
  );

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
