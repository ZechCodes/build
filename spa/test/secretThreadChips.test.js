/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";
import { provisionalThreadItem, threadHtml } from "../src/core/thread.js";

const metadata = ({ id, name, scope, ttl }) => ({
  secret_id: id,
  name,
  ttl_seconds: ttl,
  grant: scope === "project"
    ? { scope: "project", project_id: "project-1" }
    : { scope: "agent", agent_id: "agent-1", conversation_id: "conversation-1", generation: "thread-1" },
});

const message = (secretIds, extra = {}) => ({
  type: "message",
  data: { role: "user", body: "ship it", secret_ids: secretIds, sequence: 1, ...extra },
});

const labels = (html) => {
  document.body.innerHTML = html;
  return [...document.querySelectorAll(".thread-secret-chip")].map((chip) => chip.textContent.trim());
};

describe("thread secret metadata chips", () => {
  it("resolves sent secret ids only through supplied cached metadata", () => {
    const cached = [
      metadata({ id: "sec_project", name: "TEST_TOKEN", scope: "project", ttl: null }),
      metadata({ id: "sec_agent", name: "DEPLOY_TOKEN", scope: "agent", ttl: 3600 }),
    ];
    const item = message(["sec_project", "sec_agent"], {
      secrets: [{ secret_id: "sec_forged", name: "RAW_VALUE", scope: "project", ttl_seconds: null }],
    });

    expect(labels(threadHtml({ items: [item] }, { secretMetadata: cached }))).toEqual([
      "TEST_TOKEN · Project · No expiry",
      "DEPLOY_TOKEN · Agent · 1 hour after sending",
    ]);
    expect(threadHtml({ items: [item] })).not.toContain("RAW_VALUE");
  });

  it("does not invent a chip for an id absent from the metadata cache", () => {
    const html = threadHtml({ items: [message(["sec_missing"])] }, { secretMetadata: [] });
    expect(html).not.toContain("thread-secret-chip");
    expect(html).not.toContain("sec_missing");
  });

  it.each([
    [60, "1 minute after sending"],
    [120, "2 minutes after sending"],
    [3600, "1 hour after sending"],
    [7200, "2 hours after sending"],
    [86400, "1 day after sending"],
    [604800, "7 days after sending"],
    [90, "90 seconds after sending"],
  ])("formats a %i-second cached TTL", (ttl, expected) => {
    const cached = [metadata({ id: "sec_ttl", name: "TOKEN", scope: "agent", ttl })];
    expect(labels(threadHtml({ items: [message(["sec_ttl"])] }, { secretMetadata: cached })))
      .toEqual([`TOKEN · Agent · ${expected}`]);
  });

  it("retains secret ids on an optimistic post and reflects a newer cache snapshot", () => {
    const item = provisionalThreadItem({
      operationId: "operation-1",
      message: { body: "queued", secret_ids: ["sec_pending"] },
    });
    expect(item.data.secret_ids).toEqual(["sec_pending"]);

    const first = threadHtml({ items: [item] }, {
      secretMetadata: [metadata({ id: "sec_pending", name: "BEFORE", scope: "agent", ttl: null })],
    });
    const second = threadHtml({ items: [item] }, {
      secretMetadata: [metadata({ id: "sec_pending", name: "AFTER", scope: "project", ttl: 3600 })],
    });
    expect(labels(first)).toEqual(["BEFORE · Agent · No expiry"]);
    expect(labels(second)).toEqual(["AFTER · Project · 1 hour after sending"]);
  });
});
