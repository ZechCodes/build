// @vitest-environment jsdom
// Step 7 of the session spec: an outcome is a status on the agent's own
// message, not a `Done` / `Blocked` event beside it. The timeline has to read
// the same as it did before — the same icon, the same words, the same tone, and
// the structured handoff still as a card — with the marker riding the message
// the agent posted. Threads written before step 7 still carry the events, and
// they must keep rendering exactly as they always have.

import { describe, expect, it } from "vitest";
import { threadHtml } from "../src/core/thread.js";

const REPORT = {
  critical_files: ["src/thread.rs — where an outcome is written down"],
  risk_notes: ["the catch-up packet now carries outcome lines"],
  decisions: ["the report rides the message"],
  skips: ["did not touch the events"],
};

const outcomeMessage = (outcome, extra = {}) => ({
  type: "message",
  data: {
    role: "agent",
    body: "Wired the outcome onto the message.",
    created_at: "2026-08-24T09:00:00Z",
    outcome,
    ...extra,
  },
});

describe("an outcome carried by the agent's message", () => {
  it("draws a completed outcome as an agent message wearing the done marker and its report", () => {
    document.body.innerHTML = threadHtml({
      items: [outcomeMessage("completed", { done: true, completion_report: REPORT })],
    });

    const message = document.querySelector(".thread-message");
    expect(message.classList.contains("agent")).toBe(true);
    expect(message.querySelector(".thread-message-head").textContent).toContain("Agent commented");
    expect(message.textContent).toContain("Wired the outcome onto the message.");

    const marker = message.querySelector(".thread-outcome");
    expect(marker.dataset.outcome).toBe("completed");
    expect(marker.classList.contains("success")).toBe(true);
    expect(marker.textContent).toContain("Agent reported done");

    const report = message.querySelector(".completion-report");
    expect(report.textContent).toContain("Critical files");
    expect(report.textContent).toContain("src/thread.rs — where an outcome is written down");
    expect(report.textContent).toContain("Risks");
    expect(report.textContent).toContain("Decisions");
    expect(report.textContent).toContain("Skipped");
  });

  it("draws a blocked outcome with the blocker's marker and the reason the agent gave", () => {
    document.body.innerHTML = threadHtml({
      items: [outcomeMessage("blocked", { body: "The migration needs production credentials." })],
    });

    const marker = document.querySelector(".thread-outcome");
    expect(marker.dataset.outcome).toBe("blocked");
    expect(marker.classList.contains("blocked")).toBe(true);
    expect(marker.textContent).toContain("Agent reported a blocker");
    expect(document.querySelector(".thread-body").textContent).toContain("The migration needs production credentials.");
    expect(document.querySelector(".completion-report")).toBeNull();
  });

  it("draws a failed outcome with the failure's marker", () => {
    document.body.innerHTML = threadHtml({
      items: [outcomeMessage("failed", { body: "The report named no files Build could apply." })],
    });

    const marker = document.querySelector(".thread-outcome");
    expect(marker.dataset.outcome).toBe("failed");
    expect(marker.classList.contains("blocked")).toBe(true);
    expect(marker.textContent).toContain("Agent reported failure");
  });

  it("names the harness on the marker, as the event row it replaces did", () => {
    document.body.innerHTML = threadHtml({
      sessions: [{ provider: "Codex CLI" }],
      items: [outcomeMessage("completed", { done: true })],
    });
    const marker = document.querySelector(".thread-outcome");
    expect(marker.textContent).toContain("Codex reported done");
    expect(marker.textContent).not.toContain("Agent reported done");
  });

  it("escapes what the agent wrote in the report on a message", () => {
    const html = threadHtml({
      items: [
        outcomeMessage("completed", {
          done: true,
          completion_report: { decisions: ["<img src=x onerror=alert(1)>"] },
        }),
      ],
    });
    expect(html).not.toContain("<img src=x");
    expect(html).toContain("&lt;img");
  });

  it("leaves an ordinary agent message unmarked", () => {
    document.body.innerHTML = threadHtml({
      items: [{ type: "message", data: { role: "agent", body: "Which name?" } }],
    });
    expect(document.querySelector(".thread-message")).not.toBeNull();
    expect(document.querySelector(".thread-outcome")).toBeNull();
    expect(document.querySelector(".completion-report")).toBeNull();
  });

  // Before step 7 the record was the event and the message beside it carried
  // only `done`. Those threads are persisted and unmigrated, so the event keeps
  // its row, keeps its card, and the message beside it stays unmarked.
  it("renders a pre-step-7 thread exactly as it always did", () => {
    document.body.innerHTML = threadHtml({
      items: [
        { type: "event", data: { event: "done", summary: "Finished the task.", completion_report: REPORT } },
        { type: "message", data: { role: "agent", done: true, body: "Finished the task." } },
      ],
    });

    const event = document.querySelector(".thread-event");
    expect(event.classList.contains("success")).toBe(true);
    expect(event.textContent).toContain("Agent reported done");
    expect(event.querySelector(".completion-report").textContent).toContain("Critical files");
    expect(document.querySelectorAll(".completion-report")).toHaveLength(1);
    expect(document.querySelector(".thread-message .thread-outcome")).toBeNull();
  });
});
