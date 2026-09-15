// @vitest-environment jsdom
// How a task's checkout is isolated: a git worktree of the project repository,
// or a Rift copy-on-write checkout of the whole project directory.
//
// The choice is the account's, overridable per project, and Rift requires its
// CLI on the device — the bridge decides availability and hands back the
// sentence saying why. This module is the client's only naming table
// for the two, so the settings panel and the project sheet can never call the
// same isolation two different things, and neither learns a variant name, a
// label or a locked look.

import { describe, it, expect, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { renderDeviceSettingsPage } from "./deviceSettingsFixture.js";

// The device page opens its own connection to the machine it is about; here it
// answers with whatever the fixture's bridge is standing at the time.
const { openSession } = vi.hoisted(() => ({ openSession: vi.fn() }));
vi.mock("../src/connection.js", () => ({
  openDeviceSettingsSession: openSession,
  chooseCreationDevice: () => {},
  retireDevice: () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncHome: () => {},
  goOffline: () => {},
  deviceWentAway: () => {},
  forgetHomeFollow: () => {},
  forgetSecurityStops: () => {},
  forgetRendezvousSockets: () => {},
  securityStopText: () => "",
}));
import {
  ISOLATIONS,
  isolationLabel,
  isolationOf,
  isolationLockReason,
  isolationOptionsHtml,
  isolationPanelHtml,
  DEVICE_ISOLATION,
  projectIsolationTarget,
  mountIsolation,
} from "../src/core/isolation.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

// Re-importing the whole app shell can outrun the default deadline on a loaded
// machine.
const SLOW_IMPORT_MS = 30000;

const flush = () => new Promise((done) => setTimeout(done, 0));

const optionsOf = (html) => {
  const select = document.createElement("select");
  select.innerHTML = html;
  return [...select.options];
};

describe("the isolation a payload names", () => {
  it("takes the bridge's word when it names one", () => {
    expect(isolationOf({ isolation: "rift" })).toBe("rift");
    expect(isolationOf({ isolation: "worktree" })).toBe("worktree");
  });

  it("reads silence and nonsense as a git worktree rather than nothing", () => {
    expect(isolationOf({})).toBe("worktree");
    expect(isolationOf({ isolation: "nope" })).toBe("worktree");
    expect(isolationOf(undefined)).toBe("worktree");
    expect(isolationOf(null)).toBe("worktree");
  });
});

describe("the reason Rift is unavailable", () => {
  it("is empty while this device can use Rift", () => {
    expect(isolationLockReason({ rift: true, reason: null })).toBe("");
    expect(isolationLockReason(undefined)).toBe("");
    expect(isolationLockReason(null)).toBe("");
  });

  it("is the bridge's own sentence when it cannot", () => {
    expect(isolationLockReason({ rift: false, reason: "Rift CLI was not found" })).toBe(
      "Rift CLI was not found",
    );
  });

  it("still says something when the bridge sends no sentence", () => {
    expect(isolationLockReason({ rift: false })).toBe("unavailable on this device");
  });
});

describe("the isolation options", () => {
  it("names both isolations, in the naming table's order, marking the chosen one", () => {
    const options = optionsOf(isolationOptionsHtml("rift", { rift: true }));

    expect(options.map((option) => option.value)).toEqual(["worktree", "rift"]);
    expect(options.map((option) => option.textContent)).toEqual(["Git worktree", "Rift (copy-on-write)"]);
    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual(["rift"]);
  });

  it("disables Rift, and only Rift, when the CLI is unavailable", () => {
    const options = optionsOf(isolationOptionsHtml("worktree", { rift: false, reason: "Rift CLI was not found" }));

    expect(options.map((option) => option.disabled)).toEqual([false, true]);
    expect(options[1].title).toBe("Rift CLI was not found");
  });

  it("escapes the bridge's sentence rather than rendering it", () => {
    const html = isolationOptionsHtml("worktree", { rift: false, reason: '<img src=x onerror="boom">' });

    expect(html).not.toContain("<img");
    expect(optionsOf(html)[1].title).toBe('<img src=x onerror="boom">');
  });

  it("leads with the inherit option when asked, and marks it when nothing is overridden", () => {
    const options = optionsOf(isolationOptionsHtml(null, { rift: true }, { inheritLabel: "Account default (Git worktree)" }));

    expect(options.map((option) => option.value)).toEqual(["", "worktree", "rift"]);
    expect(options[0].textContent).toBe("Account default (Git worktree)");
    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual([""]);
  });

  it("marks the override, not the inherit option, once a project has one", () => {
    const options = optionsOf(isolationOptionsHtml("worktree", { rift: true }, { inheritLabel: "Account default (Rift (copy-on-write))" }));

    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual(["worktree"]);
  });

  it("escapes an inherit label too", () => {
    const html = isolationOptionsHtml(null, { rift: true }, { inheritLabel: "<b>Account default</b>" });

    expect(html).not.toContain("<b>");
    expect(optionsOf(html)[0].textContent).toBe("<b>Account default</b>");
  });

  it("offers no inherit option to the account, which has nothing to inherit from", () => {
    expect(optionsOf(isolationOptionsHtml("worktree", { rift: true })).map((option) => option.value)).toEqual([
      "worktree",
      "rift",
    ]);
  });
});

describe("the settings panel", () => {
  const panel = () => {
    document.body.innerHTML = isolationPanelHtml();
    return document.body;
  };

  it("waits for the bridge before offering a choice", () => {
    const host = panel();
    const select = host.querySelector("[data-isolation=select]");

    expect(select.disabled).toBe(true);
    expect(select.textContent).toContain("loading…");
  });

  it("says what each isolation gives you, in the words both isolations are named by", () => {
    const host = panel();

    expect(host.textContent).toContain("Work isolation");
    expect(host.textContent).toContain(
      "Rift uses copy-on-write checkouts that preserve the project's build caches. Install the Rift CLI on this device to use it. A git worktree shares the project's repository and starts empty.",
    );
  });

  it("carries the empty lines the save and the lock speak through", () => {
    const host = panel();

    expect(host.querySelector("[data-isolation=lock]").textContent).toBe("");
    expect(host.querySelector("[data-isolation=error]").textContent).toBe("");
    expect(host.querySelector("[data-isolation=saved]").textContent).toBe("");
  });
});

describe("where a chosen isolation is sent", () => {
  it("puts the account's choice to settings.set, with nothing to inherit", () => {
    expect(DEVICE_ISOLATION).toEqual({ rpc: "settings.set", params: {}, inherits: false, inheritLabel: null });
  });

  it("keys a project's choice on its own id and names the device default it replaces", () => {
    expect(projectIsolationTarget({ project_id: "p1", isolation_default: "rift" })).toEqual({
      rpc: "project.set_isolation",
      params: { project_id: "p1" },
      inherits: true,
      inheritLabel: "Device default (Rift (copy-on-write))",
    });
  });

  it("names a git worktree as the default the bridge did not name", () => {
    expect(projectIsolationTarget({ project_id: "p2" }).inheritLabel).toBe("Device default (Git worktree)");
    expect(projectIsolationTarget({ project_id: "p2", isolation_default: "nope" }).inheritLabel).toBe(
      "Device default (Git worktree)",
    );
  });
});

describe("the naming table", () => {
  it("is the one place either isolation is given a name", () => {
    expect(ISOLATIONS).toEqual([
      { id: "worktree", label: "Git worktree" },
      { id: "rift", label: "Rift (copy-on-write)" },
    ]);
  });

  it("gives a wire word its name, and a word it cannot read a git worktree's", () => {
    expect(isolationLabel("rift")).toBe("Rift (copy-on-write)");
    expect(isolationLabel("worktree")).toBe("Git worktree");
    expect(isolationLabel("nope")).toBe("Git worktree");
    expect(isolationLabel(null)).toBe("Git worktree");
  });
});

// One control, two owners: the account's own setting and a project's override.
// The mount is written once and told where to send the choice, so the settings
// page and the project sheet cannot drift on what saving means, on what a
// locked volume looks like, or on what the control shows after a refusal.
describe("the mounted control", () => {
  const mount = async (target, settings, callRpc) => {
    document.body.innerHTML = isolationPanelHtml();
    await mountIsolation(document.body, { callRpc, target, settings });
    return document.body;
  };
  const select = () => document.querySelector("[data-isolation=select]");
  const lock = () => document.querySelector("[data-isolation=lock]");
  const error = () => document.querySelector("[data-isolation=error]");
  const saved = () => document.querySelector("[data-isolation=saved]");
  const choose = async (value) => {
    select().value = value;
    select().dispatchEvent(new Event("change"));
    await flush();
    await flush();
  };

  // Whether "no choice" is a choice is the target's capability, not its copy:
  // a target that can inherit reads a missing value as inheriting, one that
  // cannot reads it as the isolation every checkout falls back to.
  it("reads a missing choice by what the target can inherit, not by what it is called", async () => {
    const callRpc = vi.fn(async (method, params) => params);
    await mount({ rpc: "t", params: {}, inherits: false, inheritLabel: "Account default (Git worktree)" }, {}, callRpc);

    expect(select().value).toBe("worktree");

    await choose("worktree");

    expect(callRpc).toHaveBeenCalledWith("t", { isolation: "worktree" });
  });

  // The account has no row to be painted from, so the control reads the account
  // setting itself — and then owns that read's failure, the way every other
  // panel on the settings page owns its own.
  it("reads the account's setting itself when the caller hands it none", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "rift", isolation_available: { rift: true, reason: null } }));
    document.body.innerHTML = isolationPanelHtml();
    await mountIsolation(document.body, { callRpc, target: DEVICE_ISOLATION });

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(select().value).toBe("rift");
    expect(select().disabled).toBe(false);
  });

  it("keeps a refused read to itself, in the bridge's words, and offers no choice", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("the bridge is offline");
    });
    document.body.innerHTML = isolationPanelHtml();
    await mountIsolation(document.body, { callRpc, target: DEVICE_ISOLATION });

    expect(error().textContent).toBe("the bridge is offline");
    expect(select().disabled).toBe(true);
  });

  it("paints the choice it was handed, without asking the bridge again", async () => {
    const callRpc = vi.fn();
    await mount(DEVICE_ISOLATION, { isolation: "rift", isolation_available: { rift: true, reason: null } }, callRpc);

    expect(callRpc).not.toHaveBeenCalled();
    expect(select().disabled).toBe(false);
    expect(select().value).toBe("rift");
    expect(lock().textContent).toBe("");
  });

  it("sends the account's choice to its own target and repaints from the answer", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "rift", isolation_available: { rift: true } }));
    await mount(DEVICE_ISOLATION, { isolation: "worktree", isolation_available: { rift: true } }, callRpc);

    await choose("rift");

    expect(callRpc).toHaveBeenCalledWith("settings.set", { isolation: "rift" });
    expect(select().value).toBe("rift");
    expect(select().disabled).toBe(false);
    expect(saved().textContent).toContain("Saved");
    expect(error().textContent).toBe("");
  });

  // The bridge is the authority: if it answers with something other than what
  // was chosen — a downgrade, a value another device wrote — that is what the
  // control shows.
  it("lands on what the bridge answered, not on what was chosen", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "worktree", isolation_available: { rift: true } }));
    await mount(DEVICE_ISOLATION, { isolation: "worktree", isolation_available: { rift: true } }, callRpc);

    await choose("rift");

    expect(select().value).toBe("worktree");
  });

  it("says a refused save in the bridge's own words and puts the control back", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("Rift isolation is unavailable: Rift CLI was not found; locked to worktrees");
    });
    await mount(DEVICE_ISOLATION, { isolation: "worktree", isolation_available: { rift: true } }, callRpc);

    await choose("rift");

    expect(error().textContent).toContain("locked to worktrees");
    expect(saved().textContent).toBe("");
    expect(select().value).toBe("worktree");
    expect(select().disabled).toBe(false);
  });

  it("shows why Rift is unavailable and still lets a worktree be chosen", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "worktree", isolation_available: { rift: false, reason: "Rift CLI was not found" } }));
    await mount(
      DEVICE_ISOLATION,
      { isolation: "worktree", isolation_available: { rift: false, reason: "Rift CLI was not found" } },
      callRpc,
    );

    expect(lock().textContent).toBe("Rift is unavailable on this device: Rift CLI was not found.");
    expect(select().disabled).toBe(false);
    expect([...select().options].map((option) => option.disabled)).toEqual([false, true]);

    await choose("worktree");

    expect(callRpc).toHaveBeenCalledWith("settings.set", { isolation: "worktree" });
    expect(error().textContent).toBe("");
  });

  it("renders the bridge's lock sentence as words, never as markup", async () => {
    const host = await mount(
      DEVICE_ISOLATION,
      { isolation: "worktree", isolation_available: { rift: false, reason: '<img src=x onerror="boom">' } },
      vi.fn(),
    );

    expect(lock().textContent).toContain('<img src=x onerror="boom">');
    expect(host.querySelector("img")).toBe(null);
  });

  it("leads a project with the device default it can fall back to", async () => {
    const row = { project_id: "p1", isolation: null, isolation_default: "rift", isolation_available: { rift: true } };
    await mount(projectIsolationTarget(row), row, vi.fn());

    expect([...select().options].map((option) => option.value)).toEqual(["", "worktree", "rift"]);
    expect(select().options[0].textContent).toBe("Device default (Rift (copy-on-write))");
    expect(select().value).toBe("");
  });

  it("clears a project's override by sending nothing in its place", async () => {
    const row = { project_id: "p1", isolation: "rift", isolation_default: "worktree", isolation_available: { rift: true } };
    const callRpc = vi.fn(async () => ({ ...row, isolation: null }));
    await mount(projectIsolationTarget(row), row, callRpc);

    expect(select().value).toBe("rift");

    await choose("");

    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "p1", isolation: null });
    expect(select().value).toBe("");
  });

  it("sends a project's own choice keyed on the project", async () => {
    const row = { project_id: "p1", isolation: null, isolation_default: "worktree", isolation_available: { rift: true } };
    const callRpc = vi.fn(async () => ({ ...row, isolation: "rift" }));
    await mount(projectIsolationTarget(row), row, callRpc);

    await choose("rift");

    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "p1", isolation: "rift" });
    expect(select().value).toBe("rift");
  });

  it("puts a refused project back on the override it still has", async () => {
    const row = { project_id: "p1", isolation: "worktree", isolation_default: "worktree", isolation_available: { rift: false, reason: "r" } };
    const callRpc = vi.fn(async () => {
      throw new Error("Rift isolation is unavailable: r; locked to worktrees");
    });
    await mount(projectIsolationTarget(row), row, callRpc);

    await choose("rift");

    expect(error().textContent).toContain("locked to worktrees");
    expect(select().value).toBe("worktree");
  });
});

// And where it does not live: the account's own page. How a checkout is made
// depends on the volume the project sits on and on what that machine has
// installed, so it is a machine's choice — the account page links to each
// machine's page and offers none of it itself.
describe("the account's settings page", () => {
  it("keeps device-owned isolation out of account settings", async () => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    // The devices panel and the downloads block talk HTTP, not a bridge.
    // Nothing here is about them, and a real request from jsdom hangs until the
    // test's own deadline.
    globalThis.fetch = vi.fn(async () => {
      throw new Error("no network in tests");
    });
    const { renderSettings } = await import("../src/views/settings.js");

    await renderSettings();
    await flush();

    expect(document.querySelector("#root #creationdev")).toBeTruthy(); // the page did stand up
    expect(document.querySelector("#root [data-isolation=select]")).toBe(null);
  }, SLOW_IMPORT_MS);
});

// Where a machine's default lives: its own settings page, directly under the
// agent it starts new work with, painted from that machine's bridge.
describe("the device's settings page", () => {
  const renderWith = async (call) => (await renderDeviceSettingsPage(call, openSession)).call;

  const settingsCall = (settings, projects = []) => async (method) => {
    if (method === "project.list") return { projects };
    if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude", ...settings };
    if (method === "models.list") return { default_provider: "claude", providers: [] };
    return {};
  };

  it("puts the isolation panel directly under the fallback agent, on what the machine holds", async () => {
    await renderWith(settingsCall({ isolation: "rift", isolation_available: { rift: true, reason: null } }));

    const headings = [...document.querySelectorAll("#root .panel h3")].map((h) => h.textContent);
    const at = (word) => headings.findIndex((heading) => heading.includes(word));
    expect(at("Work isolation")).toBe(at("Fallback agent") + 1);
    expect(document.querySelector("#root [data-isolation=select]").value).toBe("rift");
    expect(document.querySelector("#root [data-isolation=select]").disabled).toBe(false);
  }, SLOW_IMPORT_MS);

  it("shows the volume's lock rather than a choice this device cannot keep", async () => {
    await renderWith(
      settingsCall({ isolation: "worktree", isolation_available: { rift: false, reason: "no reflink support here" } }),
    );

    expect(document.querySelector("#root [data-isolation=lock]").textContent).toBe(
      "Rift is unavailable on this device: no reflink support here.",
    );
    const options = [...document.querySelector("#root [data-isolation=select]").options];
    expect(options.map((option) => option.disabled)).toEqual([false, true]);
  }, SLOW_IMPORT_MS);

  it("saves a chosen isolation through the machine's own method", async () => {
    const call = await renderWith(settingsCall({ isolation: "worktree", isolation_available: { rift: true } }));
    const select = document.querySelector("#root [data-isolation=select]");

    select.value = "rift";
    select.dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(call).toHaveBeenCalledWith("settings.set", { isolation: "rift" });
  }, SLOW_IMPORT_MS);

  // Every panel on this page owns its own bridge read: one read the bridge
  // refuses takes down the panel that made it and nothing else.
  it("survives a read the bridge refuses, panel by panel", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "models.list") throw new Error("the catalog is unavailable");
      return { projects_dir: "/p", default_harness: "claude", isolation: "rift", isolation_available: { rift: true } };
    });

    expect(document.getElementById("harnesserr").textContent).toBe("the catalog is unavailable");
    expect(document.querySelector("#root [data-isolation=select]").value).toBe("rift");
    expect(document.querySelector("#root [data-isolation=select]").disabled).toBe(false);
    expect(document.querySelector("#device-projects-path").textContent).toBe("/p");
  }, SLOW_IMPORT_MS);

  it("names on every project row what that project will actually do", async () => {
    await renderWith(
      settingsCall({ isolation: "worktree", isolation_available: { rift: true } }, [
        { project_id: "p1", name: "build", path: "/p/build", base_branch: "main", isolation_effective: "rift" },
        { project_id: "p2", name: "relay", path: "/p/relay", base_branch: "main", isolation_effective: "worktree" },
      ]),
    );

    const rows = [...document.querySelectorAll("#projlist .projrow")].map((row) => row.textContent);
    expect(rows[0]).toContain("Rift (copy-on-write)");
    expect(rows[1]).toContain("Git worktree");
  }, SLOW_IMPORT_MS);
});
