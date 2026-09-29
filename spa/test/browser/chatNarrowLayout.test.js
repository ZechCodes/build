// #212: on a phone the whole conversation scrolled sideways. A task notice
// carrying a long workspace slug was one unbreakable line, so it set the
// column's width and every bubble and paragraph beside it was cut off at the
// left. Measured in Chromium at 390px with the rail's production styles.
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const PHONE = { width: 390, height: 844 };
const LONG_SLUG = "airlock-evaluation-queue-resilience";

/** Runs in the page: the conversation from the phone screenshot, drawn by the
 *  production renderer into the rail's scroller. */
function paintConversation(longSlug) {
  const { threadHtml } = window.__layoutModules.thread;
  const identity = (agentId, workspace, name) => ({
    agent_id: agentId, name, ordinal: 1, workspace_id: `ws-${agentId}`,
    workspace_name: workspace, provider: "claude_adk", available: true,
  });
  const notice = (sequence, number, action, to, actor) => ({
    type: "message",
    data: {
      sequence, role: "user", from_build: true, body: `#${number} ${action}`,
      created_at: "2026-09-28T20:00:00Z",
      from_task: { task_id: `task-${number}`, number, title: "Fix email classification errors" },
      task_notice: { action, to, actor: { kind: "agent", agent_id: actor.agent_id, identity: actor } },
    },
  });
  const hardener = identity("agent-hardener", longSlug, "Airlock queue hardener");
  const heartbeat = identity("agent-heartbeat", "do-stream-consumer-heartbeat", "Consumer heartbeat");
  const scroller = document.querySelector(".rail-body");
  scroller.innerHTML = threadHtml({ id: "narrow-chat", items: [
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
    { type: "message", data: { sequence: 6, role: "user", from_build: true, created_at: "2026-09-28T20:01:00Z",
      body: `Build restarted while ${longSlug}-and-its-sibling-workspace-were-running; assume nothing finished.` } },
    { type: "message", data: { sequence: 7, role: "agent", created_at: "2026-09-28T20:02:00Z", body: "",
      task_action: { action: "assigned", task_id: "task-215", number: 215, title: "Harden the queue",
        assignee: { kind: "agent", agent_id: hardener.agent_id, identity: hardener } } } },
    { type: "message", data: { sequence: 8, role: "agent", created_at: "2026-09-28T20:03:00Z",
      body: `The digest was sha256:${"9f".repeat(40)} and nothing else.` } },
  ] }, { place: { projectId: "proj-1", deviceId: "device-1", projectName: "Build" } });
}

async function mountPhoneChat(page, basePath) {
  await mountLayout(page, '<main class="rail-panel"><div class="rail-body"></div></main>', {
    basePath,
    styles: `@import url("${basePath}src/styles/tasks.css");
      body{display:block;margin:0} .rail-panel{display:flex;flex-direction:column;width:100vw;height:100vh}`,
  });
  await loadBrowserModules(page, { thread: "src/core/thread.js" }, basePath);
  await page.evaluate(paintConversation, LONG_SLUG);
}

it("the conversation fits a 390px phone: nothing scrolls it sideways", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountPhoneChat(page, basePath);
    await captureLayout(page, "chat-narrow-390.png");
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
    expect(measured.notices).toBe(3);
    expect(measured.chips).toBe(2);
    expect(measured.overflowing).toEqual([]);
    expect(measured.scrollWidth).toBeLessThanOrEqual(measured.clientWidth);
  }, PHONE);
}, 30_000);

it("a notice too long for the line breaks between its parts, not inside a name", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountPhoneChat(page, basePath);
    const lines = await page.evaluate(() => {
      const top = (element) => Math.round(element.getBoundingClientRect().top);
      return [...document.querySelectorAll("[data-task-notice]")].map((notice) => {
        const number = notice.querySelector(".thread-task-number");
        const by = notice.querySelector(".thread-task-by");
        const name = by.querySelector(".thread-task-agent-link");
        const slug = [...name.querySelectorAll(".thread-task-name-part")];
        return {
          text: notice.textContent.replace(/\s+/g, " ").trim(),
          numberTop: top(number),
          byTop: top(by),
          // A part that broke inside itself has more than one line box.
          splitParts: slug.filter((part) => part.getClientRects().length > 1).map((part) => part.textContent),
          lineCount: new Set([...notice.querySelectorAll(".thread-task-number, .thread-task-said, .thread-task-by, .thread-task-name-part")]
            .map(top)).size,
        };
      });
    });
    const long = lines.find((line) => line.text.includes("airlock-evaluation-queue-resilience") && line.text.includes("moved"));
    // It no longer fits on one line at 390px, so it wraps rather than overflowing…
    expect(long.lineCount).toBeGreaterThan(1);
    // …and where it wraps is between parts: "by" starts a line of its own…
    expect(long.byTop).toBeGreaterThan(long.numberTop);
    // …and neither the slug nor the agent's name is broken mid-word.
    expect(long.splitParts).toEqual([]);
  }, PHONE);
}, 30_000);
