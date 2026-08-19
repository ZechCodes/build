import { describe, it, expect } from "vitest";
import {
  supportsRepoManagement,
  syncChipState,
  toolbarControlsDisabled,
  actionSettleReenables,
  settleReenableSelectors,
  pullSplitOptions,
  pushSplitOptions,
  stashSplitOptions,
  syncActionRpc,
  resolveInlineConfirm,
  confirmExpired,
  INLINE_CONFIRM_TTL_MS,
  repoStateBanner,
  pollRenderFrozen,
  outsidePressDismisses,
} from "../src/core/gitPane.js";
import { gitToolbarHtml, gitStateBannerHtml } from "../src/core/gitRender.js";

// ---- decision helpers --------------------------------------------------

describe("supportsRepoManagement", () => {
  it("is true only when the additive repo_state field is present", () => {
    expect(supportsRepoManagement({ repo_state: "clean" })).toBe(true);
    expect(supportsRepoManagement({ repo_state: "merging" })).toBe(true);
  });

  it("is false for an older bridge that omits repo_state", () => {
    expect(supportsRepoManagement({ branch: "main" })).toBe(false);
    expect(supportsRepoManagement({})).toBe(false);
    expect(supportsRepoManagement(null)).toBe(false);
    expect(supportsRepoManagement({ repo_state: 3 })).toBe(false);
  });
});

describe("syncChipState", () => {
  it("returns ahead/behind counts when an upstream is tracked", () => {
    expect(syncChipState({ repo_state: "clean", upstream: "origin/main", ahead: 2, behind: 1 })).toEqual({ ahead: 2, behind: 1 });
  });

  it("hides chips when there is no upstream", () => {
    expect(syncChipState({ repo_state: "clean", upstream: null, ahead: null, behind: null })).toBeNull();
  });

  it("hides chips when the additive fields are absent (older bridge)", () => {
    expect(syncChipState({ branch: "main" })).toBeNull();
    expect(syncChipState({ repo_state: "clean", upstream: "origin/main" })).toBeNull();
  });
});

describe("toolbarControlsDisabled", () => {
  it("disables every toolbar verb while an action is in flight", () => {
    expect(toolbarControlsDisabled(1)).toBe(true);
    expect(toolbarControlsDisabled(2)).toBe(true);
  });

  it("enables them when nothing is in flight", () => {
    expect(toolbarControlsDisabled(0)).toBe(false);
  });
});

describe("actionSettleReenables", () => {
  it("re-enables controls only once the last in-flight action settles", () => {
    expect(actionSettleReenables(0)).toBe(true);
    expect(actionSettleReenables(1)).toBe(false);
    expect(actionSettleReenables(2)).toBe(false);
  });
});

describe("settleReenableSelectors", () => {
  it("covers every toolbar verb", () => {
    const selectors = settleReenableSelectors();
    // The bar's verbs are mini buttons now, so the selectors reach every .btn
    // under the sync/stash hosts rather than a primary that is no longer there.
    for (const sel of [".gtfetch", ".gtsync .btn", ".gtstash .btn"]) expect(selectors).toContain(sel);
  });

  it("also re-enables the commit primary — a toolbar action's repaint disables it, so its settle must revive it (S1)", () => {
    // The whole S1 bug: runGuarded settled without ever re-enabling the commit
    // button. The unified settle list MUST include it, or a Fetch/Push leaves
    // Commit stuck disabled.
    expect(settleReenableSelectors()).toContain(".gitcommit-actions .btn.primary:not(.caret)");
  });
});

describe("pullSplitOptions", () => {
  it("leads with the fast-forward primary then merge and rebase", () => {
    expect(pullSplitOptions().map((o) => o.id)).toEqual(["pull", "pull_merge", "pull_rebase"]);
    expect(pullSplitOptions()[0].label).toBe("Pull");
  });
});

describe("pushSplitOptions", () => {
  it("leads with Push then a danger-styled force-push", () => {
    const options = pushSplitOptions();
    expect(options.map((o) => o.id)).toEqual(["push", "force_push"]);
    expect(options[0].label).toBe("Push");
    expect(options.find((o) => o.id === "force_push").danger).toBe(true);
  });

  it("labels the force-push item plainly when disarmed", () => {
    expect(pushSplitOptions(false).find((o) => o.id === "force_push").menuLabel).toBe("Force push (with lease)");
    expect(pushSplitOptions().find((o) => o.id === "force_push").menuLabel).toBe("Force push (with lease)");
  });

  it("makes the arming VISIBLE — the force-push item reads a confirm prompt when armed (S2a)", () => {
    const armed = pushSplitOptions(true).find((o) => o.id === "force_push");
    expect(armed.menuLabel).toBe("Confirm force push?");
    expect(armed.danger).toBe(true);
  });
});

describe("confirmExpired", () => {
  it("has a 10-second inline-confirm TTL", () => {
    expect(INLINE_CONFIRM_TTL_MS).toBe(10000);
  });

  it("is never expired when nothing is armed", () => {
    expect(confirmExpired(null, 999999)).toBe(false);
    expect(confirmExpired(undefined, 999999)).toBe(false);
  });

  it("holds the arm within the TTL window", () => {
    expect(confirmExpired(1000, 1000)).toBe(false);
    expect(confirmExpired(1000, 1000 + INLINE_CONFIRM_TTL_MS - 1)).toBe(false);
  });

  it("expires the arm at or past the TTL (the poll disarms + repaints) (S2c)", () => {
    expect(confirmExpired(1000, 1000 + INLINE_CONFIRM_TTL_MS)).toBe(true);
    expect(confirmExpired(1000, 1000 + INLINE_CONFIRM_TTL_MS + 1)).toBe(true);
  });
});

describe("stashSplitOptions", () => {
  it("badges the pop option with the stash count only when positive", () => {
    expect(stashSplitOptions(0).map((o) => o.id)).toEqual(["stash", "stash_pop"]);
    expect(stashSplitOptions(0)[1].menuLabel).toBe("Pop stash");
    expect(stashSplitOptions(3)[1].menuLabel).toBe("Pop stash (3)");
    expect(stashSplitOptions(undefined)[1].menuLabel).toBe("Pop stash");
  });
});

describe("syncActionRpc", () => {
  it.each([
    ["fetch", "git.fetch", {}],
    ["pull", "git.pull", {}],
    ["pull_merge", "git.pull", { mode: "merge" }],
    ["pull_rebase", "git.pull", { mode: "rebase" }],
    ["push", "git.push", {}],
    ["force_push", "git.push", { force: true }],
    ["stash", "git.stash", {}],
    ["stash_pop", "git.stash_pop", {}],
  ])("maps %s to its RPC", (optionId, method, params) => {
    expect(syncActionRpc(optionId)).toEqual({ method, params });
  });

  it("returns a fresh params object each call (no shared mutation)", () => {
    const a = syncActionRpc("pull_merge");
    a.params.mode = "tampered";
    expect(syncActionRpc("pull_merge").params.mode).toBe("merge");
  });

  it("throws on an unknown action", () => {
    expect(() => syncActionRpc("nope")).toThrow();
  });
});

describe("resolveInlineConfirm", () => {
  it("arms on the first touch of a control", () => {
    expect(resolveInlineConfirm(null, "discard:a.js")).toEqual({ fire: false, pending: "discard:a.js" });
  });

  it("fires on a second touch of the same control and disarms", () => {
    expect(resolveInlineConfirm("discard:a.js", "discard:a.js")).toEqual({ fire: true, pending: null });
  });

  it("re-arms on a different control rather than firing the old one", () => {
    expect(resolveInlineConfirm("discard:a.js", "abort")).toEqual({ fire: false, pending: "abort" });
  });
});

describe("repoStateBanner", () => {
  it.each([
    ["merging", "Merge in progress"],
    ["rebasing", "Rebase in progress"],
    ["cherry-picking", "Cherry-pick in progress"],
    ["reverting", "Revert in progress"],
    ["bisecting", "Bisect in progress"],
  ])("describes %s and offers abort", (state, opening) => {
    expect(repoStateBanner(state)).toEqual({ message: expect.stringContaining(opening), abortable: true });
  });

  it("names the operation-specific opening for every abortable state", () => {
    // Each in-progress op gets its own noun — not a generic 'operation in progress'.
    expect(repoStateBanner("cherry-picking").message).toBe("Cherry-pick in progress — resolve conflicts, then continue.");
    expect(repoStateBanner("reverting").message).toBe("Revert in progress — resolve conflicts, then continue.");
    expect(repoStateBanner("bisecting").message).toBe("Bisect in progress — resolve conflicts, then continue.");
  });

  it("surfaces a stash-pop-style conflict without offering abort (merge_abort would reject)", () => {
    const banner = repoStateBanner("conflicted");
    expect(banner.abortable).toBe(false);
    expect(banner.message).toContain("Conflicts in the working tree");
  });

  it("warns on an unusual state without offering abort (merge_abort would reject)", () => {
    const banner = repoStateBanner("other");
    expect(banner.abortable).toBe(false);
    expect(banner.message.length).toBeGreaterThan(0);
  });

  it("returns null for a clean repo and for an absent field", () => {
    expect(repoStateBanner("clean")).toBeNull();
    expect(repoStateBanner(undefined)).toBeNull();
  });
});

describe("pollRenderFrozen — interaction freeze", () => {
  it("freezes a rendered pane while a confirm is armed or a file menu is open", () => {
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: false, draftActive: false, actionInFlight: false, interactionActive: true })).toBe(true);
  });

  it("does not freeze the first paint on interaction alone", () => {
    expect(pollRenderFrozen({ paneRendered: false, keyUnchanged: false, draftActive: false, actionInFlight: false, interactionActive: true })).toBe(false);
  });

  it("still repaints when nothing is interactive", () => {
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: false, draftActive: false, actionInFlight: false, interactionActive: false })).toBe(false);
  });
});

describe("outsidePressDismisses", () => {
  it("never dismisses a press that lands inside the pane", () => {
    expect(outsidePressDismisses({ inside: true, hasPendingConfirm: true })).toBe(false);
  });

  it("dismisses an outside press while a confirm is armed — un-wedging the freeze (S5)", () => {
    expect(outsidePressDismisses({ inside: false, hasPendingConfirm: true })).toBe(true);
  });

  it("ignores an outside press when nothing is armed or open", () => {
    expect(outsidePressDismisses({ inside: false, hasPendingConfirm: false })).toBe(false);
  });
});

// ---- markup builders ---------------------------------------------------

describe("gitToolbarHtml", () => {
  it("renders fetch and the pull/push/stash split-button hosts", () => {
    const html = gitToolbarHtml({ chips: null });
    expect(html).toContain("gtfetch");
    expect(html).toContain('class="gtpull"');
    expect(html).toContain('class="gtpush"');
    expect(html).toContain('class="gtstash"');
  });

  it("renders ahead/behind chips only when chip data is supplied", () => {
    expect(gitToolbarHtml({ chips: { ahead: 3, behind: 4 } })).toContain("↑3");
    expect(gitToolbarHtml({ chips: { ahead: 3, behind: 4 } })).toContain("↓4");
    expect(gitToolbarHtml({ chips: null })).not.toContain("gtchips");
  });
});

describe("gitStateBannerHtml", () => {
  // gitStateBannerHtml is now a pure renderer for the decision object that
  // repoStateBanner produces — it owns no copy of its own. The controller feeds
  // it repoStateBanner(status.repo_state); these tests exercise that same chain.
  it("is empty when there is no banner (clean repo / absent field)", () => {
    expect(gitStateBannerHtml(repoStateBanner("clean"))).toBe("");
    expect(gitStateBannerHtml(repoStateBanner(undefined))).toBe("");
    expect(gitStateBannerHtml(null)).toBe("");
  });

  it("renders whatever message the decision carries (no second copy to drift)", () => {
    // The load-bearing guarantee: change repoStateBanner's copy and the rendered
    // banner changes with it, because gitStateBannerHtml has no map of its own.
    for (const state of ["merging", "rebasing", "cherry-picking", "reverting", "bisecting"]) {
      const banner = repoStateBanner(state);
      const html = gitStateBannerHtml(banner);
      expect(html).toContain("gitstate");
      expect(html).toContain(banner.message);
      expect(html).toContain("gitabort");
    }
  });

  it("renders a cherry-pick banner end-to-end (previously collapsed to 'other')", () => {
    const html = gitStateBannerHtml(repoStateBanner("cherry-picking"));
    expect(html).toContain("Cherry-pick in progress");
    expect(html).toContain("gitabort");
  });

  it("renders an ad-hoc decision object verbatim", () => {
    const html = gitStateBannerHtml({ message: "custom banner copy", abortable: false });
    expect(html).toContain("custom banner copy");
    expect(html).not.toContain("gitabort");
  });

  it("shows the armed abort label when confirming", () => {
    const html = gitStateBannerHtml(repoStateBanner("rebasing"), { pendingConfirm: "abort" });
    expect(html).toContain("armed");
    expect(html).toContain("Confirm");
  });

  it("omits the abort button for a non-abortable state (abort would reject)", () => {
    expect(gitStateBannerHtml(repoStateBanner("conflicted"))).not.toContain("gitabort");
    const other = gitStateBannerHtml(repoStateBanner("other"));
    expect(other).toContain("gitstate");
    expect(other).not.toContain("gitabort");
  });
});

describe("outsidePressDismisses — the file header's ⋯", () => {
  it("closes an abandoned file menu, which would otherwise freeze the poll", () => {
    expect(outsidePressDismisses({ inside: false, hasPendingConfirm: false, fileMenuOpen: true })).toBe(true);
    expect(outsidePressDismisses({ inside: true, hasPendingConfirm: false, fileMenuOpen: true })).toBe(false);
  });
});
