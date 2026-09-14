// @vitest-environment jsdom
// The inbox has a stable global trigger, a transient floating card and a
// separate persisted pin choice.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const indexSource = readFileSync(resolve("index.html"), "utf8");
const shellCss = readFileSync(resolve("src/styles/shell.css"), "utf8");
const bodyHtml = indexSource.match(/<body>([\s\S]*)<\/body>/)[1];

vi.mock("../src/app.js", () => ({
  App: { route: { name: "inbox" } },
  go: vi.fn(),
}));
vi.mock("../src/core/inboxView.js", () => ({
  mountInboxList: vi.fn(),
  inboxListRouteChanged: vi.fn(),
  openNewProject: vi.fn(),
  setInboxView: vi.fn(),
}));

const hover = (element, type) =>
  element.dispatchEvent(new window.MouseEvent(type, { bubbles: false }));

let setInboxCollapsed, initInboxRail;
let publishInboxAttentionCount;
let hoverCapability;

function mediaCapability(matches) {
  const listeners = new Set();
  return {
    matches,
    addEventListener: vi.fn((type, listener) => {
      if (type === "change") listeners.add(listener);
    }),
    change(next) {
      this.matches = next;
      listeners.forEach((listener) => listener({ matches: next }));
    },
  };
}

beforeEach(async () => {
  vi.useFakeTimers();
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  localStorage.clear();
  hoverCapability = mediaCapability(true);
  vi.stubGlobal("matchMedia", vi.fn(() => hoverCapability));
  ({ setInboxCollapsed, initInboxRail } = await import("../src/core/inboxShell.js"));
  ({ publishInboxAttentionCount } = await import("../src/core/inboxAttention.js"));
  initInboxRail();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("the inbox head", () => {
  it("keeps the global inbox control outside the persistent rail content", () => {
    const head = document.querySelector(".inbox-head");
    const order = [...head.children].map((child) => child.id || child.className);
    expect(order).toEqual(["inbox-views", "inbox-new-project", "inbox-collapse"]);
    expect(document.querySelector("#global-controls #inbox-open")).toBeTruthy();
    expect(document.getElementById("inbox-new-project").textContent).toContain("New project");
    expect(document.getElementById("inbox-collapse").querySelector("svg")).toBeTruthy();
  });

  it("does not render global branding", () => {
    expect(document.querySelector("#global-controls .logo")).toBeNull();
    expect(document.querySelector("#global-controls .brand-mark")).toBeNull();
  });
});

describe("the collapsed rail's hover peek", () => {
  beforeEach(() => {
    setInboxCollapsed(true);
    // Past the after-collapse window in which a hover is treated as the
    // browser's re-hit-test artifact rather than a request.
    vi.advanceTimersByTime(400);
  });

  it("ignores the hover the browser synthesizes right after a collapse", () => {
    // Collapsing puts the floating toggle under the pointer that clicked, and
    // the browser fires mouseenter on it without the pointer moving. That
    // hover must not peek the rail straight back open.
    setInboxCollapsed(true);
    hover(document.getElementById("inbox-open"), "mouseenter");
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
  });

  it("lays the rail over the view while the pointer is on the toggle, without docking it", () => {
    hover(document.getElementById("inbox-open"), "mouseenter");
    expect(document.body.classList.contains("inbox-peek")).toBe(true);
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    // The stored choice is untouched: a peek is not a decision.
    expect(localStorage.getItem("build.inbox.collapsed")).toBe("1");
  });

  it("does not peek for compatibility mouse events on a touch-only device", async () => {
    vi.resetModules();
    document.body.innerHTML = bodyHtml;
    document.body.className = "";
    localStorage.clear();
    hoverCapability = mediaCapability(false);
    vi.stubGlobal("matchMedia", vi.fn(() => hoverCapability));
    ({ setInboxCollapsed, initInboxRail } = await import("../src/core/inboxShell.js"));
    initInboxRail();
    setInboxCollapsed(true);
    vi.advanceTimersByTime(400);

    const toggle = document.getElementById("inbox-open");
    hover(toggle, "mouseenter");
    hover(toggle, "mouseleave");

    expect(document.body.classList.contains("inbox-peek")).toBe(false);
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    expect(matchMedia).toHaveBeenCalledWith("(hover: hover) and (pointer: fine)");

    toggle.click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    expect(document.body.classList.contains("inbox-popover-open")).toBe(false);
  });

  it("ends a live peek and cancels its close timer when hover capability is lost", () => {
    const toggle = document.getElementById("inbox-open");
    hover(toggle, "mouseenter");
    hover(toggle, "mouseleave");
    expect(document.body.classList.contains("inbox-peek")).toBe(true);

    hoverCapability.change(false);
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
    hover(toggle, "mouseenter");
    expect(document.body.classList.contains("inbox-peek")).toBe(false);

    hoverCapability.change(true);
    hover(toggle, "mouseenter");
    expect(document.body.classList.contains("inbox-peek")).toBe(true);
    vi.advanceTimersByTime(150);
    expect(document.body.classList.contains("inbox-peek")).toBe(true);
  });

  it("puts the rail away when the pointer leaves the toggle", () => {
    const toggle = document.getElementById("inbox-open");
    hover(toggle, "mouseenter");
    hover(toggle, "mouseleave");
    vi.runAllTimers();
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
  });

  it("keeps the rail out while the pointer crosses from the toggle onto it", () => {
    const toggle = document.getElementById("inbox-open");
    const rail = document.getElementById("inbox-rail");
    hover(toggle, "mouseenter");
    hover(toggle, "mouseleave");
    hover(rail, "mouseenter");
    vi.runAllTimers();
    expect(document.body.classList.contains("inbox-peek")).toBe(true);
    hover(rail, "mouseleave");
    vi.runAllTimers();
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
  });

  it("opens the floating rail on trigger click without changing its pin choice", () => {
    const toggle = document.getElementById("inbox-open");
    hover(toggle, "mouseenter");
    toggle.click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
    expect(document.body.classList.contains("inbox-popover-open")).toBe(false);
    expect(localStorage.getItem("build.inbox.collapsed")).toBe("1");
  });

  it("replaces the reopen icon with the aggregate count and resets at zero", () => {
    const toggle = document.getElementById("inbox-open");
    publishInboxAttentionCount(3);
    expect(toggle.classList.contains("has-attention")).toBe(true);
    expect(toggle.querySelector(".inbox-open-count").textContent).toBe("3");
    expect(toggle.getAttribute("aria-label")).toContain("3 unread notifications");

    publishInboxAttentionCount(0);
    expect(toggle.classList.contains("has-attention")).toBe(false);
    expect(toggle.querySelector(".inbox-open-count").textContent).toBe("");
    expect(toggle.getAttribute("aria-label")).toContain("the inbox");
  });

  it("keeps large counts inside the fixed-width toggle and still reopens", () => {
    const toggle = document.getElementById("inbox-open");
    publishInboxAttentionCount(137);
    expect(toggle.querySelector(".inbox-open-count").textContent).toBe("99+");
    expect(toggle.getAttribute("aria-label")).toContain("137 unread notifications");
    toggle.click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  });

  it("docks the rail on a click of the head toggle inside a peek", () => {
    hover(document.getElementById("inbox-open"), "mouseenter");
    document.getElementById("inbox-collapse").click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(false);
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
  });
});

describe("the docked rail's head toggle", () => {
  it("gives the persistent trigger a truthful navigation label while pinned", () => {
    setInboxCollapsed(false);
    expect(document.getElementById("inbox-open").getAttribute("aria-label")).toBe("Go to inbox");
    expect(document.getElementById("inbox-open").getAttribute("aria-expanded")).toBe("true");
  });

  it("collapses the rail", () => {
    setInboxCollapsed(false);
    document.getElementById("inbox-collapse").click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  });

  it("does not let the hidden pin change state on a narrow viewport", () => {
    vi.stubGlobal("innerWidth", 900);
    setInboxCollapsed(true, { persist: false, reveal: false });
    document.getElementById("inbox-collapse").click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
  });
});

describe("the header toggle beside a pinned rail", () => {
  it("is gone while the rail is pinned, and back once it is away", () => {
    setInboxCollapsed(false);
    expect(document.getElementById("inbox-open").hidden).toBe(true);
    setInboxCollapsed(true);
    expect(document.getElementById("inbox-open").hidden).toBe(false);
  });

  it("stays the way back where the rail overlays the view instead", () => {
    vi.stubGlobal("innerWidth", 900);
    setInboxCollapsed(false);
    expect(document.getElementById("inbox-open").hidden).toBe(false);
  });
});

describe("the floating card", () => {
  it("does not persist or animate the responsive startup state", () => {
    expect(localStorage.getItem("build.inbox.collapsed")).toBeNull();
    expect(document.body.classList.contains("inbox-transitioning")).toBe(false);
  });

  it("dismisses on Escape, restores trigger focus, and preserves the pin choice", () => {
    setInboxCollapsed(true);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.body.classList.contains("inbox-popover-open")).toBe(false);
    expect(document.activeElement).toBe(document.getElementById("inbox-open"));
    expect(localStorage.getItem("build.inbox.collapsed")).toBe("1");
  });

  it("dismisses the transient rail before opening the new-project sheet", () => {
    setInboxCollapsed(true);
    const saved = localStorage.getItem("build.inbox.collapsed");
    document.getElementById("inbox-new-project").click();
    expect(document.body.classList.contains("inbox-popover-open")).toBe(false);
    expect(document.body.classList.contains("inbox-peek")).toBe(false);
    expect(localStorage.getItem("build.inbox.collapsed")).toBe(saved);
  });

  it("dismisses a docked rail after a resize to the mobile breakpoint", () => {
    vi.stubGlobal("innerWidth", 900);
    setInboxCollapsed(false);
    const saved = localStorage.getItem("build.inbox.collapsed");
    document.getElementById("inbox-new-project").click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    expect(localStorage.getItem("build.inbox.collapsed")).toBe(saved);
  });

  it("lets the mobile scrim dismiss a pinned overlay without changing its saved choice", () => {
    vi.stubGlobal("innerWidth", 760);
    setInboxCollapsed(false);
    document.getElementById("inbox-scrim").click();
    expect(document.body.classList.contains("inbox-collapsed")).toBe(true);
    expect(localStorage.getItem("build.inbox.collapsed")).toBe("");
  });

  it("leaves Escape to a higher-priority surface when it was prevented", () => {
    setInboxCollapsed(true);
    const event = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    event.preventDefault();
    document.dispatchEvent(event);
    expect(document.body.classList.contains("inbox-popover-open")).toBe(true);
  });

  it("keeps the rail node and scroll position across pin changes", () => {
    const rail = document.getElementById("inbox-rail");
    const list = document.getElementById("inbox-list");
    list.scrollTop = 37;
    setInboxCollapsed(true);
    document.getElementById("inbox-collapse").click();
    expect(document.getElementById("inbox-rail")).toBe(rail);
    expect(list.scrollTop).toBe(37);
  });

  it("cancels an in-flight geometry animation when pinning is reversed", () => {
    hoverCapability.matches = false;
    const rail = document.getElementById("inbox-rail");
    rail.getBoundingClientRect = vi.fn(() => ({ left: 0, top: 0, width: 288, height: 700 }));
    const animations = [];
    rail.animate = vi.fn(() => {
      const animation = { cancel: vi.fn(() => animation.oncancel?.()), oncancel: null, onfinish: null };
      animations.push(animation);
      return animation;
    });
    setInboxCollapsed(true);
    setInboxCollapsed(false);
    vi.advanceTimersByTime(240);
    expect(animations).toHaveLength(1);
    animations[0].onfinish();
    expect(document.body.classList.contains("inbox-transitioning")).toBe(false);
  });
});

describe("the peek's geometry", () => {
  it("anchors the collapsed rail to the shell above the view", () => {
    const floating = shellCss.match(/body\.inbox-collapsed #inbox-rail \{[^}]*\}/)[0];
    expect(floating).toMatch(/position:absolute/);
    expect(floating).toMatch(/z-index:46/);
  });

  it("keeps the icon centered and exposes attention as a separate badge", () => {
    expect(shellCss).toMatch(/#inbox-open \{[^}]*place-items:center/);
    expect(shellCss).toMatch(/#inbox-open \{[^}]*background:none; border:0/);
    expect(shellCss).toMatch(/\.inbox-open-icon \{[^}]*place-items:center/);
    expect(shellCss).toMatch(/\.inbox-open-icon, \.inbox-open-count \{[^}]*transition:/);
    expect(shellCss).toMatch(/@media \(prefers-reduced-motion: reduce\)/);
    expect(shellCss).toMatch(/#inbox-open\.has-attention \.inbox-open-count \{[^}]*position:absolute/);
    expect(shellCss).not.toMatch(/inbox-toggle-bubble|inbox-icon-away/);
  });
});
