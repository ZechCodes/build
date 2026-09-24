// The Files tab's explorer (#151) in a real Chromium: the production view
// mounted into a flush tab body over a small checkout, answering fs.* from
// memory and keeping its UI state in the page's own IndexedDB. The layout
// check measures it (filesExplorerLayout.test.js); the capture script draws it
// (captureFilesExplorer.mjs).
import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const SHELL_HTML = `<div id="shell"><div id="view">
  <header id="toolbar">Build / ide-file-list</header>
  <div id="view-body"><nav id="dir-rail"></nav><main id="root" class="surface"><div id="tabbody" class="flush"></div></main></div>
</div></div>`;
export const SHELL_STYLES = "#shell{--inbox-space:0px;height:100vh;box-sizing:border-box} #toolbar{padding:12px 20px}";

/** Runs in the page: `page.evaluate(seedFiles, { theme })`. */
export async function seedFiles({ theme }) {
  document.documentElement.dataset.theme = theme;
  const { renderFilesTab } = window.__layoutModules.files;
  const { scopeFor } = window.__layoutModules.scope;
  const file = (name, size = 400) => ({ name, kind: "file", size });
  const tree = {
    "": [
      { name: ".github", kind: "dir" }, { name: "bridge", kind: "dir" }, { name: "spa", kind: "dir" },
      file("AGENTS.md", 8545), file("ARCHITECTURE.md", 36525), file("README.md", 28022), file("CLAUDE.md", 11),
    ],
    spa: [{ name: "src", kind: "dir" }, { name: "test", kind: "dir" }, file("package.json", 1320), file("vite.config.js", 2210)],
    "spa/src": [
      { name: "core", kind: "dir" }, { name: "views", kind: "dir" },
      file("app.js", 18230), file("main.js", 1502), file("styles.css", 98311),
    ],
  };
  const body = (path) => path.endsWith(".md")
    ? `# ${path}\n\nEverything an agent working in this repository needs to follow.\n\n## Read first\n\nRead ARCHITECTURE.md before changing spa/ or bridge/.\n`
    : `// ${path}\nimport { renderFilesTab } from "./views/files.js";\n\nexport function boot() {\n  return renderFilesTab(document.body, {});\n}\n`;
  const encode = (text) => btoa(String.fromCharCode(...new TextEncoder().encode(text)));
  const callRpc = async (method, params) => {
    if (method === "fs.tree") return { path: params.path, entries: tree[params.path] || [] };
    if (method === "fs.read") {
      const text = body(params.path);
      const mime = params.path.endsWith(".md") ? "text/markdown" : "text/plain";
      return { path: params.path, mime, size: text.length, truncated: false, editable: true, encoding: "utf-8", revision: "r1", content_b64: encode(text) };
    }
    throw new Error(`unexpected ${method}`);
  };
  window.__files = renderFilesTab(document.querySelector("#tabbody"), {
    scope: { run_id: "explorer-run" }, callRpc, cacheScope: scopeFor("explorer-device"),
  });
}

export async function mountFilesExplorer(page, basePath, { theme = "dark" } = {}) {
  await mountLayout(page, SHELL_HTML, { basePath, styles: SHELL_STYLES });
  // A phone lays out at its own width only with the app's viewport tag.
  await page.evaluate(() => {
    const viewport = Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" });
    document.head.prepend(viewport);
  });
  await loadBrowserModules(page, { files: "src/views/files.js", scope: "src/core/cacheScope.js" }, basePath);
  await page.evaluate(seedFiles, { theme });
  await page.waitForSelector('.frow[data-path="spa"]');
}

export const row = (page, path) => page.locator(`.frow[data-path="${path}"]`);

/** Open a file the way this pointer does: a double-click on a fine pointer, a
 *  tap on touch. */
export async function openFile(page, path, { touch = false } = {}) {
  if (touch) await row(page, path).tap();
  else await row(page, path).dblclick();
  await page.waitForFunction((want) => document.querySelector(".fppath")?.textContent === want, path);
}

/** The desktop state the capture shows: two levels expanded, three tabs open,
 *  one of them holding unsaved edits, the keyboard selection on another row. */
export async function stageDesktop(page) {
  await row(page, "spa").click();
  await row(page, "spa/src").click();
  await row(page, "spa/src/app.js").waitFor();
  await openFile(page, "AGENTS.md");
  await openFile(page, "spa/src/app.js");
  await page.locator('[data-file-mode="edit"]').click();
  await page.locator(".file-editor").fill("// spa/src/app.js\nimport { boot } from \"./main.js\";\n// an unsaved edit\n");
  await openFile(page, "spa/src/main.js");
  await row(page, "spa/src/styles.css").click();
  await page.mouse.move(900, 700);
}
