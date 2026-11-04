import { readFile } from "node:fs/promises";
import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

async function mountActions(page, basePath) {
  await mountLayout(page, `<button id="outside">Outside</button><div id="tree"><div data-root="code">
    <div class="frow" data-kind="dir" data-path="docs" tabindex="0"><span class="fname">docs</span></div>
    <div class="frow" data-kind="file" data-path="guide.md" tabindex="0"><span class="fname">guide.md</span></div>
  </div></div>`, { basePath, styles: "#tree{margin:24px;width:250px}.frow{min-height:40px}" });
  await loadBrowserModules(page, { actions: "src/core/filesUploadActions.js" }, basePath);
  await page.evaluate(() => {
    window.__calls = [];
    window.__uploads = [];
    window.__created = [];
    window.__actions = window.__layoutModules.actions.mountFilesUploadActions(document.querySelector("#tree"), {
      roots: [{ id: "code", label: "Code", scope: { workspace_id: "workspace", source_id: "code" } }],
      capabilities: { uploads: true, createDirectory: true },
      uploads: { enqueue: (value) => window.__uploads.push({ parent: value.parent, scope: value.scope, names: value.files.map((file) => file.name) }) },
      callRpc: async (method, params) => {
        window.__calls.push([method, params]);
        if (method === "fs.read") return { content_b64: btoa("complete downloaded file"), mime: "text/plain", truncated: false };
        return { path: `${params.parent}/${params.name}` };
      },
      onCreated: (...args) => window.__created.push(args),
    });
  });
}

it("opens file and directory context menus and reuses scoped upload and mkdir actions", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath);
    const file = page.locator('[data-path="guide.md"]');
    const directory = page.locator('[data-path="docs"]');
    await file.click({ button: "right" });
    await expect.poll(() => page.getByRole("menuitem", { name: "Download", exact: true }).isVisible()).toBe(true);
    const saved = page.waitForEvent("download");
    await page.getByRole("menuitem", { name: "Download", exact: true }).click();
    const download = await saved;
    expect(download.suggestedFilename()).toBe("guide.md");
    expect(await readFile(await download.path(), "utf8")).toBe("complete downloaded file");
    expect(await page.evaluate(() => window.__calls)).toEqual([["fs.read", { workspace_id: "workspace", source_id: "code", path: "guide.md" }]]);
    await page.evaluate(() => { window.__calls = []; });
    await file.click({ button: "right" });
    await page.keyboard.press("Escape");
    expect(await page.getByRole("menu").count()).toBe(0);
    await file.focus();
    await page.keyboard.press("Shift+F10");
    await expect.poll(() => page.getByRole("menuitem", { name: "Download", exact: true }).isVisible()).toBe(true);
    await page.locator("#outside").click();
    expect(await page.getByRole("menu").count()).toBe(0);

    await directory.click({ button: "right" });
    await captureLayout(page, "files-directory-context-menu.png");
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("menuitem", { name: "Upload files here", exact: true }).click();
    await (await chooser).setFiles({ name: "hello.txt", mimeType: "text/plain", buffer: Buffer.from("hello") });
    expect(await page.evaluate(() => window.__uploads)).toEqual([{ parent: "docs", scope: { workspace_id: "workspace", source_id: "code" }, names: ["hello.txt"] }]);

    await directory.click({ button: "right" });
    await page.getByRole("menuitem", { name: "New directory", exact: true }).click();
    const field = page.getByRole("textbox", { name: "New folder name" });
    await field.fill("drafts");
    await field.press("Enter");
    await expect.poll(() => page.evaluate(() => window.__calls)).toEqual([["fs.createDirectory", { workspace_id: "workspace", source_id: "code", parent: "docs", name: "drafts" }]]);
    await expect.poll(() => page.evaluate(() => window.__created.length)).toBe(1);
  });
}, 60_000);

it("opens Download after a phone long press, consumes the hold click, and fits the viewport", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath);
    await page.evaluate(() => {
      window.__rowClicks = 0;
      document.querySelector('[data-path="guide.md"]').addEventListener("click", () => { window.__rowClicks += 1; });
    });
    const file = page.locator('[data-path="guide.md"]');
    const box = await file.boundingBox();
    const client = await page.context().newCDPSession(page);
    const point = { x: box.x + 20, y: box.y + box.height / 2 };
    await client.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [point] });
    await expect.poll(() => page.getByRole("menuitem", { name: "Download", exact: true }).isVisible()).toBe(true);
    await client.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    expect(await page.evaluate(() => window.__rowClicks)).toBe(0);
    const menu = await page.getByRole("menu").boundingBox();
    expect(menu.x).toBeGreaterThanOrEqual(0);
    expect(menu.y).toBeGreaterThanOrEqual(0);
    expect(menu.x + menu.width).toBeLessThanOrEqual(320);
    expect(menu.y + menu.height).toBeLessThanOrEqual(640);
    await captureLayout(page, "files-phone-context-menu.png");
    await page.locator("#outside").tap();
    expect(await page.getByRole("menu").count()).toBe(0);
    await client.detach();
  }, { width: 320, height: 640, hasTouch: true });
}, 60_000);
