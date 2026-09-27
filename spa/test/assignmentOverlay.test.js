// @vitest-environment jsdom
// The assignment overlay: the handoff's fields, over the page rather than
// inside the rail. What is tested here is the overlay as its own thing —
// where it places itself, that it repaints without disturbing the field the
// user is in, and that every way out of it is a way out.

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { openAssignmentOverlay, panelPlacement, NARROW_VIEWPORT } from "../src/core/assignmentOverlay.js";

const catalog = { default_provider: "claude", providers: [{ id: "claude", label: "Claude Code", models: ["sonnet"], efforts: ["high"] }] };

const baseAssignment = { worktree: "new", agent: "new", base: "", provider: "claude", model: "", effort: "" };

/** Open the overlay over a live anchor, holding the assignment the way the
 *  task view does: the caller owns it, the overlay only hands back the next. */
function open(overrides = {}) {
  const anchor = document.createElement("button");
  document.body.appendChild(anchor);
  let assignment = { ...baseAssignment, ...(overrides.assignment || {}) };
  const closes = [];
  const overlay = openAssignmentOverlay({
    getAnchor: () => anchor,
    getAssignment: () => assignment,
    setAssignment: (next) => {
      assignment = next;
    },
    getCatalog: () => catalog,
    getWorktrees: () => overrides.worktrees || [],
    onClose: () => closes.push(true),
  });
  return { anchor, overlay, closes, held: () => assignment };
}

describe("panelPlacement", () => {
  it("hangs the panel under the control it was opened from", () => {
    const at = panelPlacement({ top: 400, bottom: 424, left: 40 }, { width: 1400, height: 900 }, 300);
    expect(at.atBottom).toBe(false);
    expect(at.top).toBe(430);
    expect(at.left).toBe(40);
  });

  it("flips above the control when the space under it cannot hold the panel", () => {
    // The control sits at the FOOT of the rail, so this is the ordinary case,
    // not the edge one.
    const at = panelPlacement({ top: 800, bottom: 824, left: 40 }, { width: 1400, height: 900 }, 300);
    expect(at.top).toBe(494);
    expect(at.top + 300).toBeLessThanOrEqual(800);
  });

  it("keeps the panel inside the frame when the control is against the right edge", () => {
    const at = panelPlacement({ top: 100, bottom: 124, left: 1380 }, { width: 1400, height: 900 }, 300);
    expect(at.left + at.width).toBeLessThanOrEqual(1400);
  });

  it("stands the panel on the bottom edge of a phone instead of anchoring it", () => {
    const at = panelPlacement({ top: 400, bottom: 424, left: 8 }, { width: NARROW_VIEWPORT, height: 780 }, 300);
    expect(at.atBottom).toBe(true);
  });
});

describe("the assignment overlay", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("paints the fields over the page, outside the rail that opened it", () => {
    const { overlay } = open();
    const panel = document.querySelector(".assign-pop");
    expect(panel).toBeTruthy();
    expect(panel.getAttribute("role")).toBe("dialog");
    expect(panel.querySelector("#assignworktree")).toBeTruthy();
    expect(panel.querySelector("#assignprovider")).toBeTruthy();
    overlay.close();
  });

  it("hands each choice back to its owner and repaints on the shape it changes", () => {
    const { overlay, held } = open({ worktrees: [{ id: "wt-1", label: "feature-x" }] });
    const worktree = document.querySelector("#assignworktree");
    worktree.value = "existing";
    worktree.dispatchEvent(new Event("change"));
    expect(held().worktree).toBe("existing");
    // The branch picker replaces the base-branch field the moment an existing
    // checkout is the target.
    expect(document.querySelector("#assignworktreeid")).toBeTruthy();
    expect(document.querySelector("#assignbase")).toBeNull();
    overlay.close();
  });

  it("leaves the field the user is in exactly where it was when nothing changed", () => {
    const { overlay } = open();
    const base = document.querySelector("#assignbase");
    base.focus();
    base.value = "release";
    base.dispatchEvent(new Event("input"));
    overlay.update();
    expect(document.querySelector("#assignbase")).toBe(base);
    expect(document.activeElement).toBe(base);
    expect(base.value).toBe("release");
    overlay.close();
  });

  it("leaves the catalog's default provider standing when the assignment names none", () => {
    // An assignment with no provider on it is not a blank picker — it is the
    // catalog's default, and the field has to say which one that is.
    const { overlay } = open({ assignment: { provider: undefined } });
    expect(document.querySelector("#assignprovider").value).toBe("claude");
    expect(document.querySelector("#assignprovider").selectedOptions[0].textContent).toBe("Claude Code");
    overlay.close();
  });

  it("offers the two agents, out of the four harnesses the bridge serves", () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);
    const overlay = openAssignmentOverlay({
      getAnchor: () => anchor,
      getAssignment: () => ({ ...baseAssignment, provider: "claude" }),
      setAssignment: () => {},
      getCatalog: () => ({
        default_provider: "claude_adk",
        providers: [
          { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
          { id: "claude", label: "Claude Code", models: [], efforts: [] },
          { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
          { id: "codex", label: "Codex TUI", models: [], efforts: [] },
        ],
      }),
      getWorktrees: () => [],
    });

    const options = [...document.querySelector("#assignprovider").options];
    expect(options.map((option) => option.value)).toEqual(["claude_adk", "codex"]);
    expect(options.map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
    // A stored preference for the carrier no longer offered clamps onto the
    // entry that is, rather than leaving the field on a name it cannot show.
    expect(document.querySelector("#assignprovider").value).toBe("claude_adk");
    overlay.close();
  });

  it("shows the catalog the moment it arrives, without losing a choice already made", () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);
    let assignment = { ...baseAssignment, provider: undefined };
    let held = {};
    const overlay = openAssignmentOverlay({
      getAnchor: () => anchor,
      getAssignment: () => assignment,
      setAssignment: (next) => {
        assignment = next;
      },
      getCatalog: () => held,
      getWorktrees: () => [],
    });
    // Before it lands the picker still offers both agents — dispatching needs
    // only a harness — with no models under either.
    expect([...document.querySelector("#assignprovider").options].map((option) => option.value)).toEqual(["claude_adk", "codex"]);
    const worktree = document.querySelector("#assignworktree");
    worktree.value = "existing";
    worktree.dispatchEvent(new Event("change"));
    held = {
      default_provider: "claude",
      providers: [
        ...catalog.providers,
        { id: "codex_app_server", label: "Codex", models: [], efforts: [] },
        { id: "codex", label: "Codex TUI", models: [], efforts: [] },
      ],
    };
    overlay.update();
    expect([...document.querySelector("#assignprovider").options].map((option) => option.value)).toEqual(["claude", "codex"]);
    expect(document.querySelector("#assignworktree").value).toBe("existing");
    overlay.close();
  });

  it("reports itself busy while the user is in one of its fields", () => {
    const { overlay } = open();
    expect(overlay.busy()).toBe(false);
    document.querySelector("#assignworktree").focus();
    expect(overlay.busy()).toBe(true);
    overlay.close();
  });

  it("closes on Done, on a press outside, and on Escape — each exactly once", () => {
    for (const dismiss of [
      () => document.querySelector("[data-assign-close]").click(),
      () => document.querySelector(".assign-scrim").dispatchEvent(new MouseEvent("click", { bubbles: true })),
      () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
    ]) {
      document.body.innerHTML = "";
      const { closes } = open();
      dismiss();
      expect(document.querySelector(".assign-pop")).toBeNull();
      expect(closes).toHaveLength(1);
    }
  });

  it("ignores a press that lands inside the panel", () => {
    const { closes, overlay } = open();
    document.querySelector("#assignworktree").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.querySelector(".assign-pop")).toBeTruthy();
    expect(closes).toHaveLength(0);
    overlay.close();
  });

  it("closes once however many times it is asked", () => {
    const { overlay, closes } = open();
    overlay.close();
    overlay.close();
    expect(closes).toHaveLength(1);
    expect(overlay.isOpen()).toBe(false);
  });
});
