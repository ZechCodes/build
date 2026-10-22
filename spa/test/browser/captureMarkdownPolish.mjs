import { mkdir } from "node:fs/promises";
import { resolve, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { openLayoutSession, mountLayout, loadBrowserModules } from "./layoutHarness.mjs";
import { MARKDOWN_POLISH_FIXTURE, FIXTURE_IMAGE, FIXTURE_STYLES, fixtureMarkup } from "./markdownPolishFixture.mjs";

const directory = resolve(process.argv[2] || "/tmp/build-markdown-695");
const phase = process.argv[3] || "after";
await mkdir(directory, { recursive: true });
// A supplied commit captures the original renderer and stylesheet through
// Vite without checking out, reverting or copying the working repository.
const sourceRef = process.argv[4];
const spaRoot = fileURLToPath(new URL("../../", import.meta.url));
const baselineFiles = new Set(["src/core/markdown.js", "src/core/markdownBlocks.js", "src/core/markdownWebLinks.js", "src/styles.css"]);
const plugins = sourceRef ? [{ name: "markdown-before", enforce: "pre", load(id) {
  const path = relative(spaRoot, id.split("?")[0]);
  if (baselineFiles.has(path)) return execFileSync("git", ["show", `${sourceRef}:spa/${path}`], { cwd: spaRoot, encoding: "utf8" });
} }] : [];
const session = await openLayoutSession({ plugins });
try {
  for (const width of [960, 390, 320]) {
    for (const theme of ["light", "dark"]) {
      await session.withPage(async ({ page, basePath }) => {
        await page.route("https://markdown-fixture.test/**", (route) => route.fulfill({ contentType: "image/svg+xml", body: FIXTURE_IMAGE }));
        await mountLayout(page, fixtureMarkup(""), { basePath, styles: FIXTURE_STYLES });
        await loadBrowserModules(page, { markdown: "src/core/markdown.js" }, basePath);
        await page.evaluate((source) => {
          document.querySelector(".thread-body").innerHTML = window.__layoutModules.markdown.markdownHtml(source, { taskLabelScope: "markdown-polish" });
        }, MARKDOWN_POLISH_FIXTURE);
        await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
        await page.evaluate(async () => {
          for (const image of document.images) { image.loading = "eager"; await image.decode().catch(() => {}); }
          window.scrollTo(0, 0);
          await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
        });
        await page.screenshot({ path: resolve(directory, `${phase}-${width === 960 ? "desktop" : width === 390 ? "phone" : "narrow"}-${theme}.png`), fullPage: true });
      }, { width, height: 900 });
    }
  }
} finally { await session.close(); }
console.log(`Saved ${phase} markdown captures to ${directory}`);
