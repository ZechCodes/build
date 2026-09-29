// A reference becomes a link, or it stays the words the agent typed.
//
// #56. Every href here comes from core/router.js `hashFromRoute` — nothing
// builds a hash by hand — and a target nothing resolves is never a broken
// link, which is the one rule this layer exists to keep.

import { describe, it, expect } from "vitest";

import { expandReferences, plainReferences } from "../src/core/markdownLinks.js";
import { hashFromRoute } from "../src/core/router.js";

const WHERE = { deviceId: "dev-1", projectId: "p-1" };

/** A resolver that knows one of everything. `null` is "looked, and it is not
 *  there"; `undefined` is "nothing here can say" (core/referenceTargets.js). */
const tasksBoard = {
  ...WHERE, workspaceId: "ws-1", name: "tasks-board",
  directories: [{ sourceId: "source-1", name: "Build" }],
};
const multi = {
  ...WHERE, workspaceId: "ws-2", name: "fixes",
  directories: [{ sourceId: "source-1", name: "skrift" }, { sourceId: "source-2", name: "skrift-core" }],
};
const links = {
  task: (number) => (number === 42 ? { ...WHERE, taskId: "task-42", title: "Rebuild the shell" } : number === 7 ? undefined : null),
  workspace: (name) => ({ "tasks-board": tasksBoard, fixes: multi, "fixes/skrift-core": { ...multi, sourceId: "source-2" } })[name] ?? null,
  agent: (id) => (id === "agent-7" ? { ...WHERE, workspaceId: "ws-1", agentId: "agent-7", name: "Ada" } : null),
  project: (name) => (name === "Skrift" ? { deviceId: "dev-1", projectId: "p-2", name: "Skrift" } : null),
};

// Decoded the way a browser decodes an attribute: `&` is written `&amp;` in
// the markup, and the href the reader follows is the route itself.
const href = (html) => html.match(/href="([^"]*)"/)?.[1]?.replace(/&amp;/g, "&");
const title = (html) => html.match(/title="([^"]*)"/)?.[1];
/** What the reader sees: the markup's text, tags and entities taken off. */
const text = (html) => html.replace(/<[^>]*>/g, "").replace(/&amp;/g, "&").replace(/&quot;/g, '"');

describe("a reference that resolves", () => {
  it("links a task at the route the router writes", () => {
    const html = expandReferences("see #42 now", links);
    expect(href(html)).toBe(hashFromRoute({ ...WHERE, name: "trackerTask", taskId: "task-42" }));
    // #229: the reader presses what the task is called; the words the agent
    // typed stay on the hover.
    expect(text(html)).toBe("see #42 Rebuild the shell now");
    expect(title(html)).toContain("#42");
    expect(html).toContain('class="md-ref"');
    expect(html.startsWith("see ")).toBe(true);
  });

  it("links a comment on a task", () => {
    const html = expandReferences("#42/c/tc-7", links);
    expect(href(html)).toBe(hashFromRoute({
      ...WHERE, name: "trackerTask", taskId: "task-42", commentId: "tc-7",
    }));
    expect(text(html)).toBe("#42 Rebuild the shell · comment");
    expect(title(html)).toContain("#42/c/tc-7");
  });

  it("links a workspace, and an agent's conversation", () => {
    expect(href(expandReferences("@workspace:tasks-board", links)))
      .toBe(hashFromRoute({ ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes" }));
    expect(href(expandReferences("@agent:agent-7", links)))
      .toBe(hashFromRoute({ ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes", agent: "agent-7" }));
    expect(text(expandReferences("@workspace:tasks-board", links))).toBe("tasks-board");
    expect(text(expandReferences("@agent:agent-7", links))).toBe("Ada");
    expect(title(expandReferences("@agent:agent-7", links))).toContain("@agent:agent-7");
  });

  it("links a project to its own page", () => {
    const html = expandReferences("@project:Skrift", links);
    expect(href(html)).toBe(hashFromRoute({ name: "project", deviceId: "dev-1", projectId: "p-2" }));
    expect(text(html)).toBe("Skrift");
    expect(title(html)).toContain("@project:Skrift");
  });

  it("links a file at its line", () => {
    const html = expandReferences("[[tasks-board:src/a.js#L10]]", links);
    expect(href(html)).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "files", file: "src/a.js", line: 10,
    }));
    expect(text(html)).toBe("src/a.js:10");
    expect(title(html)).toContain("[[tasks-board:src/a.js#L10]]");
  });

  // #229: a workspace with several source directories.
  it("links a file in the source directory the reference names", () => {
    expect(href(expandReferences("[[fixes/skrift-core:src/app.py]]", links))).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-2", sourceId: "source-2", tab: "files", file: "src/app.py",
    }));
  });

  // What an agent standing in a multi-source root copies is the path on disk,
  // which starts with the directory's name.
  it("reads a leading directory name as the directory in a multi-source workspace", () => {
    expect(href(expandReferences("[[fixes:skrift-core/src/app.py#L3]]", links))).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-2", sourceId: "source-2", tab: "files", file: "src/app.py", line: 3,
    }));
  });

  // With one directory, the path is that directory's own, and a folder that
  // happens to share its name is a folder.
  it("keeps a single-directory workspace's path as written", () => {
    expect(href(expandReferences("[[tasks-board:Build/a.js]]", links))).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "files", file: "Build/a.js",
    }));
  });

  it("links a commit in the source directory the reference names", () => {
    expect(href(expandReferences("[[fixes/skrift-core:commit:b8ce4ee9]]", links))).toBe(hashFromRoute({
      ...WHERE, name: "workspace", workspaceId: "ws-2", sourceId: "source-2", tab: "changes", commit: "b8ce4ee9",
    }));
  });

  it("links a short or full commit SHA into the workspace Changes view", () => {
    for (const sha of ["b8ce4ee9", `b8ce4ee9${"1".repeat(32)}`]) {
      expect(href(expandReferences(`[[tasks-board:commit:${sha}]]`, links))).toBe(hashFromRoute({
        ...WHERE, name: "workspace", workspaceId: "ws-1", tab: "changes", commit: sha,
      }));
    }
  });

  it("names a commit by its workspace and its short SHA", () => {
    const html = expandReferences(`[[tasks-board:commit:b8ce4ee9${"1".repeat(32)}]]`, links);
    expect(text(html)).toBe("tasks-board · b8ce4ee9");
    expect(title(html)).toContain(`[[tasks-board:commit:b8ce4ee9${"1".repeat(32)}]]`);
  });

  it("escapes a name it puts in the label", () => {
    const named = { ...links, agent: () => ({ ...WHERE, workspaceId: "ws-1", agentId: "a", name: "<b>Ada</b>" }) };
    expect(expandReferences("@agent:a", named)).toContain("&lt;b&gt;Ada&lt;/b&gt;");
  });

  it("escapes what it puts in an attribute", () => {
    const quoted = { ...links, task: () => ({ ...WHERE, taskId: "i-1", title: 'a "quoted" <b>' }) };
    const html = expandReferences("#1", quoted);
    expect(html).not.toContain('title="a "');
    expect(title(html)).toContain("&quot;quoted&quot;");
  });
});

// #229: a reference the index looked for and did not find reads as the words
// the agent typed — never a link to nowhere — but marked, with the reason on
// the hover, so a broken reference is not mistaken for prose.
describe("a reference that does not resolve", () => {
  const missing = (html) => html.match(/<span class="md-ref-missing" title="([^"]*)">/)?.[1];

  it("is the words the agent typed, marked missing, never a broken link", () => {
    for (const written of ["#999", "@workspace:gone", "@agent:nobody", "@project:Nowhere", "[[gone:src/a.js]]"]) {
      const html = expandReferences(written, links);
      expect([written, text(html)]).toEqual([written, written]);
      expect(html).not.toContain("<a ");
      expect(missing(html)).toBeTruthy();
    }
  });

  it("says why on the hover", () => {
    expect(missing(expandReferences("#999", links))).toContain("#999");
    expect(missing(expandReferences("@workspace:gone", links))).toContain("gone");
    expect(missing(expandReferences("[[fixes:commit:b8ce4ee9]]", { ...links, workspace: () => null })))
      .toContain("fixes");
  });

  it("is left exactly as typed when nothing here can tell", () => {
    expect(expandReferences("#7", links)).toBe("#7");
  });

  it("leaves everything alone when there is no resolver at all", () => {
    const written = "see #42 and @workspace:tasks-board and [[tasks-board:a.js]]";
    expect(expandReferences(written)).toBe(written);
    expect(expandReferences(written, null)).toBe(written);
  });

  it("marks unresolved comments and commits, and leaves a malformed one as prose", () => {
    for (const written of ["#999/c/tc-1", "[[gone:commit:b8ce4ee9]]"]) {
      expect(text(expandReferences(written, links))).toBe(written);
    }
    expect(expandReferences("[[tasks-board:commit:not-a-sha]]", links)).toBe("[[tasks-board:commit:not-a-sha]]");
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

// #229: a one-line preview is plain text, so a reference there reads as the
// words its link would carry — never as `[[…]]` machinery.
describe("a reference in plain text", () => {
  it("reads as its label when it resolves, and as typed when it does not", () => {
    expect(plainReferences("see #42 at [[tasks-board:commit:b8ce4ee9]] and @workspace:gone", links))
      .toBe("see #42 Rebuild the shell at tasks-board · b8ce4ee9 and @workspace:gone");
  });

  it("leaves a code span's examples literal", () => {
    expect(plainReferences("write `#42` for #42", links)).toBe("write `#42` for #42 Rebuild the shell");
  });

  it("leaves the text alone without a resolver", () => {
    expect(plainReferences("#42", null)).toBe("#42");
  });
});
