// #212: on a phone the whole conversation scrolled sideways. A task notice
// carrying a long workspace slug was one unbreakable line, so it set the
// column's width and every bubble and paragraph beside it was cut off at the
// left. Measured in Chromium at 390px with the rail's production styles.
//
// #217: the #212 fix let a notice wrap between its parts, and a long one took
// five ragged lines. Now a notice leads with what happened and is ONE line
// when the whole of it fits; when it does not, it stacks all at once — the
// action on the first line, each section indented on a line of its own,
// ellipsised rather than wrapped.
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const PHONE = { width: 390, height: 844 };
const DESKTOP = { width: 1280, height: 844 };
const LONG_SLUG = "airlock-evaluation-queue-resilience";
// #217: the workspace from the maintainer's screenshot, a sentence long.
const LONG_WORKSPACE = "Agents activity panel: show Build-MCP agents too, grouped apart from sub-agents, click opens their chat";

/** Runs in the page: the conversation from the phone screenshot, drawn by the
 *  production paint into the rail's scroller. */
function paintConversation({ longSlug, longWorkspace }) {
  const { paintThreadEntries, timelineEntries } = window.__layoutModules.thread;
  const identity = (agentId, workspace, name) => ({
    agent_id: agentId, name, ordinal: 1, workspace_id: `ws-${agentId}`,
    workspace_name: workspace, provider: "claude_adk", available: true,
  });
  // No identity is the reader themself.
  const notice = (sequence, number, action, to, actor = null, assignee = null) => ({
    type: "message",
    data: {
      sequence, role: "user", from_build: true, body: `#${number} ${action}`,
      created_at: "2026-09-28T20:00:00Z",
      from_task: { task_id: `task-${number}`, number, title: "Fix email classification errors" },
      task_notice: {
        action, to, actor: actor ? { kind: "agent", agent_id: actor.agent_id, identity: actor } : { kind: "user" },
        ...(assignee ? { assignee: { kind: "agent", agent_id: assignee.agent_id }, assignee_identity: assignee } : {}),
      },
    },
  });
  const hardener = identity("agent-hardener", longSlug, "Airlock queue hardener");
  const heartbeat = identity("agent-heartbeat", "do-stream-consumer-heartbeat", "Consumer heartbeat");
  const panel = identity("agent-panel", longWorkspace, "Activity panel agents");
  const scroller = document.querySelector(".rail-body");
  const items = [
    { type: "message", data: { sequence: 1, role: "agent", created_at: "2026-09-28T19:00:00Z",
      body: `Your call: merge and roll out the Airlock fix, and whether to re-screen the emails screened since Sept 23. The branch is ${longSlug}/queue-hardening-follow-up-with-retries and the log said https://example.com/a/very/long/path/that/never/stops/anywhere/at/all/${longSlug}.\n\n\`\`\`\nairlock evaluate --queue ${longSlug} --retries 5 --backoff exponential --deadline 30s\n\`\`\`\n\n| workspace | commit | state |\n| --- | --- | --- |\n| ${longSlug} | 2e9038d2e2de4b1c | merged and deployed |` } },
    { type: "message", data: { sequence: 2, role: "user", created_at: "2026-09-28T19:05:00Z",
      body: "#1 no re-screen",
      viewing_context: { items: [
        { kind: "workspace", name: longSlug },
        { kind: "commit", sha: "2e9038d2e2de4b1c9a7f" },
      ] } } },
    notice(3, 212, "moved", "in_review", heartbeat),
    notice(4, 213, "moved", "in_progress", hardener),
    notice(5, 214, "commented on", null, hardener),
    notice(6, 216, "commented on", null, panel),
    notice(7, 218, "commented on"),
    { type: "message", data: { sequence: 8, role: "user", from_build: true, created_at: "2026-09-28T20:01:00Z",
      body: `Build restarted while ${longSlug}-and-its-sibling-workspace-were-running; assume nothing finished.` } },
    { type: "message", data: { sequence: 9, role: "agent", created_at: "2026-09-28T20:02:00Z", body: "",
      task_action: { action: "assigned", task_id: "task-215", number: 215, title: "Harden the queue",
        assignee: { kind: "agent", agent_id: hardener.agent_id, identity: hardener } } } },
    { type: "message", data: { sequence: 10, role: "agent", created_at: "2026-09-28T20:03:00Z",
      body: `The digest was sha256:${"9f".repeat(40)} and nothing else.` } },
    notice(11, 219, "assigned", null, heartbeat, hardener),
  ];
  const place = { projectId: "proj-1", deviceId: "device-1", projectName: "Build" };
  // A poll that resolved the same conversation paints it again from scratch.
  window.__repaintConversation = () =>
    paintThreadEntries(scroller, timelineEntries(items, "Claude", "narrow-chat", [], { place }), { place });
  window.__repaintConversation();
}

async function mountChat(page, basePath) {
  await mountLayout(page, '<main class="rail-panel"><div class="rail-body"></div></main>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css");
      body{display:block;margin:0} .rail-panel{display:flex;flex-direction:column;width:100vw;height:100vh}`,
  });
  await loadBrowserModules(page, { thread: "src/core/thread.js" }, basePath);
  await page.evaluate(paintConversation, { longSlug: LONG_SLUG, longWorkspace: LONG_WORKSPACE });
}

it("the conversation fits a 390px phone: nothing scrolls it sideways", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    await captureLayout(page, "chat-narrow-390.png");
    await page.evaluate(() => { const scroller = document.querySelector(".rail-body"); scroller.scrollTop = scroller.scrollHeight; });
    await captureLayout(page, "chat-narrow-390-end.png");
    const measured = await page.evaluate(() => {
      const scroller = document.querySelector(".rail-body");
      const right = scroller.getBoundingClientRect().right;
      const overflowing = [...scroller.querySelectorAll(
        ".thread-message, .thread-task-notice, .thread-task-action, .thread-task-who, .viewing-context-chip, .thread-body > p",
      )].filter((element) => element.getBoundingClientRect().right > right + 0.5)
        .map((element) => element.className || element.tagName);
      return { scrollWidth: scroller.scrollWidth, clientWidth: scroller.clientWidth, overflowing,
        notices: scroller.querySelectorAll("[data-task-notice]").length,
        chips: scroller.querySelectorAll(".viewing-context-chip").length };
    });
    expect(measured.notices).toBe(6);
    expect(measured.chips).toBe(2);
    expect(measured.overflowing).toEqual([]);
    expect(measured.scrollWidth).toBeLessThanOrEqual(measured.clientWidth);
  }, PHONE);
}, 30_000);

/** Runs in the page: each notice as the lines it stands on — the action, then
 *  whichever sections sit below it — and what its name chips show. */
function measureNotices() {
  const box = (element) => element.getBoundingClientRect();
  const one = (element, lineHeight) => element.getClientRects().length === 1 && box(element).height <= lineHeight + 1;
  return [...document.querySelectorAll("[data-task-notice]")].map((notice) => {
    const lineHeight = parseFloat(getComputedStyle(notice).lineHeight);
    const head = notice.querySelector(".thread-task-first-line");
    const said = head.querySelector(".thread-task-said");
    const number = head.querySelector(".thread-task-number");
    const sections = [...notice.querySelectorAll(":scope > .thread-task-section")];
    const by = notice.querySelector(".thread-task-by");
    const to = notice.querySelector(".thread-task-to");
    const agent = by?.querySelector(".thread-task-agent-name");
    const workspace = by?.querySelector(".thread-task-workspace");
    return {
      text: notice.textContent.replace(/\s+/g, " ").trim(),
      stacked: notice.dataset.fit === "stacked",
      lines: Math.round(box(notice).height / lineHeight),
      // The first line is the action with the task it happened to (#217, the
      // maintainer: "keep the issue number on the same line as the action").
      leadsWithAction: notice.firstElementChild === head && head.firstElementChild === said,
      head: head.textContent.replace(/\s+/g, " ").trim(),
      numberBesideAction: Math.abs(box(number).top - box(said).top) < 2,
      sections: sections.length,
      // Stacked, every section starts a line of its own below the action and
      // is indented under it; nothing inside a section wraps.
      ownLines: sections.every((section, index) =>
        box(section).top >= box(index ? sections[index - 1] : head).bottom - 1),
      indented: sections.every((section) => box(section).left > box(head).left),
      unwrapped: [head, ...sections].every((section) => one(section, lineHeight * 2)),
      agentLine: agent && { shown: agent.scrollWidth <= agent.clientWidth, withBy: Math.abs(box(agent).top - box(by).top) < 2 },
      workspace: workspace && {
        title: workspace.getAttribute("title"),
        ellipsised: workspace.scrollWidth > workspace.clientWidth,
        oneLine: one(workspace, lineHeight),
      },
      // One link per name, drawing no underline at rest, as the chat's other
      // links do.
      // Under "to" and under "by" a workspace starts at the same x.
      workspaceLefts: [to, by].map((section) => section?.querySelector(".thread-task-workspace"))
        .filter(Boolean).map((workspace) => Math.round(box(workspace).left)),
      nameLinks: by ? by.querySelectorAll("a").length : 0,
      underlined: [...notice.querySelectorAll("a")]
        .filter((link) => getComputedStyle(link).textDecorationLine !== "none").length,
    };
  });
}

it("on a phone a notice that does not fit stacks: the action, then each section on its own line", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    const notices = await page.evaluate(measureNotices);
    for (const notice of notices) {
      expect(notice.leadsWithAction, notice.text).toBe(true);
      expect(notice.numberBesideAction, notice.text).toBe(true);
      expect(notice.underlined, notice.text).toBe(0);
      // All or nothing: one line, or the action and every section stacked.
      if (!notice.stacked) expect(notice.lines, notice.text).toBe(1);
    }
    const long = notices.find((notice) => notice.text.includes("Activity panel agents"));
    expect(long.text).toMatch(/^Commented on #216 by /);
    expect(long.stacked).toBe(true);
    expect(long).toMatchObject({ ownLines: true, indented: true, unwrapped: true, nameLinks: 1 });
    // The actor's line is "by" and the agent, whole; the workspace sits under
    // it on one line of its own, ellipsised, its whole name as hover text.
    expect(long.agentLine).toEqual({ shown: true, withBy: true });
    expect(long.workspace).toEqual({ title: LONG_WORKSPACE, ellipsised: true, oneLine: true });
    expect(long.head).toBe("Commented on #216");
    expect(long.lines).toBe(long.sections + 2);
    // A move's column is part of what happened, so it rides the first line.
    const moved = notices.find((notice) => notice.text.includes("#213"));
    expect(moved).toMatchObject({ stacked: true, head: "Moved #213 to In progress" });
    // An assignee is a name like the actor: its own section, and its
    // workspace lines up with the actor's.
    const assigned = notices.find((notice) => notice.text.includes("#219"));
    expect(assigned).toMatchObject({ stacked: true, head: "Assigned #219", sections: 2, ownLines: true });
    expect(assigned.workspaceLefts).toHaveLength(2);
    expect(assigned.workspaceLefts[0]).toBe(assigned.workspaceLefts[1]);
    const short = notices.find((notice) => notice.text.includes("#218"));
    expect(short.text).toBe("Commented on #218 by You");
    expect(short).toMatchObject({ stacked: false, lines: 1 });
  }, PHONE);
}, 30_000);

it("at desktop width every notice is one line", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    await captureLayout(page, "chat-desktop-1280.png");
    const notices = await page.evaluate(measureNotices);
    expect(notices).toHaveLength(6);
    for (const notice of notices) {
      expect(notice.stacked, notice.text).toBe(false);
      expect(notice.lines, notice.text).toBe(1);
    }
  }, DESKTOP);
}, 30_000);

it("narrowing the chat restacks the notices that no longer fit, and widening it unstacks them", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    const stackedAt = async (width) => {
      await page.setViewportSize({ width, height: DESKTOP.height });
      await page.evaluate(() => new Promise((settle) => requestAnimationFrame(() => requestAnimationFrame(settle))));
      return page.evaluate(() => [...document.querySelectorAll('[data-task-notice][data-fit="stacked"]')]
        .map((notice) => notice.dataset.taskNotice));
    };
    expect(await stackedAt(DESKTOP.width)).toEqual([]);
    expect(await stackedAt(PHONE.width)).toContain("task-216");
    expect(await stackedAt(DESKTOP.width)).toEqual([]);
  }, DESKTOP);
}, 30_000);

it("a repaint of the same conversation changes no node, stacked notices included", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    const mutations = await page.evaluate(() => {
      const scroller = document.querySelector(".rail-body");
      const records = [];
      const observer = new MutationObserver((batch) => records.push(...batch));
      observer.observe(scroller, { subtree: true, childList: true, attributes: true, characterData: true });
      window.__repaintConversation();
      window.__repaintConversation();
      records.push(...observer.takeRecords());
      observer.disconnect();
      return records.map((record) => `${record.type} ${record.attributeName || ""} ${record.target.className || record.target.nodeName}`);
    });
    expect(await page.locator('[data-task-notice][data-fit="stacked"]').count()).toBeGreaterThan(0);
    expect(mutations).toEqual([]);
  }, PHONE);
}, 30_000);

it("Build's own notices still wrap: only task notices are fitted to one line", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    const restart = await page.evaluate(() => {
      const line = [...document.querySelectorAll(".thread-task-notice:not([data-task-notice])")]
        .find((element) => element.textContent.includes("Build restarted"));
      const lineHeight = parseFloat(getComputedStyle(line).lineHeight);
      return { lines: Math.round(line.getBoundingClientRect().height / lineHeight),
        clipped: line.scrollWidth > line.clientWidth };
    });
    expect(restart.lines).toBeGreaterThan(1);
    expect(restart.clipped).toBe(false);
  }, PHONE);
}, 30_000);
