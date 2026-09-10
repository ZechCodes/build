// @vitest-environment jsdom
// The one box under the diff.
//
// A reviewer reading a change has two things to say into it: a note to the
// agent, and a commit message. Both are a few sentences about the same diff, so
// both are the same field — and which one it becomes is the button, not a mode
// the reviewer has to set first. Commenting is the primary: it is what a review
// is for, and it is the one that is always safe.
//
// It sits BELOW the stack rather than at the end of it, so scrolling the diff
// never takes it off screen. It opens as one line, because most of what goes in
// it is one line.

import { describe, expect, it, beforeEach } from "vitest";
import {
  changesComposerHtml,
  changesComposerOptions,
  changesComposerPlaceholder,
  mountChangesComposer,
} from "../src/core/changesComposer.js";

describe("what the box under the diff can do", () => {
  it("offers commenting first, and committing behind it", () => {
    const options = changesComposerOptions({ commentable: true, uncommitted: true });
    expect(options.map((option) => option.id)).toEqual(["comment", "commit"]);
  });

  it("offers only committing where there is no agent to talk to", () => {
    const options = changesComposerOptions({ commentable: false, uncommitted: true });
    expect(options.map((option) => option.id)).toEqual(["commit"]);
  });

  it("offers only commenting where there is nothing to commit", () => {
    const options = changesComposerOptions({ commentable: true, uncommitted: false });
    expect(options.map((option) => option.id)).toEqual(["comment"]);
  });

  it("offers nothing at all over a changeset that takes neither", () => {
    expect(changesComposerOptions({ commentable: false, uncommitted: false })).toEqual([]);
  });

  it("says how many comments the send carries", () => {
    const [comment] = changesComposerOptions({ commentable: true, pendingComments: 3 });
    expect(comment.label).toBe("Send 3 comments");
    expect(changesComposerOptions({ commentable: true, pendingComments: 1 })[0].label).toBe("Send 1 comment");
    expect(changesComposerOptions({ commentable: true })[0].label).toBe("Comment");
  });

  it("puts the caller's agent-commit options behind the plain one", () => {
    const options = changesComposerOptions({
      commentable: true,
      uncommitted: true,
      commitExtras: [{ id: "agent_commit" }, { id: "auto_commit" }],
    });
    expect(options.map((option) => option.id)).toEqual(["comment", "commit", "agent_commit", "auto_commit"]);
  });

  it("offers no agent-commit options where there is nothing to commit", () => {
    const options = changesComposerOptions({ commentable: true, commitExtras: [{ id: "agent_commit" }] });
    expect(options.map((option) => option.id)).toEqual(["comment"]);
  });

  it("names in the placeholder whatever the box can do", () => {
    expect(changesComposerPlaceholder({ commentable: true, uncommitted: true })).toContain("commit message");
    expect(changesComposerPlaceholder({ commentable: true, uncommitted: true })).toContain("Comment");
    expect(changesComposerPlaceholder({ commentable: true, uncommitted: false })).not.toContain("commit");
    expect(changesComposerPlaceholder({ commentable: false, uncommitted: true })).toBe("Commit message…");
  });
});

describe("the box itself", () => {
  let host;

  const offers = (over = {}) => ({ commentable: true, uncommitted: true, pendingComments: 0, ...over });

  const mount = (options = {}) => {
    let draft = "";
    const ran = [];
    const composer = mountChangesComposer(host, {
      offers: () => offers(options.offers),
      run: async (id, text) => ran.push([id, text]),
      readDraft: () => draft,
      writeDraft: (value) => {
        draft = value;
      },
      ...options.over,
    });
    return { composer, ran, readDraft: () => draft };
  };

  beforeEach(() => {
    document.body.innerHTML = "";
    host = document.createElement("div");
    document.body.appendChild(host);
  });

  it("opens as a single row", () => {
    host.innerHTML = changesComposerHtml("say something…");
    expect(host.querySelector("textarea").rows).toBe(1);
  });

  it("draws the field and its verbs", () => {
    mount();
    expect(host.querySelector(".csinput")).toBeTruthy();
    expect(host.querySelector(".csbox-actions .btn")).toBeTruthy();
  });

  it("hands what was typed to the verb the reviewer picked", async () => {
    const { ran } = mount();
    host.querySelector(".csinput").value = "rename the helper";
    host.querySelector(".csbox-actions .btn:not(.caret)").click();
    await Promise.resolve();
    await Promise.resolve();
    expect(ran).toEqual([["comment", "rename the helper"]]);
  });

  it("keeps every keystroke in the draft, so a repaint cannot take it", () => {
    const { readDraft } = mount();
    const input = host.querySelector(".csinput");
    input.value = "half a thought";
    input.dispatchEvent(new window.Event("input"));
    expect(readDraft()).toBe("half a thought");
  });

  it("restores the draft it was mounted over", () => {
    mountChangesComposer(host, {
      offers: () => offers(),
      run: async () => {},
      readDraft: () => "written earlier",
      writeDraft: () => {},
    });
    expect(host.querySelector(".csinput").value).toBe("written earlier");
  });

  it("empties the field once the verb it ran has taken the text", async () => {
    const { composer } = mount();
    const input = host.querySelector(".csinput");
    input.value = "sent";
    host.querySelector(".csbox-actions .btn:not(.caret)").click();
    await Promise.resolve();
    await Promise.resolve();
    composer.refresh();
    expect(host.querySelector(".csinput").value).toBe("");
  });

  it("draws no box at all over a changeset that takes neither verb", () => {
    mount({ offers: { commentable: false, uncommitted: false } });
    expect(host.querySelector(".csinput")).toBe(null);
  });

  it("leaves the field standing, and what is in it, when the verbs change under it", () => {
    let uncommitted = true;
    mountChangesComposer(host, {
      offers: () => offers({ uncommitted }),
      run: async () => {},
      readDraft: () => "",
      writeDraft: () => {},
    });
    const input = host.querySelector(".csinput");
    input.value = "mid-sentence";
    uncommitted = false;
    host.querySelector(".csinput").dispatchEvent(new window.Event("input"));
    expect(host.querySelector(".csinput")).toBe(input);
    expect(host.querySelector(".csinput").value).toBe("mid-sentence");
  });
});
