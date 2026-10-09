import { expect, it } from "vitest";
import { captureLayout, loadBrowserModules, withLayoutPage } from "./layoutHarness.mjs";
import { mountFilesExplorer, row } from "./filesExplorerSeed.mjs";

async function mountActions(page, basePath, touch = false, fs = { uploads: true, createDirectory: true }, beforeSeed = undefined) {
  await mountFilesExplorer(page, basePath, { beforeSeed });
  await loadBrowserModules(page, { support: "src/core/fileUploadSupport.js" }, basePath);
  await page.evaluate(async (support) => {
    await window.__layoutModules.support.rememberFileUploadSupport("explorer-device", { fs: support });
  }, fs);
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

const NEW_MENU = { uploads: true, createDirectory: true, createFile: true };

/** A 3.16 bridge greets the explorer's device before Files mounts, so writes
 *  the cached support offers are also allowed by the live greeting
 *  (core/fileUploadRpc.js). */
async function greetCreatingBridge(page, basePath) {
  await loadBrowserModules(page, {
    contexts: "src/core/deviceContexts.js", adapter: "src/core/bridgeApi/v1/index.js", changes: "src/core/changeEvents.js",
  }, basePath);
  await page.evaluate(async () => {
    const { contexts, adapter, changes } = window.__layoutModules;
    const greeting = { api_version: "3.16.0", capabilities: ["fs.uploadBegin", "fs.createDirectory", "fs.createFile"] };
    const call = async (method) => (method === "session.hello" ? greeting : {});
    const context = contexts.adoptDeviceSession({ deviceId: "explorer-device", call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    contexts.adoptBridgeSelection(context, { version: greeting.api_version }, adapter.create(call, greeting));
    await changes.greetBridge(call, { deviceId: "explorer-device" });
  });
}
const newOpener = (directory) => directory.getByRole("button", { name: "New file or folder", exact: true });
const newMenu = (page) => page.getByRole("menu", { name: "New file or folder" });
const menuLabels = (page) => newMenu(page).getByRole("menuitem").allTextContents();
const menuOpen = async (page) => (await newMenu(page).count()) === 1 && newMenu(page).isVisible();
const menuSettled = (page) => newMenu(page).evaluate((menu) => getComputedStyle(menu).opacity === "1" && menu.getAnimations().length === 0);
const insideViewport = (page) => newMenu(page).evaluate((menu) => {
  const box = menu.getBoundingClientRect();
  return box.top >= 0 && box.left >= 0 && box.bottom <= innerHeight && box.right <= innerWidth && box.height > 0;
});

it("opens New file / New folder from the folder button on desktop, closes on Escape and outside, and creates a file", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath, false, NEW_MENU, () => greetCreatingBridge(page, basePath));
    const directory = row(page, "spa");
    await newOpener(directory).waitFor({ state: "attached" });
    expect(await directory.getByRole("button", { name: "New folder", exact: true }).count()).toBe(0);
    const before = await directory.boundingBox();
    await directory.hover();
    expect(await newOpener(directory).locator("svg").count()).toBe(1);
    await newOpener(directory).click();
    await expect.poll(() => menuOpen(page)).toBe(true);
    expect(await menuLabels(page)).toEqual(["New file", "New folder"]);
    expect(await insideViewport(page)).toBe(true);
    expect((await directory.boundingBox()).height).toBe(before.height);
    await newMenu(page).getByRole("menuitem", { name: "New file" }).hover();
    expect(await actionOpacity(directory)).toBe("1");
    await expect.poll(() => menuSettled(page)).toBe(true);
    await captureLayout(page, "files-new-menu-desktop.png");
    await page.keyboard.press("Escape");
    await expect.poll(() => menuOpen(page)).toBe(false);
    expect(await newOpener(directory).evaluate((node) => node === document.activeElement)).toBe(true);
    await newOpener(directory).click();
    await expect.poll(() => menuOpen(page)).toBe(true);
    await page.locator("#toolbar").click();
    await expect.poll(() => menuOpen(page)).toBe(false);
    expect(await directory.getAttribute("aria-expanded")).toBe("false");
    await newOpener(directory).focus();
    await page.keyboard.press("ArrowDown");
    await expect.poll(() => menuOpen(page)).toBe(true);
    await page.keyboard.press("Enter");
    const field = page.getByRole("textbox", { name: "New file name" });
    expect(await field.getAttribute("placeholder")).toBe("File name");
    expect(await field.evaluate((node) => node === document.activeElement)).toBe(true);
    await page.keyboard.type("notes.md");
    await page.keyboard.press("Enter");
    await expect.poll(() => row(page, "spa/notes.md").getAttribute("aria-current")).toBe("true");
    await page.waitForFunction(() => document.querySelector(".fppath")?.textContent === "spa/notes.md");
    expect(await page.getByRole("textbox", { name: "New file name" }).count()).toBe(0);
  });
}, 60_000);

it("opens the New menu after a touch reveal, closes on an outside tap, and drafts a file", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountActions(page, basePath, true, NEW_MENU);
    const directory = row(page, "spa");
    const client = await page.context().newCDPSession(page);
    await touchStart(client, directory);
    await page.waitForFunction(() => document.querySelector('.frow[data-path="spa"]').classList.contains("fupload-revealed"));
    await touchEnd(client);
    await newOpener(directory).tap();
    await expect.poll(() => menuOpen(page)).toBe(true);
    expect(await menuLabels(page)).toEqual(["New file", "New folder"]);
    expect(await insideViewport(page)).toBe(true);
    const item = await newMenu(page).getByRole("menuitem", { name: "New file" }).boundingBox();
    expect(item.height).toBeGreaterThanOrEqual(32);
    await expect.poll(() => menuSettled(page)).toBe(true);
    await captureLayout(page, "files-new-menu-mobile.png");
    await page.locator("#toolbar").tap();
    await expect.poll(() => menuOpen(page)).toBe(false);
    expect(await directory.getAttribute("aria-expanded")).toBe("false");
    await touchStart(client, directory);
    await page.waitForFunction(() => document.querySelector('.frow[data-path="spa"]').classList.contains("fupload-revealed"));
    await touchEnd(client);
    await newOpener(directory).tap();
    await expect.poll(() => menuOpen(page)).toBe(true);
    await newMenu(page).getByRole("menuitem", { name: "New file" }).tap();
    const field = page.getByRole("textbox", { name: "New file name" });
    await field.waitFor();
    expect(await field.evaluate((node) => node === document.activeElement)).toBe(true);
    expect(await directory.getAttribute("aria-expanded")).toBe("false");
    await client.detach();
  }, { width: 390, height: 760, hasTouch: true });
}, 60_000);
