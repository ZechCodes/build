// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { docErrorPaneHtml } from "../src/core/taskRender.js";
import { shouldFetchPlanDoc, planDocPaneState } from "../src/core/taskActions.js";

// The doc-read error pane carries an inline Retry affordance (W15) instead of the
// old "Reopen the plan/stage to retry" copy. The button ids let the task view
// wire the latch-clear + refetch.
describe("docErrorPaneHtml (doc-read Retry)", () => {
  it("offers an inline Retry button for the plan doc", () => {
    const html = docErrorPaneHtml("plan");
    expect(html).toContain('class="plan-empty warn"');
    expect(html).toContain('id="docretry"');
    expect(html).toContain("Retry");
    expect(html).toContain("the stage plan document");
    expect(html).not.toContain("Reopen");
  });

  it("offers an inline Retry button for a stage doc with a distinct id", () => {
    const html = docErrorPaneHtml("stage");
    expect(html).toContain('id="stagedocretry"');
    expect(html).toContain("Retry");
    expect(html).toContain("this stage document");
    expect(html).not.toContain("Reopen");
  });
});

// The doc-fetch guard and pane-state decision that fix the "loading forever"
// bug: docs that predate canonical storage (docs_available false) and docs whose
// read has errored (latched) are never refetched, and the pane renders an honest
// state instead of the loading placeholder.
describe("plan doc fetch guard (shouldFetchPlanDoc)", () => {
  it("fetches when docs are available and no read has errored", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: true, errorLatched: false })).toBe(true);
    expect(shouldFetchPlanDoc({ docsAvailable: undefined, errorLatched: false })).toBe(true);
  });

  it("never fetches a doc that predates canonical storage", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: false, errorLatched: false })).toBe(false);
  });

  it("never refetches a doc whose read has errored (latched off)", () => {
    expect(shouldFetchPlanDoc({ docsAvailable: true, errorLatched: true })).toBe(false);
  });
});

describe("plan doc pane state (planDocPaneState)", () => {
  it("is unavailable when the docs predate canonical storage — ahead of any error/contents", () => {
    expect(planDocPaneState({ docsAvailable: false, errorLatched: false, hasContents: false })).toBe("unavailable");
    expect(planDocPaneState({ docsAvailable: false, errorLatched: true, hasContents: true })).toBe("unavailable");
  });

  it("is error when a read has errored and is latched", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: true, hasContents: false })).toBe("error");
  });

  it("is ready when contents are in hand", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: false, hasContents: true })).toBe("ready");
  });

  it("is loading while still awaiting the first successful read", () => {
    expect(planDocPaneState({ docsAvailable: true, errorLatched: false, hasContents: false })).toBe("loading");
  });
});
