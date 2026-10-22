// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { inboxRowHtml } from "../src/core/inbox.js";
import { projectAgentEntries } from "../src/core/inboxProjectAgent.js";

const DEVICE = "project-agent-activity";
const project = { id: "project-1", name: "Payments", entity_id: "project-run" };
const agent = (id, over = {}) => ({ id, watched: true, working: false, unread_count: 0, ...over });
const conversation = (over = {}) => ({ kind: "branch", project_id: project.id, run_id: project.entity_id,
  agents: [agent("pin", { topic: "Review checkout" })], ...over });
const stat = { ahead: 1, behind: 2, uncommitted: { files_changed: 2, insertions: 3, deletions: 1 } };
const agentEntry = ({ items = [], runs = [] } = {}) => {
  const view = liveFeedSnapshot({ items, runs }, { projects: [project] }, {}, DEVICE);
  return projectAgentEntries(view.projects, view.items, view.runs)[0];
};
const render = (entry, ui = {}) => new DOMParser().parseFromString(inboxRowHtml(entry, ui), "text/html");

describe("the project agent's conversation facts", () => {
  it.each([{}, { quiet: true }, { showProject: false }, { showProject: false, quiet: true }])(
    "renders its own Git facts and workspace running count with %j", (ui) => {
      const entry = agentEntry({ items: [
        conversation({ run_id: "other-run", pending: "discarding" }),
        conversation({ stat, session_started_ms: 100, last_activity_ms: 200, agents: [
          agent("pin", { topic: "Review checkout", working: true, unread_count: 2 }),
          agent("unwatched", { watched: false, agents_running: 1 }), agent("idle"),
        ] }),
      ] });
      const doc = render(entry, ui);
      expect(doc.querySelector(".inbox-facts").textContent).toBe("2 files · ↑1 ↓2 · +3 −1 · 2 running");
      expect(doc.querySelector(".stitle").textContent).toBe("Review checkout");
      expect(doc.querySelector("[data-menu], [data-done], [data-workspace-done]")).toBeNull();
      expect(entry).toMatchObject({ unreadCount: 2, anchorMs: 100, lastActivityMs: 200 });
    },
  );

  it("gives its pending operation precedence over its Git facts", () => {
    const entry = agentEntry({ items: [conversation({ pending: "updating", stat })] });
    expect(render(entry).querySelector(".inbox-facts").textContent).toBe("Updating… · 0 running");
  });

  it("reads facts from runs when the conversation is absent from items", () => {
    const entry = agentEntry({ runs: [conversation({ stat })] });
    expect(render(entry).querySelector(".inbox-facts").textContent).toBe("2 files · ↑1 ↓2 · +3 −1 · 0 running");
  });

  it("uses Getting started only when there are no conversation facts and counts a known idle roster", () => {
    const entry = agentEntry({ items: [conversation()] });
    expect(render(entry).querySelector(".inbox-facts").textContent).toBe("Getting started · 0 running");
    const unknownRoster = agentEntry({ items: [conversation({ agents: undefined })] });
    expect(render(unknownRoster).querySelector(".inbox-facts").textContent).toBe("Getting started");
    expect(render(agentEntry()).querySelector(".inbox-facts").textContent).toBe("Getting started");
  });

  it.each([[undefined, {}], [undefined, { quiet: true }], [[], {}], [[], { quiet: true }]])(
    "keeps empty facts for a conversation with history and agents %j, with UI %j", (agents, ui) => {
      const entry = agentEntry({ items: [conversation({ agents, session_started_ms: 100, last_activity_ms: 200 })] });
      expect(entry.facts).toBe("");
      const doc = render(entry, ui);
      expect(doc.body.textContent).not.toContain("Getting started");
      expect(doc.querySelector(".inbox-facts")).toBeNull();
    },
  );

  it("renders the latest conversation facts held in the cache", async () => {
    vi.resetModules();
    globalThis.indexedDB = new IDBFactory();
    globalThis.IDBKeyRange = IDBKeyRange;
    const { writeCached } = await import("../src/core/localCache.js");
    const { cachedFeedView } = await import("../src/core/cachedRows.js");
    const { stampProject, stampRow } = await import("../src/core/feedMerge.js");
    const address = (kind, entityId = "") => ({ deviceId: DEVICE, entityId, kind });
    await writeCached(address("projects"), [stampProject(project, DEVICE)]);
    await writeCached(address("row", project.entity_id), stampRow(conversation({ stat }), DEVICE));
    const renderedFacts = async () => {
      const view = await cachedFeedView(DEVICE);
      return render(projectAgentEntries(view.projects, view.items, view.runs)[0]).querySelector(".inbox-facts").textContent;
    };
    expect(await renderedFacts()).toBe("2 files · ↑1 ↓2 · +3 −1 · 0 running");
    await writeCached(address("row", project.entity_id), stampRow(conversation({
      stat: { ahead: 4, uncommitted: { files_changed: 1, insertions: 9, deletions: 0 } },
      agents: [agent("pin", { working: true })],
    }), DEVICE));
    expect(await renderedFacts()).toBe("1 file · ↑4 · +9 −0 · 1 running");
  });
});
