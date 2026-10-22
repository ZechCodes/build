/** @vitest-environment jsdom */
import { expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

it("gives identical notes in separate messages distinct stable document ids", () => {
  const body = "The result[^note].\n\n[^note]: More detail.";
  const thread = { id: "thread-one", items: ["message-a", "message-b"].map((id, sequence) => ({
    type: "message", data: { id, sequence: sequence + 1, role: "agent", body },
  })) };
  const html = threadHtml(thread);
  const host = document.createElement("div");
  host.innerHTML = html;
  const notes = [...host.querySelectorAll(".md-footnotes li")];
  expect(notes).toHaveLength(2);
  expect(new Set(notes.map((note) => note.id)).size).toBe(2);
  expect(threadHtml(thread)).toBe(html);
  for (const message of host.querySelectorAll(".thread-message")) {
    const reference = message.querySelector(".md-footnote-ref a");
    expect(reference.getAttribute("href")).toBe(`#${message.querySelector(".md-footnotes li").id}`);
  }
});
