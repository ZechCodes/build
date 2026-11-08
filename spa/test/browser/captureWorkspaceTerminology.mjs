// Isolated production creation markup: no bridge session or real sources.
// Run in spa/: node test/browser/captureWorkspaceTerminology.mjs <before|after> <output>
import { mkdir } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import assert from "node:assert/strict";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { navigationAppShim } from "./navigationBreadcrumbSeed.mjs";

const phase = process.argv[2] || "after";
const output = process.argv[3] || "/home/zech/Projects/.agent-scratch/env-m13/workspace-captures";
const creationPath = phase === "before" ? "src/core/createWork.js" : "src/core/createWorkspaceWizard.js";
await mkdir(output, { recursive: true });
const beforeBody = phase === "before"
  ? execFileSync("git", ["show", "dd7c1b064:spa/src/core/createWork.js"], { encoding: "utf8" }) : null;
const baseline = { name: "workspace-terminology-baseline", enforce: "pre", load(id) {
  return beforeBody && id.endsWith("/src/core/createWork.js") ? beforeBody : null;
} };
const session = await openLayoutSession({ plugins: [navigationAppShim, baseline] });
try {
  for (const width of [1280, 390]) for (const theme of ["light", "dark"]) {
    await session.withPage(async ({ page, basePath }) => {
      await mountLayout(page, '<div class="modal-scrim" id="creation"></div>', { basePath });
      await loadBrowserModules(page, { creation: creationPath }, basePath);
      await page.evaluate(({ theme, phase }) => {
        document.documentElement.dataset.theme = theme;
        const creation = window.__layoutModules.creation;
        document.querySelector("#creation").innerHTML = phase === "before"
          ? creation.createWorkHtml({ projectName: "Build", name: "Release", isolation: "", busy: false, error: "" })
          : creation.createWorkspaceWizardHtml({ page: 3, target: { kind: "project", id: "p1", deviceId: "dev-1", name: "Build", label: "Build", sources: [], taskText: "0 tasks" }, name: "Release", isolation: "", error: "" });
      }, { theme, phase });
      assert.equal(await page.getByRole("heading", { name: "New workspace in Build" }).count(), 1);
      if (phase === "before") assert.equal(await page.locator(".create-hint").textContent(), "Copies every source in this project into one durable workspace.");
      assert.equal(await page.getByLabel(phase === "before" ? "Workspace label" : "Name (optional)").inputValue(), "Release");
      assert.equal(await page.getByRole("button", { name: "Create workspace", exact: true }).count(), 1);
      await page.screenshot({ path: join(output, `${phase}-${width}-${theme}.png`) });
    }, { width, height: width === 390 ? 844 : 800 });
  }
} finally { await session.close(); }
console.log(`Workspace terminology ${phase}: four viewport/theme captures and copy/name assertions passed.`);
