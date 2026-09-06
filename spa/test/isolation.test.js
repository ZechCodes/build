// @vitest-environment jsdom
// How a task's checkout is isolated: a git worktree of the project repository,
// or a copy-on-write clone of the whole project directory.
//
// The choice is the account's, overridable per project, and locked to git
// worktrees on a volume that cannot clone — the bridge decides that and hands
// back the sentence saying why. This module is the client's only naming table
// for the two, so the settings panel and the project sheet can never call the
// same isolation two different things, and neither learns a variant name, a
// label or a locked look.

import { describe, it, expect, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  ISOLATIONS,
  isolationLabel,
  isolationOf,
  isolationLockReason,
  isolationOptionsHtml,
  isolationPanelHtml,
  ACCOUNT_ISOLATION,
  projectIsolationTarget,
  mountIsolation,
} from "../src/core/isolation.js";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

const optionsOf = (html) => {
  const select = document.createElement("select");
  select.innerHTML = html;
  return [...select.options];
};

describe("the isolation a payload names", () => {
  it("takes the bridge's word when it names one", () => {
    expect(isolationOf({ isolation: "cow" })).toBe("cow");
    expect(isolationOf({ isolation: "worktree" })).toBe("worktree");
  });

  it("reads silence and nonsense as a git worktree rather than nothing", () => {
    expect(isolationOf({})).toBe("worktree");
    expect(isolationOf({ isolation: "nope" })).toBe("worktree");
    expect(isolationOf(undefined)).toBe("worktree");
    expect(isolationOf(null)).toBe("worktree");
  });
});

describe("the reason a volume locks the choice", () => {
  it("is empty while this device can clone", () => {
    expect(isolationLockReason({ cow: true, reason: null })).toBe("");
    expect(isolationLockReason(undefined)).toBe("");
    expect(isolationLockReason(null)).toBe("");
  });

  it("is the bridge's own sentence when it cannot", () => {
    expect(isolationLockReason({ cow: false, reason: "the worktrees folder is on another volume" })).toBe(
      "the worktrees folder is on another volume",
    );
  });

  it("still says something when the bridge sends no sentence", () => {
    expect(isolationLockReason({ cow: false })).toBe("unavailable on this device");
  });
});

describe("the isolation options", () => {
  it("names both isolations, in the naming table's order, marking the chosen one", () => {
    const options = optionsOf(isolationOptionsHtml("cow", { cow: true }));

    expect(options.map((option) => option.value)).toEqual(["worktree", "cow"]);
    expect(options.map((option) => option.textContent)).toEqual(["Git worktree", "Copy-on-write clone"]);
    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual(["cow"]);
  });

  it("disables the clone, and only the clone, when the volume locks it", () => {
    const options = optionsOf(isolationOptionsHtml("worktree", { cow: false, reason: "no reflink support here" }));

    expect(options.map((option) => option.disabled)).toEqual([false, true]);
    expect(options[1].title).toBe("no reflink support here");
  });

  it("escapes the bridge's sentence rather than rendering it", () => {
    const html = isolationOptionsHtml("worktree", { cow: false, reason: '<img src=x onerror="boom">' });

    expect(html).not.toContain("<img");
    expect(optionsOf(html)[1].title).toBe('<img src=x onerror="boom">');
  });

  it("leads with the inherit option when asked, and marks it when nothing is overridden", () => {
    const options = optionsOf(isolationOptionsHtml(null, { cow: true }, { inheritLabel: "Account default (Git worktree)" }));

    expect(options.map((option) => option.value)).toEqual(["", "worktree", "cow"]);
    expect(options[0].textContent).toBe("Account default (Git worktree)");
    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual([""]);
  });

  it("marks the override, not the inherit option, once a project has one", () => {
    const options = optionsOf(isolationOptionsHtml("worktree", { cow: true }, { inheritLabel: "Account default (Copy-on-write clone)" }));

    expect(options.filter((option) => option.selected).map((option) => option.value)).toEqual(["worktree"]);
  });

  it("escapes an inherit label too", () => {
    const html = isolationOptionsHtml(null, { cow: true }, { inheritLabel: "<b>Account default</b>" });

    expect(html).not.toContain("<b>");
    expect(optionsOf(html)[0].textContent).toBe("<b>Account default</b>");
  });

  it("offers no inherit option to the account, which has nothing to inherit from", () => {
    expect(optionsOf(isolationOptionsHtml("worktree", { cow: true })).map((option) => option.value)).toEqual([
      "worktree",
      "cow",
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
      "A copy-on-write clone starts with the project's build caches already in place and keeps its own git repository. A git worktree shares the project's repository and starts empty.",
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
    expect(ACCOUNT_ISOLATION).toEqual({ rpc: "settings.set", params: {}, inherits: false, inheritLabel: null });
  });

  it("keys a project's choice on its own id and names the account default it replaces", () => {
    expect(projectIsolationTarget({ project_id: "p1", isolation_default: "cow" })).toEqual({
      rpc: "project.set_isolation",
      params: { project_id: "p1" },
      inherits: true,
      inheritLabel: "Account default (Copy-on-write clone)",
    });
  });

  it("names a git worktree as the default the bridge did not name", () => {
    expect(projectIsolationTarget({ project_id: "p2" }).inheritLabel).toBe("Account default (Git worktree)");
    expect(projectIsolationTarget({ project_id: "p2", isolation_default: "nope" }).inheritLabel).toBe(
      "Account default (Git worktree)",
    );
  });
});

describe("the naming table", () => {
  it("is the one place either isolation is given a name", () => {
    expect(ISOLATIONS).toEqual([
      { id: "worktree", label: "Git worktree" },
      { id: "cow", label: "Copy-on-write clone" },
    ]);
  });

  it("gives a wire word its name, and a word it cannot read a git worktree's", () => {
    expect(isolationLabel("cow")).toBe("Copy-on-write clone");
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
    const callRpc = vi.fn(async () => ({ isolation: "cow", isolation_available: { cow: true, reason: null } }));
    document.body.innerHTML = isolationPanelHtml();
    await mountIsolation(document.body, { callRpc, target: ACCOUNT_ISOLATION });

    expect(callRpc).toHaveBeenCalledWith("settings.get");
    expect(select().value).toBe("cow");
    expect(select().disabled).toBe(false);
  });

  it("keeps a refused read to itself, in the bridge's words, and offers no choice", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("the bridge is offline");
    });
    document.body.innerHTML = isolationPanelHtml();
    await mountIsolation(document.body, { callRpc, target: ACCOUNT_ISOLATION });

    expect(error().textContent).toBe("the bridge is offline");
    expect(select().disabled).toBe(true);
  });

  it("paints the choice it was handed, without asking the bridge again", async () => {
    const callRpc = vi.fn();
    await mount(ACCOUNT_ISOLATION, { isolation: "cow", isolation_available: { cow: true, reason: null } }, callRpc);

    expect(callRpc).not.toHaveBeenCalled();
    expect(select().disabled).toBe(false);
    expect(select().value).toBe("cow");
    expect(lock().textContent).toBe("");
  });

  it("sends the account's choice to its own target and repaints from the answer", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "cow", isolation_available: { cow: true } }));
    await mount(ACCOUNT_ISOLATION, { isolation: "worktree", isolation_available: { cow: true } }, callRpc);

    await choose("cow");

    expect(callRpc).toHaveBeenCalledWith("settings.set", { isolation: "cow" });
    expect(select().value).toBe("cow");
    expect(select().disabled).toBe(false);
    expect(saved().textContent).toContain("Saved");
    expect(error().textContent).toBe("");
  });

  // The bridge is the authority: if it answers with something other than what
  // was chosen — a downgrade, a value another device wrote — that is what the
  // control shows.
  it("lands on what the bridge answered, not on what was chosen", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "worktree", isolation_available: { cow: true } }));
    await mount(ACCOUNT_ISOLATION, { isolation: "worktree", isolation_available: { cow: true } }, callRpc);

    await choose("cow");

    expect(select().value).toBe("worktree");
  });

  it("says a refused save in the bridge's own words and puts the control back", async () => {
    const callRpc = vi.fn(async () => {
      throw new Error("copy-on-write isolation is unavailable: no reflink support here; locked to worktrees");
    });
    await mount(ACCOUNT_ISOLATION, { isolation: "worktree", isolation_available: { cow: true } }, callRpc);

    await choose("cow");

    expect(error().textContent).toContain("locked to worktrees");
    expect(saved().textContent).toBe("");
    expect(select().value).toBe("worktree");
    expect(select().disabled).toBe(false);
  });

  it("shows the volume's lock under the select and still lets a worktree be chosen", async () => {
    const callRpc = vi.fn(async () => ({ isolation: "worktree", isolation_available: { cow: false, reason: "no reflink support here" } }));
    await mount(
      ACCOUNT_ISOLATION,
      { isolation: "worktree", isolation_available: { cow: false, reason: "no reflink support here" } },
      callRpc,
    );

    expect(lock().textContent).toBe("Locked to git worktrees on this device: no reflink support here.");
    expect(select().disabled).toBe(false);
    expect([...select().options].map((option) => option.disabled)).toEqual([false, true]);

    await choose("worktree");

    expect(callRpc).toHaveBeenCalledWith("settings.set", { isolation: "worktree" });
    expect(error().textContent).toBe("");
  });

  it("renders the bridge's lock sentence as words, never as markup", async () => {
    const host = await mount(
      ACCOUNT_ISOLATION,
      { isolation: "worktree", isolation_available: { cow: false, reason: '<img src=x onerror="boom">' } },
      vi.fn(),
    );

    expect(lock().textContent).toContain('<img src=x onerror="boom">');
    expect(host.querySelector("img")).toBe(null);
  });

  it("leads a project with the account default it can fall back to", async () => {
    const row = { project_id: "p1", isolation: null, isolation_default: "cow", isolation_available: { cow: true } };
    await mount(projectIsolationTarget(row), row, vi.fn());

    expect([...select().options].map((option) => option.value)).toEqual(["", "worktree", "cow"]);
    expect(select().options[0].textContent).toBe("Account default (Copy-on-write clone)");
    expect(select().value).toBe("");
  });

  it("clears a project's override by sending nothing in its place", async () => {
    const row = { project_id: "p1", isolation: "cow", isolation_default: "worktree", isolation_available: { cow: true } };
    const callRpc = vi.fn(async () => ({ ...row, isolation: null }));
    await mount(projectIsolationTarget(row), row, callRpc);

    expect(select().value).toBe("cow");

    await choose("");

    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "p1", isolation: null });
    expect(select().value).toBe("");
  });

  it("sends a project's own choice keyed on the project", async () => {
    const row = { project_id: "p1", isolation: null, isolation_default: "worktree", isolation_available: { cow: true } };
    const callRpc = vi.fn(async () => ({ ...row, isolation: "cow" }));
    await mount(projectIsolationTarget(row), row, callRpc);

    await choose("cow");

    expect(callRpc).toHaveBeenCalledWith("project.set_isolation", { project_id: "p1", isolation: "cow" });
    expect(select().value).toBe("cow");
  });

  it("puts a refused project back on the override it still has", async () => {
    const row = { project_id: "p1", isolation: "worktree", isolation_default: "worktree", isolation_available: { cow: false, reason: "r" } };
    const callRpc = vi.fn(async () => {
      throw new Error("copy-on-write isolation is unavailable: r; locked to worktrees");
    });
    await mount(projectIsolationTarget(row), row, callRpc);

    await choose("cow");

    expect(error().textContent).toContain("locked to worktrees");
    expect(select().value).toBe("worktree");
  });
});

// Where the account's own choice lives: Account -> Settings, directly under the
// agent it starts new work on, painted from the same bridge every device reads.
describe("the Settings page", () => {
  const renderWith = async (call) => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    // The devices panel talks HTTP, not the bridge. Nothing here is about it,
    // and a real request from jsdom hangs until the test's own deadline.
    globalThis.fetch = vi.fn(async () => {
      throw new Error("no network in tests");
    });
    const { App } = await import("../src/app.js");
    const { renderSettings } = await import("../src/views/settings.js");
    App.call = vi.fn(call);
    await renderSettings();
    await flush();
    return App.call;
  };

  // Re-importing the whole app shell can outrun the default deadline on a
  // loaded machine.
  const SLOW_IMPORT_MS = 30000;

  const settingsCall = (settings, projects = []) => async (method) => {
    if (method === "project.list") return { projects };
    if (method === "settings.get") return { projects_dir: "/p", default_harness: "claude", ...settings };
    if (method === "models.list") return { default_provider: "claude", providers: [] };
    return {};
  };

  it("puts the isolation panel directly under the fallback agent, on what the account holds", async () => {
    await renderWith(settingsCall({ isolation: "cow", isolation_available: { cow: true, reason: null } }));

    const headings = [...document.querySelectorAll("#root .panel h3")].map((h) => h.textContent);
    const at = (word) => headings.findIndex((heading) => heading.includes(word));
    expect(at("Work isolation")).toBe(at("Fallback agent") + 1);
    expect(document.querySelector("#root [data-isolation=select]").value).toBe("cow");
    expect(document.querySelector("#root [data-isolation=select]").disabled).toBe(false);
  }, SLOW_IMPORT_MS);

  it("shows the volume's lock rather than a choice this device cannot keep", async () => {
    await renderWith(
      settingsCall({ isolation: "worktree", isolation_available: { cow: false, reason: "no reflink support here" } }),
    );

    expect(document.querySelector("#root [data-isolation=lock]").textContent).toBe(
      "Locked to git worktrees on this device: no reflink support here.",
    );
    const options = [...document.querySelector("#root [data-isolation=select]").options];
    expect(options.map((option) => option.disabled)).toEqual([false, true]);
  }, SLOW_IMPORT_MS);

  it("saves a chosen isolation through the account's own method", async () => {
    const call = await renderWith(settingsCall({ isolation: "worktree", isolation_available: { cow: true } }));
    const select = document.querySelector("#root [data-isolation=select]");

    select.value = "cow";
    select.dispatchEvent(new Event("change"));
    await flush();
    await flush();

    expect(call).toHaveBeenCalledWith("settings.set", { isolation: "cow" });
  }, SLOW_IMPORT_MS);

  // Every panel on this page owns its own bridge read: a settings.get the
  // bridge refuses takes down the isolation panel and nothing else — not the
  // devices list, which is api-backed and lives while the bridge is gone.
  it("survives a settings read the bridge refuses, panel by panel", async () => {
    await renderWith(async (method) => {
      if (method === "project.list") return { projects: [] };
      if (method === "models.list") return { default_provider: "claude", providers: [] };
      throw new Error("the bridge is offline");
    });

    expect(document.querySelector("#root [data-isolation=error]").textContent).toBe("the bridge is offline");
    expect(document.querySelector("#root [data-isolation=select]").disabled).toBe(true);
    expect(document.querySelector("#pushtoggle").textContent).not.toBe("checking…");
    expect(document.querySelector("#devlist").textContent).not.toContain("loading…");
    expect(document.querySelector("#themepick")).not.toBe(null);
  }, SLOW_IMPORT_MS);

  it("names on every project row what that project will actually do", async () => {
    await renderWith(
      settingsCall({ isolation: "worktree", isolation_available: { cow: true } }, [
        { project_id: "p1", name: "build", path: "/p/build", base_branch: "main", isolation_effective: "cow" },
        { project_id: "p2", name: "relay", path: "/p/relay", base_branch: "main", isolation_effective: "worktree" },
      ]),
    );

    const rows = [...document.querySelectorAll("#projlist .projrow")].map((row) => row.textContent);
    expect(rows[0]).toContain("Copy-on-write clone");
    expect(rows[1]).toContain("Git worktree");
  }, SLOW_IMPORT_MS);
});
