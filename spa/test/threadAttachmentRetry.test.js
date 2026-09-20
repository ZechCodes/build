// @vitest-environment jsdom
// #30 point 3: a picture that failed because the wire was not there is not
// "unavailable" — it is still loading, and it is asked for again after the
// reconnect.
//
// Zech's 789 KB attachment showed "unavailable" while the post beside it said
// "Delivery uncertain", and only a hard refresh brought it back. The cache
// remembered every failure as final, which is right for a bridge that refuses a
// path (asking again on every 1.5-second repaint would be pure waste) and wrong
// for a path that has gone: nothing was refused, nothing was asked.

import { describe, expect, it, beforeEach } from "vitest";
import { createThreadState, threadHtml, wireThreadAttachments } from "../src/core/thread.js";
import { createChatRepository } from "../src/core/chatRepository.js";

const withPicture = (path = "shots/big.png") => ({
  items: [{
    type: "message",
    data: {
      role: "user",
      body: "look",
      created_at: "2026-09-20T18:57:29Z",
      attachments: [{ path, name: "big.png", mime: "image/png", size: 789212 }],
    },
  }],
});

/** The refusal a call fails with when this session's carrier has gone
 *  (core/session.js). `isTransientTransportError` is the one rule that reads it,
 *  and test/transientRead.test.js holds the sentence to the module. */
const wireWent = () => new Error("your device went offline");
const pathDeadline = () => Object.assign(new Error("thread.attachment timed out"), { timedOut: true, deadline: "path" });
const bridgeRefused = () => new Error("no such attachment");

const paint = (threadState, path) => {
  document.body.innerHTML = `<div id="host">${threadHtml(withPicture(path), { threadState })}</div>`;
  return document.querySelector("#host");
};
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const figure = () => document.querySelector(".thread-attachment-figure");

describe("an attachment fetch that failed while the path was dead", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("stays loading rather than saying unavailable", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    wireThreadAttachments(paint(threadState), async () => { throw wireWent(); }, threadState);
    await settle();

    expect(figure().classList.contains("unavailable")).toBe(false);
    expect(document.querySelector("img.thread-attachment-image").getAttribute("src")).toBe(null);
  });

  it("does not ask again on every repaint while the path is still gone", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    let asked = 0;
    const load = async () => {
      asked += 1;
      throw wireWent();
    };
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();

    // A timeline repaints every second and a half; a dead window is a minute.
    expect(asked).toBe(1);
  });

  it("asks again once the device has reconnected, and fills the picture", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    let answer = null;
    const load = async () => {
      if (answer) return answer;
      throw pathDeadline();
    };
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();
    expect(figure().classList.contains("unavailable")).toBe(false);

    answer = { mime: "image/png", content_b64: "AAAA" };
    threadState.retryDeferredAttachments();
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();

    expect(document.querySelector("img.thread-attachment-image").getAttribute("src"))
      .toBe("data:image/png;base64,AAAA");
  });

  it("still says unavailable when the bridge is the one refusing", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    let asked = 0;
    const refuse = async () => {
      asked += 1;
      throw bridgeRefused();
    };
    wireThreadAttachments(paint(threadState), refuse, threadState);
    await settle();
    expect(figure().classList.contains("unavailable")).toBe(true);

    // And a reconnect does not re-ask for it: nothing about the wire was wrong.
    threadState.retryDeferredAttachments();
    wireThreadAttachments(paint(threadState), refuse, threadState);
    await settle();

    expect(asked).toBe(1);
    expect(figure().classList.contains("unavailable")).toBe(true);
  });

  it("keeps a picture it already has through a reconnect", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    let asked = 0;
    const load = async () => {
      asked += 1;
      return { mime: "image/png", content_b64: "AAAA" };
    };
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();

    threadState.retryDeferredAttachments();
    wireThreadAttachments(paint(threadState), load, threadState);
    await settle();

    expect(asked).toBe(1);
    expect(document.querySelector("img.thread-attachment-image").getAttribute("src"))
      .toBe("data:image/png;base64,AAAA");
  });

  it("keeps one conversation's deferred pictures out of another's", async () => {
    const first = createThreadState({ ownerId: "conversation-1" });
    const second = createThreadState({ ownerId: "conversation-2" });
    const load = async () => { throw wireWent(); };
    wireThreadAttachments(paint(first), load, first);
    await settle();

    expect(first.attachmentDeferred("shots/big.png")).toBe(true);
    expect(second.attachmentDeferred("shots/big.png")).toBe(false);
  });

  it("forgets its deferrals when the conversation is let go of", async () => {
    const threadState = createThreadState({ ownerId: "conversation-1" });
    wireThreadAttachments(paint(threadState), async () => { throw wireWent(); }, threadState);
    await settle();
    expect(threadState.attachmentDeferred("shots/big.png")).toBe(true);

    threadState.dispose();

    expect(threadState.attachmentDeferred("shots/big.png")).toBe(false);
  });
});

describe("a device's whole repository releasing what a dead path ate", () => {
  const repositoryWithDeferredPicture = async (conversationIds) => {
    const repository = createChatRepository({
      scope: { accountId: "account-1", deviceId: "device-1" },
      call: async () => ({}),
    });
    for (const conversationId of conversationIds) {
      const { threadState } = repository.history(conversationId);
      wireThreadAttachments(paint(threadState, `shots/${conversationId}.png`), async () => { throw wireWent(); }, threadState);
      await settle();
      expect(threadState.attachmentDeferred(`shots/${conversationId}.png`)).toBe(true);
    }
    return repository;
  };

  it("releases every conversation's deferred pictures at once", async () => {
    const repository = await repositoryWithDeferredPicture(["conversation-1", "conversation-2"]);

    expect(repository.retryDeferredAttachments()).toEqual({ conversations: 2, paths: 2 });
    expect(repository.history("conversation-1").threadState.attachmentDeferred("shots/conversation-1.png")).toBe(false);
    expect(repository.history("conversation-2").threadState.attachmentDeferred("shots/conversation-2.png")).toBe(false);
  });

  it("says nothing happened when nothing was waiting", async () => {
    const repository = createChatRepository({
      scope: { accountId: "account-1", deviceId: "device-1" },
      call: async () => ({}),
    });
    repository.history("conversation-1");

    expect(repository.retryDeferredAttachments()).toEqual({ conversations: 0, paths: 0 });
  });

  it("is quiet on a repository whose scope has gone", async () => {
    const repository = await repositoryWithDeferredPicture(["conversation-1"]);
    repository.dispose();

    expect(repository.retryDeferredAttachments()).toEqual({ conversations: 0, paths: 0 });
  });
});
