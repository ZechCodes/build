/** @vitest-environment jsdom */
// A tracking notice as one line (#38).
//
// Zech, with a screenshot of a project-agent conversation: "Tracking notices
// come in looking like user messages (same color and on the right). They
// should be a single line 'X did Y on Z' deep linking."
//
// On the wire a notice IS a message on the user's side, so the two marks
// together are what tell it apart. Everything below is about the line it draws
// instead, and about the one thing that must survive a bridge that has not
// landed the structured field yet: the link.

import { describe, expect, it } from "vitest";
import { isIssueNotice, issueNoticeLineHtml, issueNoticeOf, noticeHref } from "../src/core/trackerNotice.js";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { threadHtml } from "../src/core/thread.js";
import { itemsAtDetailLevel } from "../src/core/conversationDetail.js";

const PLACE = { projectId: "proj-1", deviceId: "dev-1" };

const notice = (over = {}) => ({
  from_build: true,
  role: "user",
  from_issue: { issue_id: "issue-32", number: 32, title: "Kanban drag does not persist" },
  body: "#32 Kanban drag does not persist — agent-01M2XXGQ commented: this reproduces on a phone too.",
  ...over,
});

const html = (message, options = { place: PLACE }) => {
  const host = document.createElement("div");
  host.innerHTML = issueNoticeLineHtml(issueNoticeOf(message), options);
  return host.firstElementChild;
};
const text = (message, options) => html(message, options).textContent.replace(/\s+/g, " ").trim();

describe("what counts as a notice", () => {
  it("is a message carrying both marks", () => {
    expect(isIssueNotice(notice())).toBe(true);
  });

  // `from_build` alone is the restart notice, which is an instruction to the
  // agent and reads as one.
  it("is not Build's own restart notice", () => {
    expect(isIssueNotice({ from_build: true, body: "Carry on." })).toBe(false);
  });

  // A hand-off carries `from_issue` and is a real message from somebody.
  it("is not an issue handed over in a message", () => {
    expect(isIssueNotice({ from_issue: { issue_id: "issue-1" }, body: "Take this" })).toBe(false);
    expect(isIssueNotice(null)).toBe(false);
  });
});

describe("the line, from the structured field", () => {
  const stated = (over = {}) =>
    notice({ issue_notice: { actor: "agent-01M2XXGQ", action: "commented", comment_id: "ic-9", ...over } });

  it("reads actor, action, then the issue", () => {
    expect(text(stated())).toBe("Agent 01M2 commented on #32 Kanban drag does not persist");
  });

  it("uses the bridge's own phrase for an action it has never heard of", () => {
    expect(text(stated({ action: "moved to In review" }))).toContain("moved to In review #32");
    expect(text(stated({ action: "assigned to Agent 2" }))).toContain("assigned to Agent 2 #32");
  });

  // A bare token from a sender that sends one, turned into the words a reader
  // says. `comment` carries its own preposition because the sentence has none.
  it("puts a bare token into the words a reader says", () => {
    for (const [token, said] of [["comment", "commented on"], ["close", "closed"], ["reopen", "reopened"], ["edit", "edited"], ["link", "linked"], ["update", "edited"]]) {
      expect(text(stated({ action: token }))).toBe(`Agent 01M2 ${said} #32 Kanban drag does not persist`);
    }
  });

  it("says You for the user", () => {
    expect(text(stated({ actor: { kind: "user" } }))).toContain("You commented on");
  });

  it("uses the name this client has for an agent when it has one", () => {
    const line = issueNoticeLineHtml(issueNoticeOf(stated()), {
      place: PLACE,
      agentLabels: { "agent-01M2XXGQ": "issues-spa · Agent 1" },
    });
    expect(line).toContain("issues-spa · Agent 1 commented on");
  });
});

describe("the line, parsed from the body", () => {
  it("reads the actor and the action out of the first line", () => {
    expect(text(notice())).toBe("Agent 01M2 commented on #32 Kanban drag does not persist");
  });

  // The comment body is carried but never drawn: the press is what opens it.
  it("never shows what was said", () => {
    expect(text(notice())).not.toContain("this reproduces on a phone too");
  });

  it("reads only the first line, however many the body has", () => {
    const long = notice({ body: `${notice().body}\n\nAnd a second paragraph.` });
    expect(text(long)).not.toContain("second paragraph");
  });

  // A title may contain an em dash; the actor never does, so the split is at
  // the last one.
  it("survives a title with an em dash in it", () => {
    const dashed = notice({
      from_issue: { issue_id: "issue-9", number: 9, title: "Board — on a phone" },
      body: "#9 Board — on a phone — agent-01M2XXGQ closed: done.",
    });
    expect(text(dashed)).toBe("Agent 01M2 closed #9 Board — on a phone");
  });

  // The whole point of the fallback degrading rather than failing: the issue
  // comes from the envelope, so the LINK never depends on the parse.
  it("says the issue alone when the prose says nothing it can read", () => {
    const opaque = notice({ body: "something else entirely" });
    expect(text(opaque)).toBe("#32 Kanban drag does not persist");
    expect(html(opaque).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/issues/issue-32");
  });
});

describe("where the line goes", () => {
  it("opens the issue's page on the machine the project is on", () => {
    expect(html(notice()).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/issues/issue-32");
  });

  it("lands on the comment itself when the field names one", () => {
    const stated = notice({ issue_notice: { actor: "agent-01M2XXGQ", action: "commented", comment_id: "ic-9" } });
    expect(html(stated).getAttribute("href")).toBe("#/device/dev-1/project/proj-1/issues/issue-32#comment-ic-9");
  });

  // Nothing in a sentence is a comment id, so a parsed notice lands on the
  // issue — the right page, one scroll from the right place.
  it("lands on the issue when only the prose was available", () => {
    expect(html(notice()).getAttribute("href")).not.toContain("#comment-");
  });

  // Pointing nowhere is worse than not being a link.
  it("draws as plain text where there is no project to stand in", () => {
    const loose = html(notice(), { place: null });
    expect(loose.tagName).toBe("SPAN");
    expect(noticeHref(issueNoticeOf(notice()), null)).toBe("");
  });

  it("names the issue on the row, so a press can be found by id", () => {
    expect(html(notice()).dataset.issueNotice).toBe("issue-32");
  });
});

// ---- the row it draws in the conversation ---------------------------------

describe("the row, in the timeline", () => {
  const item = (over = {}) => ({ type: "message", data: { id: "message-9", sequence: 9, ...notice(over) } });

  const paint = (items) => {
    document.body.innerHTML = threadHtml({ id: "conversation-3", items }, { place: PLACE });
    return document.querySelector(".thread-message");
  };

  // The whole of Zech's report. A notice is a message on the user's side, so
  // drawn as one it wore his colour, sat on his side, and claimed he wrote it.
  it("is not a user bubble", () => {
    const row = paint([item()]);
    expect(row.classList.contains("thread-notice")).toBe(true);
    expect(row.classList.contains("user")).toBe(false);
    expect(row.classList.contains("thread-comment")).toBe(false);
    expect(row.querySelector(".thread-avatar")).toBeNull();
  });

  // Nor Build's restart notice, which IS a bubble and has to stay one.
  it("leaves Build's own notice alone", () => {
    const restart = { type: "message", data: { id: "m-1", sequence: 1, role: "user", from_build: true, body: "Carry on." } };
    const row = paint([restart]);
    expect(row.classList.contains("from-build")).toBe(true);
    expect(row.classList.contains("thread-notice")).toBe(false);
  });

  it("is one line, with the comment body nowhere on it", () => {
    const row = paint([item()]);
    expect(row.querySelectorAll("a, span").length).toBeGreaterThan(0);
    expect(row.querySelector(".thread-body")).toBeNull();
    expect(row.textContent).not.toContain("this reproduces on a phone too");
  });

  // Zech asked for "X did Y", and X has to be a name he recognises. The names
  // come off the feed, so the timeline is handed them.
  it("names the agent the way the rest of the project names it", () => {
    document.body.innerHTML = threadHtml(
      { id: "conversation-3", items: [item({ issue_notice: { actor: "agent-01M2XXGQ", action: "commented" } })] },
      { place: PLACE, agentLabels: { "agent-01M2XXGQ": "issues-spa · Agent 1" } },
    );
    expect(document.querySelector(".thread-notice").textContent.replace(/\s+/g, " ").trim())
      .toBe("issues-spa · Agent 1 commented on #32 Kanban drag does not persist");
  });

  // Not a blank where a name should be: an agent this client cannot name is
  // still said, by the four characters it wears everywhere else.
  it("falls back to the agent's short name when the feed has none for it", () => {
    expect(paint([item({ issue_notice: { actor: "agent-01M2XXGQ", action: "commented" } })])
      .textContent.replace(/\s+/g, " ").trim())
      .toBe("Agent 01M2 commented on #32 Kanban drag does not persist");
  });

  it("carries its sequence, so it reads in order and counts as unread", () => {
    expect(paint([item()]).dataset.sequence).toBe("9");
  });

  it("does not fold", () => {
    expect(paint([item()]).querySelector("details")).toBeNull();
    expect(paint([item()]).querySelector("[data-arrival-press]")).toBeNull();
  });

  // A line nobody can see is a line nobody can press.
  it("survives every detail level", () => {
    for (const level of ["all", "messages", "agent"]) {
      expect(itemsAtDetailLevel([item()], level)).toHaveLength(1);
    }
  });

  // Explicitly, rather than falling out of carrying no `from_agent`: whether a
  // notice survives the narrowest level should not depend on which fields the
  // bridge happens to set on it.
  it("survives the narrowest level even when it names another conversation", () => {
    const relayed = { type: "message", data: { id: "m-2", sequence: 2, ...notice({ from_agent: { id: "agent-elsewhere" } }) } };
    expect(itemsAtDetailLevel([relayed], "agent")).toHaveLength(1);
  });
});

// #40. Zech, on the rolled build: "There's a lot of space on the left of the
// issue notifications, there's a lot of space between them, and they're not
// one line." All three are structural, so all three are asserted structurally
// here — and then measured for real in a browser, which is the only place the
// first and third can actually be seen.
describe("the shape of the row", () => {
  const notice_ = (over = {}) => ({ type: "message", data: { id: "m-n", sequence: 9, ...notice(over) } });
  const action_ = (over = {}) => ({
    type: "message",
    data: {
      id: "m-a", sequence: 10, role: "agent", body: "",
      issue_action: { action: "created", issue_id: "issue-39", number: 39, title: "A title", ...over },
    },
  });
  const said = (item) => {
    document.body.innerHTML = threadHtml({ id: "c-3", items: [item] }, { place: PLACE });
    return document.querySelector(".thread-message");
  };

  // One KIND of row, so the rules that matter are written about the kind.
  it("marks both rows as the same kind of line", () => {
    expect(said(notice_()).classList.contains("thread-issue-line")).toBe(true);
    expect(said(action_()).classList.contains("thread-issue-line")).toBe(true);
  });

  // The title is the part that can be any length, so it is the part that
  // gives; nothing else on the row is allowed to wrap.
  it("gives the title the ellipsis and nothing a wrap", () => {
    for (const row of [said(notice_()), said(action_())]) {
      const line = row.querySelector("a, span");
      expect(line.querySelector(".thread-issue-line-title")).not.toBeNull();
      expect(line.querySelector(".thread-issue-number")).not.toBeNull();
      // The old markup put the title in a span that wrapped anywhere.
      expect(row.querySelector(".thread-issue-action-title")).toBeNull();
      expect(row.querySelector(".thread-issue-notice-title")).toBeNull();
    }
  });

  it("carries no indent of its own, so it starts where message text starts", () => {
    const rules = readFileSync(resolve(process.cwd(), "src/styles/issues.css"), "utf8");
    expect(rules).toContain(".thread-message.thread-issue-line { padding:0; }");
    // The 34 px indent the rows used to carry on top of the avatar gutter.
    expect(rules).not.toContain("padding:1px 0 1px 34px");
  });

  // Consecutive lines read as a list; a real message either side keeps the
  // full gap, because it IS a separate thing to say.
  it("pulls consecutive lines together and leaves a message alone", () => {
    const rules = readFileSync(resolve(process.cwd(), "src/styles/issues.css"), "utf8");
    expect(rules).toContain(".thread-issue-line + .thread-issue-line { margin-top:calc(4px - var(--thread-gap)); }");
    const shell = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");
    expect(shell).toContain("--thread-gap:18px");
    expect(shell).toContain("--thread-gap:20px");
  });

  // The actor Zech saw as "Agent 01M2": Build's own agent for the project.
  it("names the project's agent after its project", () => {
    document.body.innerHTML = threadHtml(
      { id: "c-3", items: [notice_({ issue_notice: { actor: "project-01M2SCB", action: "commented" } })] },
      { place: { ...PLACE, projectName: "Build" } },
    );
    expect(document.querySelector(".thread-notice").textContent.replace(/\s+/g, " ").trim())
      .toBe("Build agent commented on #32 Kanban drag does not persist");
  });
});
