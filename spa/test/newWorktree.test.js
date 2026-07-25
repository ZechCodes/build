// @vitest-environment jsdom
// Naming a worktree: the picker cards, the branch preview, and the one-shot
// hand-off that opens the chosen tool when the surface mounts.

import { describe, it, expect } from "vitest";
import { kindCardsHtml, NEW_TAB_KINDS } from "../src/core/surfaceTabs.js";
import { markNewWorktree, takeNewWorktreeMark } from "../src/core/newWorktree.js";
import { slugPreview } from "../src/sheets/newWorktree.js";

const memoryStorage = () => {
  const map = new Map();
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, v),
    removeItem: (k) => map.delete(k),
  };
};

describe("kindCardsHtml", () => {
  it("offers every kind as a card carrying its id", () => {
    const html = kindCardsHtml();
    for (const kind of NEW_TAB_KINDS) {
      expect(html).toContain(`data-kind="${kind.id}"`);
      expect(html).toContain(kind.label);
    }
  });

  it("escapes option strings", () => {
    const html = kindCardsHtml([{ id: "x", label: "<b>x</b>", description: "<img src=x>" }]);
    expect(html).not.toContain("<b>x</b>");
    expect(html).not.toContain("<img");
  });
});

describe("slugPreview", () => {
  // Mirrors the daemon's slugifier so the branch is not a surprise after the
  // fact. The daemon is still the one that decides.
  it("matches the daemon's rules: lowercase alphanumerics and single hyphens", () => {
    expect(slugPreview("Mascot Model Spike!")).toBe("mascot-model-spike");
    expect(slugPreview("  Add OAuth!! support  ")).toBe("add-oauth-support");
    expect(slugPreview("feat/thing")).toBe("feat-thing");
  });

  it("cannot produce a separator, a dot segment or a leading dash", () => {
    for (const raw of ["../../etc/passwd", "-rf", "a/../b", "..."]) {
      const slug = slugPreview(raw);
      expect(slug).not.toMatch(/[/.]/);
      expect(slug.startsWith("-")).toBe(false);
    }
  });

  it("gives nothing back for a name with nothing in it (the daemon refuses those)", () => {
    expect(slugPreview("***")).toBe("");
    expect(slugPreview("   ")).toBe("");
  });
});

describe("the new-worktree hand-off", () => {
  it("carries the chosen tool to the surface exactly once", () => {
    const storage = memoryStorage();
    markNewWorktree("wt-1", "codex", storage);
    expect(takeNewWorktreeMark("wt-1", storage)).toBe("codex");
    expect(takeNewWorktreeMark("wt-1", storage)).toBeNull();
  });

  it("marks nothing without both a worktree and a tool", () => {
    const storage = memoryStorage();
    markNewWorktree("wt-1", null, storage);
    markNewWorktree(null, "claude", storage);
    expect(takeNewWorktreeMark("wt-1", storage)).toBeNull();
  });

  it("survives a storage that throws (private mode) by opening on the default tab", () => {
    const hostile = {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
      removeItem() {},
    };
    expect(() => markNewWorktree("wt-1", "shell", hostile)).not.toThrow();
    expect(takeNewWorktreeMark("wt-1", hostile)).toBeNull();
  });
});
