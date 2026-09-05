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

import { describe, it, expect } from "vitest";
import {
  ISOLATIONS,
  isolationOf,
  isolationLockReason,
  isolationOptionsHtml,
  isolationPanelHtml,
  ACCOUNT_ISOLATION,
  projectIsolationTarget,
} from "../src/core/isolation.js";

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
    expect(ACCOUNT_ISOLATION).toEqual({ rpc: "settings.set", params: {}, inheritLabel: null });
  });

  it("keys a project's choice on its own id and names the account default it replaces", () => {
    expect(projectIsolationTarget({ project_id: "p1", isolation_default: "cow" })).toEqual({
      rpc: "project.set_isolation",
      params: { project_id: "p1" },
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
});
