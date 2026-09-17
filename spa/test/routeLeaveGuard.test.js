// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { App, go, initRouter, unmountView } from "../src/app.js";
import { goFromInbox } from "../src/core/inboxShell.js";

const settleHashChange = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("route leave guards", () => {
  beforeEach(() => {
    // A work URL names the machine the project is on; a device-less one is a
    // question the app answers on the resolve hop, not a place to restore to.
    history.replaceState(null, "", "#/device/dev-1/project/p1/branch/build%2Fedit/files?path=notes.txt");
    App.routeLeaveGuard = null;
    App.poll = null;
    App.viewDispose = null;
  });

  // A guard belongs to the view that set it. The reader follows a device-less
  // in-app link out of a surface with unsaved work: the hash path asks the
  // guard, is allowed, and renders the resolve hop — which navigates on to the
  // work item the moment it knows where it is. Nothing cleared the departed
  // view's guard, so that second navigation asked the same guard again, and a
  // veto left the reader on "Opening…" with nowhere to go.
  it("lets the mounted view's guard go with the view", async () => {
    const guard = vi.fn(async () => false);
    App.route = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files" };
    App.viewingContext = { clear: vi.fn() };
    App.routeLeaveGuard = guard;

    unmountView();

    expect(App.routeLeaveGuard).toBe(null);
    expect(go({ name: "inbox" })).toBe(true);
    expect(guard).not.toHaveBeenCalled();
  });

  it("keeps the route, context, and edited buffer when navigation is cancelled", async () => {
    const originalRoute = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files", file: "notes.txt" };
    const clear = vi.fn();
    const draft = { value: "unsaved text" };
    App.route = originalRoute;
    App.viewingContext = { clear };
    App.routeLeaveGuard = vi.fn(async () => false);

    expect(await go({ name: "inbox" })).toBe(false);

    expect(App.route).toBe(originalRoute);
    expect(location.hash).toContain("build%2Fedit/files");
    expect(clear).not.toHaveBeenCalled();
    expect(draft.value).toBe("unsaved text");
  });

  it("shares one confirmation and lets the latest requested route win", async () => {
    let decide;
    App.route = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files" };
    App.viewingContext = { clear: vi.fn() };
    App.routeLeaveGuard = vi.fn(() => new Promise((resolve) => { decide = resolve; }));

    const first = go({ name: "inbox" });
    const latest = go({ name: "account" });
    expect(App.routeLeaveGuard).toHaveBeenCalledTimes(1);
    decide(true);
    await Promise.all([first, latest]);

    expect(App.route.name).toBe("account");
    expect(App.viewingContext.clear).toHaveBeenCalledTimes(1);
  });

  it("does not let a late approval overwrite a newer unguarded route", async () => {
    let approveOldRoute;
    App.route = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files" };
    App.viewingContext = { clear: vi.fn() };
    App.routeLeaveGuard = () => new Promise((resolve) => { approveOldRoute = resolve; });
    const oldNavigation = go({ name: "inbox" });

    App.routeLeaveGuard = null;
    expect(go({ name: "account" })).toBe(true);
    approveOldRoute(true);

    expect(await oldNavigation).toBe(false);
    expect(App.route.name).toBe("account");
  });

  it("keeps the mobile inbox open when guarded navigation is cancelled", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 390 });
    document.body.classList.remove("inbox-collapsed");
    App.route = { name: "branch", projectId: "p1", branch: "build/edit", tab: "files" };
    App.viewingContext = { clear: vi.fn() };
    App.routeLeaveGuard = vi.fn(async () => false);

    expect(await goFromInbox({ name: "inbox" })).toBe(false);
    expect(document.body.classList.contains("inbox-collapsed")).toBe(false);
  });

  it("guards browser hash navigation and restores the current URL on cancellation", async () => {
    App.gated = true;
    App.viewingContext = { clear: vi.fn() };
    initRouter();
    const originalRoute = App.route;
    App.routeLeaveGuard = vi.fn(async () => false);

    location.hash = "#/account";
    await settleHashChange();

    expect(App.route).toEqual(originalRoute);
    expect(location.hash).toContain("build%2Fedit/files");
    expect(App.viewingContext.clear).not.toHaveBeenCalled();
  });

  it("follows in-app links without adding history and still honors leave guards", async () => {
    App.gated = true;
    App.viewingContext = { clear: vi.fn() };
    initRouter();
    const entries = history.length;
    const link = document.createElement("a");
    link.href = "#/account";
    document.body.append(link);
    App.routeLeaveGuard = vi.fn(async () => false);
    link.click();
    await settleHashChange();
    expect(App.route.name).toBe("branch");
    expect(history.length).toBe(entries);

    App.routeLeaveGuard = null;
    link.click();
    expect(App.route.name).toBe("account");
    expect(location.hash).toBe("#/account/settings");
    expect(history.length).toBe(entries);
    link.remove();
  });
});
