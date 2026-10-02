// #212: on a phone the whole conversation scrolled sideways. A task notice
// carrying a long workspace slug was one unbreakable line, so it set the
// column's width and every bubble and paragraph beside it was cut off at the
// left. Measured in Chromium at 390px with the rail's production styles.
//
// #323: #217 stacked a notice too long for its line — the action, then "by"
// and the agent, then the workspace — and the maintainer found three lines a
// notice bad looking. Now every task notice and every action line is exactly
// ONE line: it never wraps, and one too long for the width ends in an
// ellipsis. The workspace is not on the line at all.
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
  const acted = (sequence, number, action, over = {}) => ({
    type: "message",
    data: { sequence, role: "agent", created_at: "2026-09-28T20:04:00Z", body: "",
      task_action: { action, task_id: `task-${number}`, number, title: "Fix email classification errors", ...over } },
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
    // #323: this conversation's own agent acting, in its own voice.
    acted(12, 320, "moved", { to: "done" }),
    acted(13, 320, "linked"),
    acted(14, 321, "commented_on", { comment_id: "tc-321" }),
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
        // A task line's own parts run past its end on purpose: the line
        // clips them with an ellipsis (#323), so the line is what is measured.
        ".thread-message, .thread-task-notice, .thread-task-action, .viewing-context-chip, .thread-body > p",
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

/** Runs in the page: every task line — the notices from elsewhere and this
 *  agent's own action lines — as the lines it stands on and what it shows. */
function measureTaskLines() {
  const box = (element) => element.getBoundingClientRect();
  return [...document.querySelectorAll("[data-task-notice], .thread-task-action[data-task-action]")].map((line) => {
    const style = getComputedStyle(line);
    const lineHeight = parseFloat(style.lineHeight);
    const number = line.querySelector(".thread-task-number");
    const marks = [...line.querySelectorAll(".thread-task-actor-mark")];
    return {
      text: line.textContent.replace(/\s+/g, " ").trim(),
      notice: line.hasAttribute("data-task-notice"),
      oneLine: box(line).height <= lineHeight + 1,
      // Nothing inside the line wraps onto a second one either: every part
      // sits inside the line's own band.
      partsOnOneLine: [...line.querySelectorAll("*")].every((part) => [...part.getClientRects()]
        .every((rect) => rect.top >= box(line).top - 1 && rect.bottom <= box(line).bottom + 1)),
      ellipsis: style.textOverflow === "ellipsis" && style.whiteSpace === "nowrap" && style.overflow === "hidden",
      clipped: line.scrollWidth > line.clientWidth,
      withinRow: box(line).right <= box(line.closest(".thread-message")).right + 0.5,
      // The number is the styled task link it always was.
      numberStyled: getComputedStyle(number).fontWeight === "650",
      marksBesideNumber: marks.every((mark) => Math.abs(box(mark).top + box(mark).height / 2 - (box(number).top + box(number).height / 2)) < 6),
      workspaces: line.querySelectorAll(".thread-task-workspace").length,
      underlined: [...line.querySelectorAll("a")]
        .filter((link) => getComputedStyle(link).textDecorationLine !== "none").length,
    };
  });
}

const OWN_LINES = ["Moved #320 to “Done”", "Linked #320", "Commented on #321"];

it("on a phone every task line is one line, a long one ending in an ellipsis", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    const lines = await page.evaluate(measureTaskLines);
    expect(lines).toHaveLength(10);
    for (const line of lines) {
      expect(line, line.text).toMatchObject({
        oneLine: true, partsOnOneLine: true, ellipsis: true, withinRow: true,
        numberStyled: true, workspaces: 0, underlined: 0,
      });
    }
    const said = (number) => lines.find((line) => line.text.includes(`#${number} `) || line.text.endsWith(`#${number}`)).text;
    // #323's wording: the number leads a notice from elsewhere, and the
    // agent's own name stands for it, without its workspace.
    expect(said(212)).toBe("#212 moved to “In review” by Consumer heartbeat");
    expect(said(214)).toBe("#214 comment from Airlock queue hardener");
    expect(said(218)).toBe("#218 comment from you");
    expect(lines.map((line) => line.text).join("\n")).not.toContain("do-stream-consumer-heartbeat");
    // This conversation's own agent leads with its verb.
    expect(lines.filter((line) => !line.notice).map((line) => line.text)).toEqual(
      expect.arrayContaining(OWN_LINES),
    );
    // The harness mark rides the line beside the name it marks.
    for (const line of lines.filter((one) => one.notice)) expect(line.marksBesideNumber, line.text).toBe(true);
    // The long one is cut, not wrapped: its whole name is longer than a phone.
    const long = lines.find((line) => line.text.startsWith("#219"));
    expect(long.text).toBe("#219 assigned to Airlock queue hardener by Consumer heartbeat");
    expect(long.clipped).toBe(true);
  }, PHONE);
}, 30_000);

it("at desktop width every task line is one line and none is cut", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChat(page, basePath);
    await captureLayout(page, "chat-desktop-1280.png");
    const lines = await page.evaluate(measureTaskLines);
    expect(lines).toHaveLength(10);
    for (const line of lines) {
      expect(line.oneLine, line.text).toBe(true);
      expect(line.clipped, line.text).toBe(false);
    }
  }, DESKTOP);
}, 30_000);

it("a repaint of the same conversation changes no node", async () => {
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
    expect(mutations).toEqual([]);
  }, PHONE);
}, 30_000);

it("Build's own notices still wrap: only task notices are held to one line", async () => {
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
