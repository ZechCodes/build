import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { settled } from "./chatMenuSeed.mjs";
import { flowTerminalShim, mountProjectFlow } from "./projectWorkspaceFlowFixture.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim, flowTerminalShim] }); });
afterAll(async () => { await session?.close(); });

const PIN = '[data-bubble="agent"][data-agent="project-pin"]';
const CHILD_PIN = '[data-bubble="project"][data-agent="project-owner"]';
const SIBLING = '[data-bubble="agent"][data-agent="project-sibling"]';
const CREATED = '[data-bubble="agent"][data-agent="project-created"]';
const INPUT = ".thread-composer textarea";
const SAVED_CONTENT = "# shared source wording\n";
const callsTo = (page, method) => page.evaluate((method) => window.__flowCalls.filter((call) => call.method === method), method);

async function selectAgent(page, selector) {
  await page.locator(selector).focus();
  await page.keyboard.press("Enter");
  await page.locator(INPUT).waitFor();
  await settled(page);
}

async function openSourceShell(page, sourceId) {
  await page.getByLabel("Terminal source", { exact: true }).selectOption(sourceId);
  await page.getByRole("button", { name: "New terminal", exact: true }).click();
  await page.waitForFunction((sourceId) => window.__flowCalls.some((call) => call.method === "term.attach" && call.params.source_id === sourceId), sourceId);
  await page.locator(".project-terminal-screen").waitFor();
}

async function createWorker(page) {
  await selectAgent(page, '[data-bubble="add"]');
  expect(await page.locator(".rail-worker-notice").textContent()).toContain("Files, Git index and current branch are shared");
  await page.locator('.rail-harness-picker [data-provider="codex"]').click();
  await page.locator("[data-new-agent-name]").fill("Release helper");
  await page.locator(INPUT).fill("Check the local base commit");
  await page.locator(".composer-send").click();
  await page.waitForFunction(() => window.__flowCalls.some((call) => call.method === "thread.post"));
  await page.locator(CREATED).waitFor();
}

for (const width of [390, 1280]) {
  it(`accepts the combined original-source / worker / console / child journey (${width}px)`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await mountProjectFlow(page, basePath);
      expect(await callsTo(page, "term.create")).toEqual([]);
      expect(await callsTo(page, "term.attach")).toEqual([]);
      const bubbles = await page.locator(".rail-strip > *").evaluateAll((nodes) => nodes.map((node) =>
        node.getAttribute("role") === "separator" ? "separator" : node.dataset.agent || node.dataset.bubble));
      expect(bubbles.slice(0, 4)).toEqual(["project-pin", "separator", "project-sibling", "add"]);
      expect(await page.locator('[data-directory]').count()).toBe(4);
      await captureLayout(page, `538-changes-${width}.png`);

      // Edit the original source through Files, then use the unchanged
      // commit-all pane. Equal status/content keys cannot select Docs instead.
      await page.locator('[data-tab="files"]').click();
      if (width === 390) await page.getByRole("button", { name: "Show files", exact: true }).click();
      await page.locator('[data-root="code"] .frow[data-path="README.md"]').dblclick();
      await page.locator('[data-file-mode="edit"]').click();
      await page.locator(".file-editor").fill(SAVED_CONTENT);
      await page.locator(".file-save").click();
      await page.waitForFunction(() => window.__flowCalls.some((call) => call.method === "fs.write"));
      const write = (await callsTo(page, "fs.write"))[0];
      expect(write).toMatchObject({ deviceId: "flow-device",
        params: { project_id: "p", source_id: "code", source_path: "/fixture/code", path: "README.md", expected_revision: "revision-1" } });
      expect(Buffer.from(write.params.content_b64, "base64").toString("utf8")).toBe(SAVED_CONTENT);
      const saved = await page.evaluate(() => window.__flowCall("flow-device", "fs.read", {
        project_id: "p", source_id: "code", source_path: "/fixture/code", path: "README.md",
      }));
      expect(Buffer.from(saved.content_b64, "base64").toString("utf8")).toBe(SAVED_CONTENT);
      await page.locator('[data-tab="changes"]').click();
      await page.locator('[data-surface="code"] .csinput').fill("Commit the shared source edit");
      await page.locator('[data-surface="code"] .csbox-actions button').filter({ hasText: "Commit" }).click();
      await page.waitForFunction(() => window.__flowCalls.some((call) => call.method === "git.commit"));
      expect(await callsTo(page, "git.stage")).toEqual([{ deviceId: "flow-device", method: "git.stage",
        params: { project_id: "p", source_id: "code", source_path: "/fixture/code", paths: ["README.md"] } }]);
      expect(await callsTo(page, "git.commit")).toEqual([{ deviceId: "flow-device", method: "git.commit",
        params: { project_id: "p", source_id: "code", source_path: "/fixture/code", message: "Commit the shared source edit" } }]);
      const bytes = await page.evaluate(() => {
        const held = window.__flowDevices["flow-device"];
        return { saved: held.files.get("code"), staged: held.staged.get("code"), committed: held.committed.get("code").files };
      });
      expect(bytes).toEqual({ saved: SAVED_CONTENT, staged: { "README.md": SAVED_CONTENT }, committed: { "README.md": SAVED_CONTENT } });
      const committed = await page.evaluate(() => window.__flowCall("flow-device", "git.show", {
        project_id: "p", source_id: "code", source_path: "/fixture/code", hash: "e".repeat(40),
      }));
      expect(committed.subject).toBe("Commit the shared source edit");
      expect(committed.patch).toBe("diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-# Build code source\n+# shared source wording\n");
      await page.locator('[data-directory="docs"]').click();
      await page.waitForFunction(() => document.querySelector('[data-surface="docs"]')?.textContent.includes("shared source wording"));
      await page.locator('[data-surface="docs"] .csinput').fill("Keep the Docs commit draft");
      await selectAgent(page, SIBLING);
      await page.locator(INPUT).fill("Keep this sibling conversation draft");
      await createWorker(page);
      const added = (await callsTo(page, "agent.add"))[0];
      expect(added).toMatchObject({ deviceId: "flow-device", params: { entity_id: "project-owner", project_role: "worker", name: "Release helper" } });
      expect((await callsTo(page, "thread.post"))[0].params).toMatchObject({ entity_id: "project-owner", agent_id: "project-created",
        conversation_id: "conversation-project-created", body: "Check the local base commit" });

      // Open the same assignment control used by tasks, populated from the
      // production complete-roster projection rather than a hand-built option.
      await page.evaluate(() => {
        const { assignee, feed } = window.__layoutModules;
        window.__flowStopAssignmentFeed = feed.subscribeFeed((snapshot) => {
          window.__flowAssignmentOptions = assignee.assigneeOptions(assignee.assignableAgents(snapshot, "flow-device/p"));
        });
        void feed.refreshFeed();
      });
      // Cache writes announce asynchronously; a visible provisional bubble
      // does not yet mean the complete roster has reached the task feed.
      await page.waitForFunction(() => window.__flowAssignmentOptions?.some((option) => option.id === "agent:project-created"));
      await page.evaluate(() => {
        const { picker } = window.__layoutModules;
        window.__flowStopAssignmentFeed();
        window.__flowPicker = picker.openAssigneePicker({ task: { id: "task-acceptance", number: 538, title: "Accept project workspace" },
          options: window.__flowAssignmentOptions, current: "agent:project-created",
          callRpc: (method, params) => window.__flowCall("flow-device", method, params),
          onAssigned: (answer) => { window.__flowAssignmentAnswer = answer; } });
      });
      await page.locator("[data-assign-go]").click();
      await page.waitForFunction(() => window.__flowAssignmentAnswer);
      expect(await callsTo(page, "tasks.assign")).toEqual([{ deviceId: "flow-device", method: "tasks.assign",
        params: { task_id: "task-acceptance", assignee: { kind: "agent", agent_id: "project-created" } } }]);
      expect(await page.evaluate(() => window.__flowAssignmentAnswer.task.links.workspace_ids)).toEqual([]);

      await page.locator("#console-toggle").click();
      await openSourceShell(page, "code");
      await openSourceShell(page, "assets");
      expect(await page.locator(".console-tab-name").allTextContents()).toEqual(["Code · Source shell", "Assets · Source shell"]);
      expect((await callsTo(page, "term.create")).map((call) => call.params)).toEqual([
        { project_id: "p", source_id: "code", source_path: "/fixture/code" },
        { project_id: "p", source_id: "assets", source_path: "/fixture/assets" },
      ]);
      await captureLayout(page, `538-console-${width}.png`);
      // The phone's chat card dismisses when the source console is used.
      if (width === 390) await selectAgent(page, CREATED);
      await page.locator(INPUT).fill("Keep this worker draft on return");

      // Mount the real child Changes view with colliding source IDs, so its
      // checkout content must remain separate from the original sources.
      await page.evaluate(() => window.__flowNavigate({ name: "workspace", deviceId: "flow-device", projectId: "p", workspaceId: "child", tab: "changes", sourceId: "code", agent: "child-agent" }));
      await page.locator('[data-surface="code"] .workspace-refbar').waitFor();
      expect(await page.locator('[data-directory="code"]').textContent()).toContain("Child Code");
      expect(await page.locator('[data-surface="code"] .workspace-refbar').textContent()).toContain("build/local-commit-workspace");
      if (width === 390) await page.getByRole("button", { name: "Show commits", exact: true }).click();
      await page.locator('[data-surface="code"] [data-sel="uncommitted"]').click();
      await page.waitForFunction(() => document.querySelector('[data-surface="code"]')?.textContent.includes("child checkout wording"));
      expect(await page.locator('[data-surface="code"]').textContent()).toContain("CHILD.md");
      expect((await callsTo(page, "git.diff")).filter((call) => call.params.workspace_id === "child")).toContainEqual({ deviceId: "flow-device", method: "git.diff",
        params: { workspace_id: "child", source_id: "code", entity_id: "child-owner", paths: ["CHILD.md"] } });
      await page.locator('[data-directory="assets"]').click();
      expect(await page.locator('[data-surface="assets"]').textContent()).toContain("Child Assets has no Git repository");
      await page.locator('[data-agent="child-agent"]').waitFor();
      expect(await page.locator(CREATED).count()).toBe(0);
      expect(await page.locator(SIBLING).count()).toBe(0);
      expect(await page.locator(CHILD_PIN).count()).toBe(1);
      await selectAgent(page, CHILD_PIN);
      expect(await page.locator(".thread-items").textContent()).toContain("Cached Build: Build coordinator");
      await page.locator(INPUT).fill("Check this child from the coordinator conversation");
      await page.locator(".composer-send").click();
      await page.waitForFunction(() => window.__flowCalls.some((call) => call.method === "thread.post"
        && call.params.body === "Check this child from the coordinator conversation"));
      const childPost = (await callsTo(page, "thread.post")).find((call) => call.params.body === "Check this child from the coordinator conversation");
      expect(childPost.params).toMatchObject({ entity_id: "project-owner", agent_id: "project-pin", conversation_id: "conversation-project-pin" });
      expect(await page.locator(CREATED).count()).toBe(0);
      expect(await page.locator(SIBLING).count()).toBe(0);
      await page.evaluate(async () => {
        const route = await window.__layoutModules.returnRoute.projectReturnRoute(window.__flowDevices["flow-device"].record);
        await window.__flowNavigate({ ...route, agent: "project-created" });
      });
      await page.locator(CREATED).waitFor();
      await page.locator(INPUT).waitFor();
      expect(await page.locator(INPUT).inputValue()).toBe("Keep this worker draft on return");
      expect(await page.locator('[data-directory="code"]').textContent()).toContain("Code");
      expect(await page.locator('[data-directory="docs"]').count()).toBe(1);
      expect(await page.locator("#root").textContent()).not.toContain("child checkout wording");
      expect(await page.locator('[data-surface="docs"] .csinput').inputValue()).toBe("Keep the Docs commit draft");
      expect(await page.locator(".console-tab-name").allTextContents()).toEqual(["Code · Source shell", "Assets · Source shell"]);
      expect(await callsTo(page, "term.close")).toEqual([]);

      // Project-hosted task context retains these workers; a task assignee is
      // never used to invent a workspace host for its shell.
      const sameStrip = await page.evaluate(() => {
        const { app, shell } = window.__layoutModules;
        const strip = document.querySelector(".rail-strip");
        app.App.route = { ...app.App.route, name: "trackerTask", taskId: "task-acceptance" };
        shell.standShell(app.App.route);
        return strip === document.querySelector(".rail-strip");
      });
      expect(sameStrip).toBe(true);
      expect(await page.locator(CREATED).count()).toBe(1);
      await page.evaluate(() => window.__flowNavigate({ name: "project", deviceId: "flow-device", projectId: "p", tab: "changes", sourceId: "docs", agent: "project-created" }));
      await page.locator(INPUT).waitFor();
      await settled(page);
      // The shared Git pane holds async repaints while a commit draft exists.
      // Explicitly revisit Docs to paint its retained body with that draft.
      await page.locator('[data-directory="code"]').click();
      await page.locator('[data-directory="docs"]').click();
      if (!await page.locator(INPUT).isVisible()) await selectAgent(page, CREATED);
      await page.waitForFunction(() => document.querySelector('[data-surface="docs"]')?.textContent.includes("shared source wording"));
      await captureLayout(page, `538-agent-separator-${width}.png`);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.locator(".rail-remove").click();
      await page.locator("[data-confirm-ok]").click();
      await page.locator(CREATED).waitFor({ state: "detached" });
      expect(await callsTo(page, "agent.remove")).toEqual([{ deviceId: "flow-device", method: "agent.remove",
        params: { entity_id: "project-owner", agent_id: "project-created" } }]);
      expect(await page.locator(PIN).count()).toBe(1);
      await selectAgent(page, SIBLING);
      expect(await page.locator(INPUT).inputValue()).toBe("Keep this sibling conversation draft");
      expect(await page.locator('.thread-items').textContent()).toContain("Cached Build: Docs worker");
      expect(await page.locator('[data-surface="docs"] .csinput').inputValue()).toBe("Keep the Docs commit draft");
      await page.locator('.console-tab').filter({ hasText: "Assets · Source shell" }).locator('[data-close]').click();
      await page.waitForFunction(() => !document.querySelector(".console-tabs")?.textContent.includes("Assets · Source shell"));
      expect((await callsTo(page, "term.close"))[0].params).toEqual({ project_id: "p", source_id: "assets", source_path: "/fixture/assets", term_id: "term-assets" });
      await page.evaluate(() => { window.__layoutModules.app.App.viewDispose?.(); window.__layoutModules.shell.stopShell(); });
    }, { width, height: width === 390 ? 844 : 900 });
  }, 60_000);
}

it("paints a cold cache, reconnects without losing source or agent drafts, then safely downgrades", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectFlow(page, basePath, { greeting: false });
    await page.locator('[data-surface="code"] .csinput').fill("Cold cache commit draft");
    await selectAgent(page, SIBLING);
    await page.locator(INPUT).fill("Cold cache worker draft");
    expect(await callsTo(page, "git.status")).toEqual([]);
    expect(await callsTo(page, "agent.list")).toEqual([]);
    await page.evaluate(() => {
      window.__flowHeld = { input: document.querySelector(".thread-composer textarea"), commit: document.querySelector('[data-surface="code"] .csinput') };
    });
    for (let attempt = 0; attempt < 2; attempt += 1) {
      await page.evaluate(() => window.__flowGreet());
      await settled(page);
      expect(await page.locator(INPUT).inputValue()).toBe("Cold cache worker draft");
      expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("Cold cache commit draft");
      expect(await page.evaluate(() => window.__flowHeld.input === document.querySelector(".thread-composer textarea")
        && window.__flowHeld.commit === document.querySelector('[data-surface="code"] .csinput'))).toBe(true);
    }
    await page.evaluate(() => { window.__flowCalls.length = 0; });
    await page.evaluate(() => window.__flowGreet("flow-device", false));
    await page.locator(SIBLING).waitFor({ state: "detached" });
    expect(await page.locator(PIN).count()).toBe(1);
    expect(await page.locator('[data-bubble="add"]').count()).toBe(0);
    await page.locator('[data-directory="docs"]').click();
    expect(await page.locator('[data-surface="docs"]').textContent()).toContain("Update this device");
    await page.locator("#console-toggle").click();
    expect(await page.locator('.console-empty').textContent()).toContain("Update this device");
    const calls = await page.evaluate(() => window.__flowCalls);
    expect(calls.filter((call) => call.params.source_id || call.params.source_path || call.params.include_project_workers || call.params.project_role)).toEqual([]);
    await page.evaluate(() => window.__flowGreet());
    await page.locator(SIBLING).waitFor();
    await selectAgent(page, SIBLING);
    expect(await page.locator(INPUT).inputValue()).toBe("Cold cache worker draft");
    await page.locator('[data-directory="code"]').click();
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("Cold cache commit draft");
  });
}, 60_000);

it("keeps colliding device/source/agent/terminal IDs distinct and refuses a moved captured terminal location", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectFlow(page, basePath);
    await page.locator('[data-surface="code"] .csinput').fill("First device commit draft");
    await selectAgent(page, SIBLING);
    await page.locator(INPUT).fill("First device worker draft");
    await page.locator("#console-toggle").click();
    await openSourceShell(page, "code");
    await page.evaluate(() => window.__flowNavigate({ name: "project", deviceId: "other-device", projectId: "p", tab: "changes", sourceId: "code", agent: "project-sibling" }));
    await page.locator(INPUT).waitFor();
    expect(await page.locator(INPUT).inputValue()).toBe("");
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("");
    await page.locator(INPUT).fill("Other device worker draft");
    await page.locator("#console-toggle").click();
    await openSourceShell(page, "code");
    expect(await page.locator('.project-terminal-screen').textContent()).toContain("other-device");
    await page.evaluate(() => window.__flowNavigate({ name: "project", deviceId: "flow-device", projectId: "p", tab: "changes", sourceId: "code", agent: "project-sibling" }));
    await page.locator(INPUT).waitFor();
    expect(await page.locator(INPUT).inputValue()).toBe("First device worker draft");
    expect(await page.locator('[data-surface="code"] .csinput').inputValue()).toBe("First device commit draft");
    expect((await callsTo(page, "term.create")).map((call) => [call.deviceId, call.params.source_id])).toEqual([["flow-device", "code"], ["other-device", "code"]]);
    const refused = await page.evaluate(async () => {
      const { cache, contexts, terminalRpc, terminalCache } = window.__layoutModules;
      const oldScope = { project_id: "p", source_id: "code", source_path: "/fixture/code" };
      const held = window.__flowDevices["flow-device"];
      held.record = { ...held.record, sources: held.record.sources.map((source) => source.id === "code" ? { ...source, path: "/fixture/moved-code" } : source) };
      await cache.writeCached({ deviceId: "flow-device", entityId: "", kind: "projects" }, [held.record]);
      const context = contexts.knownDeviceContext("flow-device");
      const replies = [];
      for (const method of ["term.attach", "term.close", "term.create"]) {
        try { await terminalRpc.projectTerminalRpc(context, method, { ...oldScope, term_id: "term-code" }); replies.push("sent"); }
        catch (error) { replies.push(error.message); }
      }
      const staleWritten = await terminalCache.writeProjectTerminalRoster({ deviceId: "flow-device", active: () => true }, oldScope, [{ term_id: "late-term" }], { pushed: true });
      return { replies, staleWritten };
    });
    expect(refused.replies.every((message) => message.includes("moved or was removed"))).toBe(true);
    expect(refused.staleWritten).toBe(false);
    expect(await page.locator('[data-recovered-draft="code"] textarea').inputValue()).toBe("First device commit draft");
    const other = await page.evaluate(async () => {
      const { cache, directory } = window.__layoutModules;
      const entityId = directory.directoryCacheId({ project_id: "p", source_id: "code", source_path: "/fixture/code" });
      return (await cache.readCached({ deviceId: "other-device", entityId, kind: "terminals" })).value;
    });
    expect(other.tabs).toEqual([{ term_id: "term-code", name: "Source shell" }]);
  });
}, 60_000);

it("keeps plain sources usable and commits an unborn secondary without fabricating history", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectFlow(page, basePath);
    await page.locator('[data-directory="assets"]').click();
    expect(await page.locator('[data-surface="assets"]').textContent()).toContain("has no Git repository");
    expect(await page.locator('[data-init-git]').count()).toBe(0);
    expect((await callsTo(page, "git.status")).some((call) => call.params.source_id === "assets")).toBe(false);
    await page.locator('[data-directory="unborn"]').click();
    expect(await page.locator('[data-surface="unborn"] .commitheader').count()).toBe(0);
    await page.locator('[data-surface="unborn"] .csinput').fill("First source commit");
    await page.locator('[data-surface="unborn"] .csbox-actions button').filter({ hasText: "Commit" }).click();
    await page.waitForFunction(() => window.__flowCalls.some((call) => call.method === "git.commit"));
    expect((await callsTo(page, "git.commit"))[0].params).toEqual({ project_id: "p", source_id: "unborn", source_path: "/fixture/unborn", message: "First source commit" });
  });
}, 60_000);

// Frozen pre-project-worker requests replay the legacy client contract. The
// bridge is an isolated protocol fixture, not an old executable or live device.
const LEGACY_PROJECT_REQUESTS = [
  { method: "git.status", params: { project_id: "p" } },
  { method: "fs.read", params: { project_id: "p", path: "README.md" } },
  { method: "agent.list", params: { entity_id: "project-owner" } },
  { method: "thread.post", params: { entity_id: "project-owner", agent_id: "project-pin",
    conversation_id: "conversation-project-pin", body: "Legacy pin message" } },
].map((request) => Object.freeze({ ...request, params: Object.freeze(request.params) }));

it.each([false, true])("replays the frozen legacy client contract against a %s worker-capable protocol fixture", async (supported) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountProjectFlow(page, basePath, { supported });
    const result = await page.evaluate(async ({ requests, supported }) => {
      // The current production adapter negotiates either frozen old greeting
      // or the new greeting while these legacy caller bytes stay unchanged.
      const context = window.__layoutModules.contexts.knownDeviceContext("flow-device");
      const replies = [];
      const first = window.__flowCalls.length;
      for (const request of requests) replies.push(await context.rpc(request.method, request.params));
      const calls = window.__flowCalls.slice(first);
      const forbidden = [];
      if (!supported) {
        for (const params of [{ project_id: "p", source_id: "code" }, { project_id: "p", source_path: "/fixture/code" }]) {
          try { await window.__flowCall("flow-device", "git.status", params); forbidden.push("accepted"); }
          catch { forbidden.push("refused"); }
        }
      }
      return { replies, calls, forbidden };
    }, { requests: LEGACY_PROJECT_REQUESTS, supported });
    expect(result.calls).toEqual(LEGACY_PROJECT_REQUESTS.map((request) => ({ deviceId: "flow-device", ...request })));
    expect(result.replies[0].branch).toBe("main");
    expect(result.replies[1].revision).toBe("revision-1");
    expect(result.replies[2].agents.map((agent) => agent.id)).toEqual(["project-pin"]);
    expect(result.replies[3]).toMatchObject({ entity_id: "project-owner", agent_id: "project-pin", conversation_id: "conversation-project-pin" });
    if (!supported) {
      expect(result.replies[2]).not.toHaveProperty("pinned_agent_id");
      expect(result.replies[2].agents[0]).not.toHaveProperty("project_role");
      expect(result.forbidden).toEqual(["refused", "refused"]);
    }
  });
}, 60_000);
