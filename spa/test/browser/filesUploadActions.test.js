import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer, row } from "./filesExplorerSeed.mjs";

async function mountActions(page, basePath, touch = false) {
  await mountFilesExplorer(page, basePath);
  await loadBrowserModules(page, { support: "src/core/fileUploadSupport.js" }, basePath);
  await page.evaluate(async () => {
    await window.__layoutModules.support.rememberFileUploadSupport("explorer-device", { fs: { uploads: true, createDirectory: true } });
  });
  if (touch) {
    await page.locator(".pane-handle").tap();
    await page.waitForFunction(() => getComputedStyle(document.querySelector("#ftree")).transform === "none");
  }
  await row(page, "spa").locator('[data-upload-action="upload"]').waitFor();
}

const actionOpacity = (directory) => directory.locator(".fupload-directory-actions").evaluate((node) => getComputedStyle(node).opacity);
const visibleActions = (page) => page.locator(".fupload-directory-actions").evaluateAll((nodes) => nodes.filter((node) => getComputedStyle(node).opacity === "1").length);

async function touchStart(client, directory) {
  // Directory navigation repaints the tree asynchronously. Use Playwright's
  // actionability checks before measuring for the raw CDP touch input.
  await directory.click({ trial: true });
  const point = await directory.evaluate((node) => {
    const box = node.getBoundingClientRect();
    return { x: box.x + 55, y: box.y + box.height / 2, id: 1 };
  });
  await client.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [point] });
  return point;
}

const touchEnd = (client) => client.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });

it("reveals SVG actions on fine-pointer hover and keyboard focus without changing row height", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath);
    expect(await page.evaluate(() => matchMedia("(pointer: fine)").matches)).toBe(true);
    const directory = row(page, "spa");
    expect(await actionOpacity(directory)).toBe("0");
    const before = await directory.boundingBox();
    await directory.hover();
    expect(await actionOpacity(directory)).toBe("1");
    expect(await visibleActions(page)).toBe(1);
    for (const label of ["Upload files", "New folder"]) {
      const control = directory.getByRole("button", { name: label, exact: true });
      expect(await control.locator("svg").count()).toBe(1);
      expect((await control.boundingBox()).height).toBeLessThanOrEqual(before.height);
      expect(await control.locator("svg").evaluate((node) => getComputedStyle(node).width)).toBe("14px");
    }
    expect((await directory.boundingBox()).height).toBe(before.height);
    await captureLayout(page, "files-actions-hover.png");
    await page.mouse.move(900, 700);
    await directory.getByRole("button", { name: "Upload files", exact: true }).focus();
    expect(await actionOpacity(directory)).toBe("1");
  });
}, 60_000);

it("reveals one row after a 600 ms touch hold, prevents contextmenu and preserves normal taps", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath, true);
    expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
    const directory = row(page, "spa");
    expect(await visibleActions(page)).toBe(0);
    expect(await directory.evaluate((node) => getComputedStyle(node).userSelect)).toBe("none");
    const client = await page.context().newCDPSession(page);
    await touchStart(client, directory);
    await page.waitForTimeout(600);
    expect(await actionOpacity(directory)).toBe("1");
    expect(await visibleActions(page)).toBe(1);
    const prevented = await directory.evaluate((node) => {
      const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      node.dispatchEvent(menu);
      return menu.defaultPrevented;
    });
    expect(prevented).toBe(true);
    await touchEnd(client);
    expect(await directory.getAttribute("aria-expanded")).toBe("false");
    await captureLayout(page, "files-actions-long-press.png");
    await row(page, "bridge").tap();
    expect(await visibleActions(page)).toBe(0);
    await touchStart(client, directory);
    await page.waitForTimeout(200);
    await touchEnd(client);
    await expect.poll(() => directory.getAttribute("aria-expanded")).toBe("true");
    expect(await visibleActions(page)).toBe(0);
    await touchStart(client, directory);
    await page.waitForTimeout(600);
    await touchEnd(client);
    await directory.getByRole("button", { name: "New folder", exact: true }).tap();
    expect(await page.getByRole("textbox", { name: "New folder name" }).count()).toBe(1);
    await page.keyboard.press("Escape");
    await page.locator("#toolbar").tap();
    expect(await visibleActions(page)).toBe(0);
    await directory.getByRole("button", { name: "Upload files", exact: true }).focus();
    await page.keyboard.press("Tab");
    expect(await directory.getByRole("button", { name: "New folder", exact: true }).evaluate((node) => node === document.activeElement)).toBe(true);
    expect(await actionOpacity(directory)).toBe("1");
    await client.detach();
  }, { width: 390, height: 760, hasTouch: true });
}, 60_000);

it("cancels a touch reveal after a 12 px movement", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath, true);
    const client = await page.context().newCDPSession(page);
    const directory = row(page, "spa");
    const point = await touchStart(client, directory);
    await page.waitForTimeout(200);
    await client.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints: [{ ...point, x: point.x + 12 }] });
    await page.waitForTimeout(400);
    expect(await visibleActions(page)).toBe(0);
    await touchEnd(client);
    await client.detach();
  }, { width: 390, height: 760, hasTouch: true });
}, 60_000);
