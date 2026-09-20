/** @vitest-environment jsdom */
// An agent acting on an issue, narrated in its own conversation as one line.
//
// It is a message like any other — it reads in sequence and counts as unread —
// and it is the agent saying what it just did, so it survives every detail
// level including the narrowest.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";
import { itemsAtDetailLevel } from "../src/core/conversationDetail.js";
import { actionHref, actionWord, issueActionLineHtml } from "../src/core/trackerActionLine.js";

const HERE = { deviceId: "dev-1", projectId: "proj-1" };

const action = (over = {}) => ({
  action: "commented",
  issue_id: "issue-01M2ZN6P",
  number: 14,
  title: "Activity entry for issues in the conversation",
  ...over,
});

const acted = (over = {}, message = {}) => ({
  type: "message",
  data: {
    id: "message-9",
    sequence: 9,
    role: "agent",
    body: "",
    issue_action: action(over),
    ...message,
  },
});

const paint = (items, place = HERE) => {
  document.body.innerHTML = threadHtml({ id: "conversation-3", items }, { place });
  return document.querySelector(".thread-issue-action");
};

describe("the line", () => {
  it("reads as one sentence in the agent's voice", () => {
    const line = paint([acted()]);
    expect(line.textContent.replace(/\s+/g, " ").trim())
      .toBe("commented on #14 Activity entry for issues in the conversation");
  });

  // Zech's four, plus the rest of the board's verbs. A bare token and a past
  // tense both read, because the bridge may send either.
  it("renders each action", () => {
    const said = (name) => paint([acted({ action: name })]).textContent.replace(/\s+/g, " ").trim().split(" #")[0];
    expect(said("created")).toBe("created");
    expect(said("create")).toBe("created");
    expect(said("assigned")).toBe("assigned");
    expect(said("updated")).toBe("updated");
    expect(said("commented")).toBe("commented on");
    expect(said("moved")).toBe("moved");
    expect(said("closed")).toBe("closed");
  });

  // A later verb should leave a legible line, not a blank one.
  it("says an action it has never heard of as itself", () => {
    expect(actionWord("escalated")).toBe("escalated");
    expect(actionWord("")).toBe("acted on");
  });

  it("is one anchor over the whole line, not several", () => {
    const line = paint([acted()]);
    expect(line.tagName).toBe("A");
    expect(line.querySelectorAll("a")).toHaveLength(0);
  });
});

describe("where it goes", () => {
  it("opens the issue on this machine", () => {
    expect(paint([acted({ comment_id: null })]).getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/issues/issue-01M2ZN6P");
  });

  // A comment is a place in the issue, not just the issue.
  it("lands on the comment when the action was one", () => {
    expect(paint([acted({ comment_id: "ic-01M2ZNVB" })]).getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/issues/issue-01M2ZN6P#comment-ic-01M2ZNVB");
  });

  it("encodes a comment id that carries a separator", () => {
    expect(actionHref(action({ comment_id: "a/b" }), HERE))
      .toBe("#/device/dev-1/project/proj-1/issues/issue-01M2ZN6P#comment-a%2Fb");
  });

  // A conversation rendered with nowhere to stand points nowhere rather than
  // at a broken route.
  it("draws as plain text where there is no project to open", () => {
    const line = paint([acted()], { deviceId: null, projectId: null });
    expect(line.tagName).not.toBe("A");
    expect(line.textContent).toContain("#14");
  });

  it("draws nothing for an action naming no issue", () => {
    expect(issueActionLineHtml(null)).toBe("");
    expect(issueActionLineHtml({ number: 14 })).toBe("");
  });
});

describe("it is a message like any other", () => {
  const items = () => [
    acted(),
    { type: "message", data: { id: "m1", sequence: 10, role: "agent", body: "and here is what I found" } },
    { type: "event", data: { sequence: 11, event: "tool_use", summary: "Read a file" } },
  ];

  // "Treated as a message from the agent, so it should show up in all
  // conversation view modes."
  it("survives all three detail levels", () => {
    for (const level of ["all", "messages", "agent"]) {
      const kept = itemsAtDetailLevel(items(), level);
      expect(kept.some((one) => one.data?.issue_action)).toBe(true);
    }
  });

  // The narrowest level is exactly "what this agent did", which is what this
  // line says — so it must not depend on which fields the bridge happens to
  // set alongside it.
  it("survives Agent only even when the record also carries a from_agent", () => {
    const withSender = acted({}, { from_agent: { id: "agent-01M2OTHER" } });
    expect(itemsAtDetailLevel([withSender], "agent")).toHaveLength(1);
  });

  it("rides its sequence, so it reads in order and counts as unread", () => {
    paint([acted()]);
    expect(document.querySelector(".thread-message").getAttribute("data-sequence")).toBe("9");
  });

  it("leaves a message carrying no action completely alone", () => {
    document.body.innerHTML = threadHtml(
      { id: "conversation-3", items: [{ type: "message", data: { id: "m1", sequence: 1, role: "agent", body: "hello" } }] },
      { place: HERE },
    );
    expect(document.querySelector(".thread-issue-action")).toBeNull();
    expect(document.querySelector(".thread-body").textContent).toContain("hello");
  });
});

describe("the comment it points at", () => {
  it("is a row the issue page answers to by id", async () => {
    const { issuePageHtml } = await import("../src/core/trackerIssueRender.js");
    const { timelineRows } = await import("../src/core/trackerTimeline.js");
    const { issue, comment } = await import("./trackerWireFixture.js");
    const rows = timelineRows([comment({ id: "ic-01M2ZNVB" })]);
    document.body.innerHTML = issuePageHtml(issue(), { columns: null, rows, links: [], draft: "", labelsDraft: "" });
    expect(document.querySelector("#comment-ic-01M2ZNVB")).not.toBeNull();
  });
});
