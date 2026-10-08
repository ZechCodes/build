import { readFileSync } from "node:fs";
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

// Keep the real shell rather than recreating its controls for the screenshot.
const index = readFileSync(new URL("../../index.html", import.meta.url), "utf8");
const shell = index.match(/<body>([\s\S]*?)<\/body>/)[1];

export const HEADER_TEXT = "#397 SPA interaction polish";
export const COMMENT_TEXT = "Comments remain selectable so useful details can be copied.";

export async function mountInteractionFixture(page, basePath, { theme = "light", extras = false } = {}) {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mountLayout(page, shell, { basePath, styles: `@import url("${basePath}src/styles/tasks.css");
    @import url("${basePath}src/styles/surfaces.css");
    #root{display:flex;flex-direction:column;min-height:0} #tabbody{overflow:auto}
    #interaction-extras{padding:16px} #interaction-terminal{width:100%;height:220px}` });
  await loadBrowserModules(page, {
    toolbar: "src/core/toolbarRender.js", task: "src/core/trackerTaskRender.js",
    tabs: "src/core/tabshell.js", rows: "src/core/trackerListRender.js",
    board: "src/core/trackerBoardRender.js", diff: "src/core/diffRender.js",
  }, basePath);
  await page.evaluate(({ theme, extras, header, comment }) => {
    document.documentElement.dataset.theme = theme;
    document.body.classList.add("inbox-collapsed");
    const m = window.__layoutModules;
    const task = { id: "task-397", number: 397, title: "Make Build feel like an app", state: "open", status: "in_progress", priority: "medium", labels: ["ui"], updated_at: "2026-10-07T20:00:00Z", body: "Keep the navigation quiet while preserving readable content.\n\nCopy task descriptions, comments and code without accidentally selecting the app controls.", attachments: [] };
    if (extras) task.body += "\n\n| Area | Expected |\n| --- | --- |\n| Content | Selectable |";
    const context = { deviceId: "interaction-device", projectId: "proj-1", columns: [{ id: "in_progress", name: "In progress" }], links: [], labelsDraft: "ui", rows: [{ type: "comment", key: "comment-397", actor: { kind: "user", name: "You" }, body: `${comment}\n\nThe toolbar, tabs and status labels should behave like application controls.`, at: "2026-10-07T20:05:00Z", attachments: [] }], draft: "", sending: false, busy: false, identities: {} };
    if (extras) context.rows[0].body += "\n\nRead [the docs](https://docs.example.test/interaction).";
    document.querySelector("#toolbar").innerHTML = m.toolbar.toolbarHtml({ project: "Build", kind: "task", label: header });
    const root = document.querySelector("#root");
    root.classList.add("surface");
    const tabs = document.createElement("div");
    root.appendChild(tabs);
    window.__interactionTab = "task";
    window.__interactionTabs = m.tabs.mountTabShell(tabs, { tabs: [{ id: "task", label: "Task" }, { id: "changes", label: "Changes" }], active: "task", onSelect: (id) => { window.__interactionTab = id; } });
    const content = document.createElement("div");
    content.id = "tabbody";
    content.className = "task-surface";
    content.innerHTML = m.task.taskPageHtml(task, context);
    if (extras) {
      const value = document.createElement("span");
      value.className = "v";
      value.innerHTML = "<span>Copy this diagnostic value</span>";
      document.querySelector(".toolbar").appendChild(value);
      const action = document.createElement("button");
      action.className = "btn";
      action.dataset.commentAction = "";
      action.textContent = "Comment action";
      content.querySelector(".task-comment-body").appendChild(action);
    }
    root.appendChild(content);
    if (!extras) return;
    const extra = document.createElement("section");
    extra.id = "interaction-extras";
    const rowContext = { ...context, href: () => "#task-397" };
    extra.innerHTML = `<ul class="task-rows">${m.rows.taskRowHtml(task, rowContext)}</ul>
      <div role="button" tabindex="0" data-error-row><span class="error"><span>Upload refused</span></span></div>
      <div class="card archive-row" tabindex="0">Archived task</div>
      <div class="crow ahead" tabindex="0">Ahead commit</div>
      <div class="crow unpushed" tabindex="0">Unpushed commit</div>
      <div class="crow sel" tabindex="0">Selected commit</div>
      ${m.board.boardFrameHtml([{ id: "in_progress", name: "In progress", tasks: [task] }], rowContext)}
      ${m.diff.diffFileHtml({ path: "src/interaction.js", status: "M", add: 1, del: 0, rows: [{ t: "add", n: 1, text: "const copyable = true;" }] }, { fold: "open" })}
      <pre class="bridge-update-command">curl https://build.example/install | sh</pre>
      ${m.task.taskBodyHtml({ ...task, body: "- [ ] Verify native controls" }, context)}
      <div class="markdown"><label for="interaction-checkbox"><input id="interaction-checkbox" type="checkbox"> Include archived tasks</label></div>
      <input type="hidden" value="saved scope">
      <input id="interaction-readonly" readonly value="Saved diagnostic text">
      <div id="interaction-editor" contenteditable="true" role="textbox">Editable native content</div>
      <div id="interaction-terminal" class="termpane"></div>`;
    extra.querySelector(".task-column-cards").innerHTML = m.board.taskCardHtml(task, rowContext);
    content.appendChild(extra);
  }, { theme, extras, header: HEADER_TEXT, comment: COMMENT_TEXT });
  await page.evaluate(() => document.fonts.ready);
}

/** A trusted mouse drag, which CSS can block; a programmatic Range cannot. */
export async function dragInteractionText(page, selector, { clickCount = 1 } = {}) {
  const element = page.locator(selector).first();
  await element.scrollIntoViewIfNeeded();
  await page.evaluate(() => window.getSelection().removeAllRanges());
  const box = await element.evaluate((node) => {
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
    let text;
    do { text = walker.nextNode(); } while (text && !text.textContent.trim());
    if (!text) throw new Error("No text to drag");
    const range = document.createRange();
    range.selectNodeContents(text);
    const first = range.getClientRects()[0];
    return { x: first.x, y: first.y, width: first.width, height: first.height };
  });
  await page.mouse.move(box.x + 1, box.y + box.height / 2);
  if (clickCount === 3) await page.mouse.click(box.x + 1, box.y + box.height / 2, { clickCount: 2 });
  await page.mouse.down({ clickCount });
  await page.mouse.move(box.x + box.width - 1, box.y + box.height / 2, { steps: 16 });
  await page.mouse.up({ clickCount });
  return page.evaluate(() => window.getSelection().toString());
}
