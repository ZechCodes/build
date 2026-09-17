// @vitest-environment jsdom
// One agent's words in another agent's conversation.
//
// They arrive on the user's side of the thread, because that is the side
// anything addressed to the agent arrives on — and the bubble stays there. What
// changes is the mark beside it: the human's initial would claim they sent
// something they never wrote, so an attributed message wears its sender.
import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

const sentByRouter = (from_agent) => threadHtml({
  items: [{
    type: "message",
    data: { sequence: 1, role: "user", body: "finish the toast", from_agent },
  }],
});

describe("a message one agent sent another", () => {
  it("stays on the user's side and wears the sender instead of the human", () => {
    document.body.innerHTML = sentByRouter({ id: "router-ef0a0d3f-f02d-49d6" });

    const message = document.querySelector(".thread-message");
    expect(message.classList.contains("user")).toBe(true);
    const avatar = message.querySelector(".thread-avatar");
    expect(avatar.textContent).not.toBe("Y");
    expect(avatar.textContent).toBe("EF0A");
    expect(avatar.classList.contains("from-agent")).toBe(true);
    expect(avatar.getAttribute("aria-label")).toBe("Sent by agent router-ef0a0d3f-f02d-49d6");
  });

  it("says Agent when the sender's id has nothing short to show", () => {
    document.body.innerHTML = sentByRouter({ id: "agent-" });

    expect(document.querySelector(".thread-avatar").textContent).toBe("Agent");
  });

  it("leaves the human's own message exactly as it was", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "message", data: { sequence: 1, role: "user", body: "finish the toast" } }],
    });

    const avatar = document.querySelector(".thread-avatar");
    expect(avatar.textContent).toBe("Y");
    expect(avatar.classList.contains("from-agent")).toBe(false);
    expect(avatar.getAttribute("aria-hidden")).toBe("true");
  });
});
