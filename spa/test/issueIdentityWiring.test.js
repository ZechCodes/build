/** @vitest-environment jsdom */
// The bridge contract, real cache, and real issue renderer in one path.
import { beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import answer from "../../fixtures/api/v1/issues.get.json";
import { issueRecord, readIssueRecord, writeIssueRecord } from "../src/core/trackerCache.js";
import { timelineRows } from "../src/core/trackerTimeline.js";
import { issuePageHtml } from "../src/core/trackerIssueRender.js";
import { issueLinkRows } from "../src/core/trackerLinks.js";
import { issueNoticeLineHtml } from "../src/core/trackerNotice.js";
import { issueRowHtml } from "../src/core/trackerListRender.js";
import { issueCardHtml } from "../src/core/trackerBoardRender.js";

const DEVICE = "dev-1";
const PROJECT = "proj-1";
const AGENT = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
const place = { deviceId: DEVICE, projectId: PROJECT, projectKey: "dev-1|proj-1" };
const workspace = { workspace_id: "ws-3f2a91c4", id: "ws-3f2a91c4", name: "spa-flaky-tests", projectKey: place.projectKey };

const renderCached = async (feed = { workspaces: [workspace] }) => {
  const record = await readIssueRecord(DEVICE, PROJECT, answer.params.issue_id);
  const host = document.createElement("div");
  host.innerHTML = issuePageHtml(record.issue, {
    ...place,
    projectName: "Build",
    identities: record.issue.identities,
    rows: timelineRows(record.timeline),
    links: issueLinkRows(record.issue, place, feed),
    columns: [], draft: "", labelsDraft: "", busy: false, sending: false,
  });
  return host;
};

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
});

describe("bridge identity through the issue cache", () => {
  it("names an unwatched comment author, draws its harness, and links its agent", async () => {
    // No feed item or watched-agent digest is involved in this path.
    await writeIssueRecord(DEVICE, PROJECT, answer.params.issue_id,
      issueRecord(answer.result.issue, answer.result.timeline));
    const host = await renderCached();
    const author = host.querySelector(".issue-comment .issue-entry-head strong a");
    expect(author.textContent).toBe("spa-flaky-tests · Fix drag");
    expect(author.href).toContain(`agent=${AGENT}`);
    expect(host.querySelector(".issue-comment .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
  });

  it("keeps a finished workspace's identity and icon but leaves deleted destinations as text", async () => {
    const issue = structuredClone(answer.result.issue);
    issue.identities[AGENT].available = false;
    await writeIssueRecord(DEVICE, PROJECT, answer.params.issue_id,
      issueRecord(issue, answer.result.timeline));
    const host = await renderCached({ workspaces: [] });
    expect(host.querySelector(".issue-comment .issue-entry-head strong").textContent).toBe("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".issue-comment .issue-entry-head a")).toBeNull();
    expect(host.querySelector(".issue-comment .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
    expect(host.querySelector(".issue-assignee-current a")).toBeNull();
    expect(host.querySelector(".issue-assign-open").textContent.replace(/\s+/g, " ").trim()).toBe("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".issue-links a[href*='workspace']")).toBeNull();
  });

  it("uses Agent 1 when the captured agent has no name", async () => {
    const issue = structuredClone(answer.result.issue);
    issue.identities[AGENT].name = null;
    await writeIssueRecord(DEVICE, PROJECT, answer.params.issue_id,
      issueRecord(issue, answer.result.timeline));
    const host = await renderCached();
    expect(host.querySelector(".issue-comment .issue-entry-head strong").textContent).toBe("spa-flaky-tests · Agent 1");
  });

  it("links the project agent to the project and keeps its square mark", async () => {
    const issue = structuredClone(answer.result.issue);
    issue.assignee = { kind: "agent", agent_id: "project-01M2SCB" };
    const timeline = [{ type: "comment", id: "ic-project", author: issue.assignee, body: "Ready.", created_at: "2026-09-19T10:12:00Z" }];
    await writeIssueRecord(DEVICE, PROJECT, answer.params.issue_id, issueRecord(issue, timeline));
    const host = await renderCached();
    expect(host.querySelector(".issue-comment .issue-entry-head a").textContent).toBe("Build");
    expect(host.querySelector(".issue-comment .issue-entry-head a").getAttribute("href")).toContain("/project/proj-1");
    expect(host.querySelector(".issue-comment .issue-avatar.is-project")).not.toBeNull();
  });

  it("links the actor, assignment target, dispatch target, assignee and workspace", async () => {
    const issue = structuredClone(answer.result.issue);
    const agent = { kind: "agent", agent_id: AGENT };
    const timeline = [
      { type: "event", id: "assigned", kind: "assigned", actor: agent, payload: { assignee: agent } },
      { type: "event", id: "dispatched", kind: "dispatched", actor: { kind: "user" }, payload: { agent_id: AGENT } },
      { type: "event", id: "linked", kind: "linked", actor: { kind: "user" }, payload: { workspace_id: workspace.id } },
    ];
    await writeIssueRecord(DEVICE, PROJECT, answer.params.issue_id, issueRecord(issue, timeline));
    const host = await renderCached();
    expect(host.querySelectorAll(".issue-event .issue-actor-link[href*='agent=']")).toHaveLength(3);
    expect(host.querySelector(".issue-assignee-current a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".issue-event:last-child a[href*='workspace']")).not.toBeNull();
    expect(host.querySelector(".issue-links a[href*='workspace']")).not.toBeNull();
  });

  it("names an unwatched notice actor from its wire identity and draws the harness", () => {
    const identity = answer.result.issue.identities[AGENT];
    const html = issueNoticeLineHtml({
      issue_id: answer.params.issue_id, number: 102, title: "A test", action: "commented",
      actor: { kind: "agent", agent_id: AGENT, identity },
    }, { place, projectName: "Build" });
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelector(".thread-issue-by").textContent).toContain("spa-flaky-tests · Fix drag");
    expect(host.querySelector(".rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
  });

  it("names an unwatched notice assignee and draws its harness", () => {
    const identity = answer.result.issue.identities[AGENT];
    const html = issueNoticeLineHtml({
      issue_id: answer.params.issue_id, number: 102, title: "A test", action: "assigned",
      actor: { kind: "user" }, assignee: { kind: "agent", agent_id: AGENT }, assignee_identity: identity,
    }, { place, projectName: "Build" });
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelector(".thread-issue-said").textContent.replace(/\s+/g, " ").trim()).toBe("assigned to spa-flaky-tests · Fix drag");
    expect(host.querySelector(".thread-issue-said .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
  });

  it("uses the issue identity on list rows and board cards without a watched digest", () => {
    for (const html of [issueRowHtml, issueCardHtml]) {
      const host = document.createElement("div");
      host.innerHTML = html(answer.result.issue, { href: () => "#/issue", projectName: "Build", columns: [] });
      expect(host.querySelector(".issue-assignee").textContent.replace(/\s+/g, " ").trim()).toBe("spa-flaky-tests · Fix drag");
      expect(host.querySelector(".issue-assignee .rail-harness-icon")?.dataset.harnessIcon).toBe("codex_app_server");
    }
  });
});
