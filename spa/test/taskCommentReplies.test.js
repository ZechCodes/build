// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { timelineHtml } from "../src/core/trackerTaskRender.js";
import { createTaskReplyNavigation } from "../src/core/taskCommentReplies.js";
import { patchElement } from "../src/core/domPatch.js";

const parent = { type: "comment", id: "tc-parent", author: { kind: "agent", agent_id: "agent-one" },
  body: "# **Check** [the patch](https://example.com) with `care`.\n" + "More detail. ".repeat(20),
  created_at: "2026-10-07T10:00:00Z" };
const reply = { type: "comment", id: "tc-reply", author: { kind: "user" },
  body: "I checked it.", reply_to: parent.id, created_at: "2026-10-07T10:10:00Z" };
const context = { deviceId: "device", projectId: "project", identities: {
  "agent-one": { name: 'Ada <&"', provider: "codex", available: true, workspace_id: "ws" },
} };
const render = (timeline, reading = context) => {
  const host = document.createElement("div");
  host.innerHTML = timelineHtml(timelineRows(timeline), reading);
  return host;
};

describe("task comment reply quotes", () => {
  it("quotes the parent's mark, name, relative time and bounded plain excerpt in one named control", () => {
    const host = render([parent, reply]);
    const quote = host.querySelector('[data-comment-id="tc-reply"] .tracker-task-comment-reply');
    expect(quote).not.toBeNull();
    expect(quote.tagName).toBe("BUTTON");
    expect(quote.getAttribute("aria-label")).toBe('Go to the comment by Ada <&"');
    expect(quote.dataset.commentJump).toBe(parent.id);
    expect(quote.querySelector(".task-avatar").outerHTML).toBe(host.querySelector('[data-comment-id="tc-parent"] .task-avatar').outerHTML);
    expect(quote.querySelector(".task-when").outerHTML).toBe(host.querySelector('[data-comment-id="tc-parent"] .task-when').outerHTML);
    const excerpt = quote.querySelector(".task-reply-excerpt").textContent;
    expect(excerpt).toMatch(/^Check the patch with care\. More detail\./);
    expect(excerpt.length).toBeLessThanOrEqual(120);
    expect(excerpt).toMatch(/…$/);
    expect(excerpt).not.toMatch(/[\n*`#]/);
    expect(quote.querySelectorAll("a, button, input")).toHaveLength(0);
    expect(quote.textContent).not.toContain(parent.id);
    expect(quote.querySelector("script")).toBeNull();
  });

  it("shows an unavailable parent as text without exposing its id or offering a jump", () => {
    const quote = render([reply]).querySelector(".tracker-task-comment-reply");
    expect(quote).not.toBeNull();
    expect(quote.textContent).toBe("Reply to a comment");
    expect(quote.matches("button, a, [role=button]")).toBe(false);
    expect(quote.hasAttribute("data-comment-jump")).toBe(false);
  });

  it("counts direct replies and points to the first one in timeline order", () => {
    const nested = { ...reply, id: "tc-nested", reply_to: reply.id };
    const second = { ...reply, id: "tc-second" };
    const host = render([parent, reply, nested, second]);
    const count = host.querySelector('[data-comment-id="tc-parent"] .task-comment-replies');
    expect(count).not.toBeNull();
    expect(count.textContent).toBe("2 replies");
    expect(count.dataset.commentJump).toBe(reply.id);
    expect(host.querySelector('[data-comment-id="tc-reply"] .task-comment-replies').textContent).toBe("1 reply");
  });

  it("refreshes a quote from the current cached rows without adding a context line", () => {
    const next = render([{ ...parent, body: "Updated plain body", author_context: { tokens: 200000 } }, reply], {
      ...context, identities: { "agent-one": { ...context.identities["agent-one"], name: "Updated author" } },
    }).querySelector('[data-comment-id="tc-reply"] .tracker-task-comment-reply');
    expect(next).not.toBeNull();
    expect(next.textContent).toContain("Updated author");
    expect(next.querySelector(".task-reply-excerpt").textContent).toBe("Updated plain body");
    expect(next.textContent).not.toContain("200000");
  });
});

describe("deliberate reply navigation", () => {
  it("scrolls once on activation, keeps the arrival mark through a paint, then removes it", () => {
    vi.useFakeTimers();
    const host = render([parent, reply]);
    host.scrollTo = vi.fn();
    const navigation = createTaskReplyNavigation(host);
    try {
      navigation.wire();
      expect(host.scrollTo).not.toHaveBeenCalled();
      host.querySelector('[data-comment-id="tc-reply"] button[data-comment-jump]').click();
      expect(host.scrollTo).toHaveBeenCalledTimes(1);
      const row = host.querySelector('[data-comment-id="tc-parent"]');
      expect(row.classList.contains("task-comment-target")).toBe(true);
      const next = render([{ ...parent, body: "Updated while highlighted" }, reply]);
      patchElement(row, next.querySelector('[data-comment-id="tc-parent"]'));
      navigation.wire();
      expect(host.scrollTo).toHaveBeenCalledTimes(1);
      expect(row.classList.contains("task-comment-target")).toBe(true);
      vi.advanceTimersByTime(2000);
      expect(row.classList.contains("task-comment-target")).toBe(false);
    } finally {
      navigation.dispose();
      vi.useRealTimers();
    }
  });

  it("jumps from the reply count to the first reply and unwires on disposal", () => {
    const host = render([parent, reply]);
    host.scrollTo = vi.fn();
    const navigation = createTaskReplyNavigation(host);
    navigation.wire();
    const count = host.querySelector("button.task-comment-replies");
    count.click();
    expect(host.querySelector('[data-comment-id="tc-reply"]').classList.contains("task-comment-target")).toBe(true);
    expect(host.scrollTo).toHaveBeenCalledTimes(1);
    navigation.dispose();
    count.click();
    expect(host.scrollTo).toHaveBeenCalledTimes(1);
    expect(host.querySelector(".task-comment-target")).toBeNull();
  });
});
