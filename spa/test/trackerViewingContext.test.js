/** @vitest-environment jsdom */
// Telling the project agent which issue the reader has open — and the gate
// that decides whether it may be told at all.
//
// A bridge refuses a viewing context carrying a kind it does not know, and the
// refusal takes the whole message with it. So the gate is not about a missing
// feature; it is about not losing what the user typed.

import { describe, expect, it, vi } from "vitest";
import { normalizeViewingContext } from "../src/core/viewingContext.js";

let apiVersion = "1.4.0";
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeApiVersion: () => apiVersion,
}));

const { ISSUE_CONTEXT_SINCE, carriesIssueContext, issueContextItem } = await import(
  "../src/core/trackerViewingContext.js"
);

const issue = (over = {}) => ({
  id: "issue-01M2ZS29",
  number: 21,
  title: "The issue page stamps the issue on what the user sends",
  ...over,
});

describe("the item on the wire", () => {
  const normalize = (item) => normalizeViewingContext([item])?.items?.[0] ?? null;

  it("carries the issue, its number and its title", () => {
    expect(normalize({ kind: "issue", issue_id: "issue-1", number: 21, title: "A title" })).toEqual({
      kind: "issue", issue_id: "issue-1", number: 21, title: "A title",
    });
  });

  // The number is what a person says out loud, so it has to survive as one.
  it("refuses a number that names nothing", () => {
    for (const number of [0, -1, 1.5, "21", null, undefined, NaN]) {
      expect(normalize({ kind: "issue", issue_id: "issue-1", number, title: "A title" })).toBeNull();
    }
  });

  it("refuses an issue with no id or no title", () => {
    expect(normalize({ kind: "issue", number: 21, title: "A title" })).toBeNull();
    expect(normalize({ kind: "issue", issue_id: "issue-1", number: 21 })).toBeNull();
  });

  // The bridge's own label limit, the one a workspace name is held to.
  it("refuses a title past the label limit rather than sending it clipped", () => {
    const tooLong = "x".repeat(513);
    expect(normalize({ kind: "issue", issue_id: "issue-1", number: 21, title: tooLong })).toBeNull();
    expect(normalize({ kind: "issue", issue_id: "issue-1", number: 21, title: "x".repeat(512) })).not.toBeNull();
  });

  it("measures that limit in bytes, not characters", () => {
    // Four bytes each, so 128 of them is exactly the limit and 129 is past it.
    expect(normalize({ kind: "issue", issue_id: "i", number: 1, title: "😀".repeat(128) })).not.toBeNull();
    expect(normalize({ kind: "issue", issue_id: "i", number: 1, title: "😀".repeat(129) })).toBeNull();
  });

  it("leaves every other kind exactly as it was", () => {
    expect(normalize({ kind: "workspace", workspace_id: "ws-1", name: "wire-facade" })).toEqual({
      kind: "workspace", workspace_id: "ws-1", name: "wire-facade",
    });
    expect(normalize({ kind: "sandwiches", issue_id: "i" })).toBeNull();
  });
});

describe("the gate", () => {
  // The minor #13 introduces and #20 lands the item at. It was held at null
  // until that number was reported rather than inferred: guess low and every
  // message from the issue page is refused whole, the user's words with it.
  it("stands at the minor the item landed in", () => {
    expect(ISSUE_CONTEXT_SINCE).toBe("1.5.0");
  });

  it("opens at that minor and above, on the constant alone", () => {
    apiVersion = "1.5.0";
    expect(carriesIssueContext("dev-1")).toBe(true);
    apiVersion = "1.6.0";
    expect(carriesIssueContext("dev-1")).toBe(true);
    apiVersion = "2.0.0";
    expect(carriesIssueContext("dev-1")).toBe(true);
  });

  it("stays shut below it", () => {
    apiVersion = "1.4.0";
    expect(carriesIssueContext("dev-1")).toBe(false);
    expect(issueContextItem(issue(), "dev-1")).toBeNull();
    apiVersion = "1.2.0";
    expect(carriesIssueContext("dev-1")).toBe(false);
  });

  // A threshold nobody has named yet sends nothing to anybody — the state this
  // gate spent its first day in, kept because it is the safe default.
  it("sends nothing at all when no minor is named", () => {
    apiVersion = "9.9.9";
    expect(carriesIssueContext("dev-1", null)).toBe(false);
    expect(issueContextItem(issue(), "dev-1", null)).toBeNull();
  });

  // Every unknown answers no: the cost of a wrong yes is the user's message.
  it("refuses on anything it cannot read", () => {
    apiVersion = "1.5.0";
    expect(carriesIssueContext(null, "1.5.0")).toBe(false);
    apiVersion = "";
    expect(carriesIssueContext("dev-1", "1.5.0")).toBe(false);
    apiVersion = "not a version";
    expect(carriesIssueContext("dev-1", "1.5.0")).toBe(false);
  });
});

describe("the item the page would send", () => {
  it("names the issue once the bridge can take it", () => {
    apiVersion = "1.5.0";
    expect(issueContextItem(issue(), "dev-1")).toEqual({
      kind: "issue",
      issue_id: "issue-01M2ZS29",
      number: 21,
      title: "The issue page stamps the issue on what the user sends",
    });
  });

  // A page that stamps a half-read issue tells the agent a number with no
  // title behind it.
  it("says nothing about an issue that has not been read", () => {
    apiVersion = "1.5.0";
    expect(issueContextItem(null, "dev-1")).toBeNull();
    expect(issueContextItem({ id: "issue-1" }, "dev-1")).toBeNull();
  });
});
