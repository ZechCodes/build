// A reference becomes a link, or it stays the words the agent typed.
//
// #56. Every href here comes from core/router.js `hashFromRoute` — nothing
// builds a hash by hand — and a target nothing resolves is never a broken
// link, which is the one rule this layer exists to keep.

import { describe, it, expect } from "vitest";

import { expandReferences } from "../src/core/markdownLinks.js";
import { hashFromRoute } from "../src/core/router.js";

const WHERE = { deviceId: "dev-1", projectId: "p-1" };

/** A resolver that knows one of everything. */
const links = {
  task: (number) => (number === 42 ? { ...WHERE, taskId: "task-42", title: "Rebuild the shell" } : null),
  workspace: (name) => (name === "tasks-board" ? { ...WHERE, workspaceId: "ws-1", name: "tasks-board" } : null),
  agent: (id) => (id === "agent-7" ? { ...WHERE, workspaceId: "ws-1", agentId: "agent-7", name: "Ada" } : null),
};

// Decoded the way a browser decodes an attribute: `&` is written `&amp;` in
// the markup, and the href the reader follows is the route itself.
const href = (html) => html.match(/href="([^"]*)"/)?.[1]?.replace(/&amp;/g, "&");
const title = (html) => html.match(/title="([^"]*)"/)?.[1];

describe("a reference that resolves", () => {
  it("links a task at the route the router writes", () => {
    const html = expandReferences("see #42 now", links);
    expect(href(html)).toBe(hashFromRoute({ ...WHERE, name: "trackerTask", taskId: "task-42" }));
    expect(title(html)).toContain("Rebuild the shell");
    // The words the agent typed are what the reader presses.
    expect(html).toContain(">#42</a>");
    expect(html.startsWith("see ")).toBe(true);
  });

  it("links a comment on a task", () => {
    expect(href(expandReferences("#42/c/tc-7", links))).toBe(hashFromRoute({
      ...WHERE, name: "trackerTask", taskId: "task-42", commentId: "tc-7",
    }));
  });

  it("links a workspace, and an agent's conversation", () => {
    expect(href(expandReferences("@workspace:tasks-board", links)))
      .toBe(hashFromRoute({ ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes" }));
    expect(href(expandReferences("@agent:agent-7", links)))
      .toBe(hashFromRoute({ ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes", agent: "agent-7" }));
  });

  it("links a file at its line", () => {
    const html = expandReferences("[[tasks-board:src/a.js#L10]]", links);
    expect(href(html)).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "files", file: "src/a.js", line: 10,
    }));
  });

  it("links a short or full commit SHA into the workspace Changes view", () => {
    for (const sha of ["b8ce4ee9", `b8ce4ee9${"1".repeat(32)}`]) {
      expect(href(expandReferences(`[[tasks-board:commit:${sha}]]`, links))).toBe(hashFromRoute({
        ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes", commit: sha,
      }));
    }
  });

  it("escapes what it puts in an attribute", () => {
    const quoted = { ...links, task: () => ({ ...WHERE, taskId: "i-1", title: 'a "quoted" <b>' }) };
    const html = expandReferences("#1", quoted);
    expect(html).not.toContain('title="a "');
    expect(title(html)).toContain("&quot;quoted&quot;");
  });
});

describe("a reference that does not resolve", () => {
  it("is the words the agent typed, never a broken link", () => {
    for (const text of ["#999", "@workspace:gone", "@agent:nobody", "[[gone:src/a.js]]"]) {
      expect([text, expandReferences(text, links)]).toEqual([text, text]);
    }
  });

  it("leaves everything alone when there is no resolver at all", () => {
    const text = "see #42 and @workspace:tasks-board and [[tasks-board:a.js]]";
    expect(expandReferences(text)).toBe(text);
    expect(expandReferences(text, null)).toBe(text);
  });

  it("leaves unresolved comments and commits as text", () => {
    for (const text of ["#999/c/tc-1", "[[gone:commit:b8ce4ee9]]", "[[tasks-board:commit:not-a-sha]]"]) {
      expect(expandReferences(text, links)).toBe(text);
    }
  });
});

describe("what it will not reach into", () => {
  it("leaves task-shaped URL fragments and link targets literal", () => {
    const html = expandReferences("https://example.test/#42 https://example.test/?q=1&amp;next=2#42/c/tc-7 #42/c/tc-7", links);
    expect(html).toContain("https://example.test/#42");
    expect(html).toContain("https://example.test/?q=1&amp;next=2#42/c/tc-7");
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(href(html)).toBe(hashFromRoute({ ...WHERE, name: "trackerTask", taskId: "task-42", commentId: "tc-7" }));

    const target = expandReferences("[outside](./#42/c/tc-7) and #42", links);
    expect(target).toContain("[outside](./#42/c/tc-7)");
    expect(target.match(/<a /g)).toHaveLength(1);
  });

  it("leaves a reference inside a code span alone", () => {
    const html = expandReferences("write <code>#42</code> to link it", links);
    expect(html).toBe("write <code>#42</code> to link it");
  });

  it("links outside the span while leaving the span itself", () => {
    const html = expandReferences("<code>#42</code> means #42", links);
    expect(html.match(/<a /g)).toHaveLength(1);
    expect(html).toContain("<code>#42</code>");
  });
});
