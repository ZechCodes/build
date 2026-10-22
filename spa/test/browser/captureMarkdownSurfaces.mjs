import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountAgentMarkdown, mountTaskMarkdown, mountPlanMarkdown } from "./markdownSurfacesFixture.mjs";

const directory = resolve(process.argv[2] || "/tmp/build-markdown-695-review");
await mkdir(directory, { recursive: true });
const session = await openLayoutSession({ plugins: [deviceShim] });
try {
  for (const theme of ["light", "dark"]) {
    for (const [name, width, mount] of [
      ["other-agent-phone", 390, mountAgentMarkdown],
      ["task-comment", 960, mountTaskMarkdown],
      ["plan-document", 1180, mountPlanMarkdown],
    ]) {
      await session.withPage(async ({ page, basePath }) => {
        await mount(page, basePath, theme);
        const metrics = await page.evaluate(() => ({
          headings: [...document.querySelectorAll(".thread-body :is(h1,h2,h3), .task-comment-body :is(h1,h2,h3), .plan :is(h1,h2,h3,h4,h5,h6)")].map((el) => ({
            tag: el.tagName, fontSize: getComputedStyle(el).fontSize,
          })),
          tables: [...document.querySelectorAll(".mdtable")].map((el) => ({
            scrollWidth: el.scrollWidth, clientWidth: el.clientWidth, tabindex: el.getAttribute("tabindex"),
          })),
        }));
        console.log(name, theme, JSON.stringify(metrics));
        await page.screenshot({ path: resolve(directory, `${name}-${theme}.png`), fullPage: name === "other-agent-phone" });
        await page.evaluate(() => {
          window.__markdownPage?.dispose();
          window.__markdownFiles?.dispose();
        });
      }, { width, height: 1000 });
    }
  }
} finally { await session.close(); }
