// @vitest-environment jsdom
// The shells type at the machine the surface on screen is about. A route change
// is one of the two ways that machine moves (a home move is the other), and
// render() is where the app hears about it — but only a change moves them: the
// socket has to drop to re-point, and dropping it re-attaches every open tab.

import { beforeEach, describe, expect, it, vi } from "vitest";

const terminals = vi.hoisted(() => ({ followTerminalDevice: vi.fn() }));

vi.mock("../src/terminal/manager.js", () => ({
  followTerminalDevice: (...args) => terminals.followTerminalDevice(...args),
}));
// The surfaces themselves are their own files' business; this one is about what
// render() does on the way in.
vi.mock("../src/views/inbox.js", () => ({ renderInbox: () => {} }));
vi.mock("../src/views/branchView.js", () => ({ renderBranch: () => {} }));
vi.mock("../src/views/issueView.js", () => ({ renderIssue: () => {} }));
vi.mock("../src/core/inboxShell.js", () => ({ inboxRouteChanged: () => {} }));
vi.mock("../src/core/toolbar.js", () => ({ toolbarRouteChanged: () => {} }));

const { App, render } = await import("../src/app.js");

const branchOn = (deviceId, tab = "changes") => ({
  name: "branch",
  deviceId,
  projectId: "p1",
  branch: "main",
  tab,
});

beforeEach(() => {
  document.body.innerHTML = '<div id="root"></div>';
  App.gated = false;
  App.poll = null;
  App.viewDispose = null;
  App.route = { name: "inbox" };
  render(); // settle on "no device named", whatever the last case left
  terminals.followTerminalDevice.mockClear();
});

describe("the device a rendered route names", () => {
  it("takes the terminals to it", () => {
    App.route = branchOn("dev-b");

    render();

    expect(terminals.followTerminalDevice).toHaveBeenCalledTimes(1);
  });

  it("leaves them where they are while the route stays on that device", () => {
    App.route = branchOn("dev-b");
    render();
    terminals.followTerminalDevice.mockClear();

    App.route = branchOn("dev-b", "files");
    render();
    App.route = { name: "issue", deviceId: "dev-b", projectId: "p1", id: "i-1" };
    render();

    expect(terminals.followTerminalDevice).not.toHaveBeenCalled();
  });

  it("takes them back to the home device when the route names none", () => {
    App.route = branchOn("dev-b");
    render();
    terminals.followTerminalDevice.mockClear();

    App.route = { name: "inbox" };
    render();

    expect(terminals.followTerminalDevice).toHaveBeenCalledTimes(1);
  });
});
