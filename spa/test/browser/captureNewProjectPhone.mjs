// Review images for the Add project sheet on a phone (390x844): the device
// choice alone, the full form once a device is chosen, and the GitHub
// repository list open over the fields under a Git remote URL — one remote
// high in the sheet, one near its bottom. Prints what it measured: where the
// field under the input sat with the list closed and open, and the list's box
// against the input's.
// Run from spa/: node test/browser/captureNewProjectPhone.mjs <out-dir> [prefix]
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const out = process.argv[2] || "/tmp/new-project-phone";
const prefix = process.argv[3] || "";
await mkdir(out, { recursive: true });
const shot = (page, name) => page.screenshot({ path: join(out, `${prefix}${name}.png`) });

const repo = (name_with_owner, description, isPrivate, daysAgo) => ({
  name_with_owner, description, private: isPrivate,
  ssh_url: `git@github.com:${name_with_owner}.git`, url: `https://github.com/${name_with_owner}`,
  pushed_at: new Date(Date.now() - daysAgo * 86_400_000).toISOString(),
});
const REPOS = [
  repo("ZechCodes/skrift", "Async web framework", false, 0),
  repo("ZechCodes/skrift-worker", "Background jobs for Skrift", false, 1),
  repo("ZechCodes/build-web", "Build — local-first agent orchestration platform", true, 2),
  repo("Smarter-Dev/smarter-dev", "The Smarter Dev community site", false, 3),
  repo("Smarter-Dev/bot", "Discord bot for the Smarter Dev server", false, 5),
  repo("beginner-codes/bevy", "Dependency injection for Python", false, 40),
  repo("ZechCodes/dotfiles", "", false, 12),
  repo("8ly-dev/buildkit", "Shared CI actions", true, 30),
];

const rect = (page, selector) => page.evaluate((s) => {
  const node = document.querySelector(s);
  if (!node) return null;
  const { top, left, width, height, bottom } = node.getBoundingClientRect();
  return { top: Math.round(top), left: Math.round(left), width: Math.round(width), height: Math.round(height), bottom: Math.round(bottom) };
}, selector);

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, '<div class="scrim" id="scrim"><div class="sheet" id="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, { changes: "src/core/changeEvents.js", newRepo: "src/sheets/newRepo.js" }, basePath);
  await page.evaluate(async (repos) => {
    const { changes, newRepo } = window.__layoutModules;
    const call = async (method) => {
      if (method === "session.hello") return { api_version: "2.0.0", capabilities: ["github.repos"] };
      if (method === "github.repos") return { repos };
      if (method === "settings.get") return { projects_dir: "/home/zech/Projects" };
      return {};
    };
    await changes.greetBridge(call, { deviceId: "beast" });
    const devices = [{ id: "beast", name: "The Beast" }, { id: "air", name: "Zechariahs-MacBook-Air.local" }, { id: "mini", name: "Zechariahs-Mac-mini.local" }];
    newRepo.openNewRepo(() => {}, { devices, defaultDeviceId: "", callRpcFor: () => call });
  }, REPOS);
  await page.waitForTimeout(300);
  await shot(page, "1-device-choice");
  await page.selectOption("#nrdevice", "beast");
  await page.waitForTimeout(200);
  await page.locator("#nrproject").fill("Skrift");
  await shot(page, "2-full-form");

  // Two remotes: the first has the second's fields under it; the second sits
  // at the bottom of the sheet.
  await page.locator("#nraddremote").click();
  await page.locator("#nraddremote").click();
  const inputs = page.locator("[data-source-value]");
  const firstBelow = '[data-source-name]';
  await inputs.nth(0).click();
  await inputs.nth(0).pressSequentially("skrift");
  await page.waitForFunction(() => document.querySelectorAll('[role="option"]').length >= 2);
  const open = await rect(page, firstBelow);
  const list = await rect(page, ".repo-picker-list:not([hidden])");
  const input = await rect(page, "[data-source-value]");
  await shot(page, "3-list-over-fields");
  await page.keyboard.press("Escape"); // closes the list, focus and scroll stay
  const closed = await rect(page, firstBelow);
  console.log(JSON.stringify({ state: "first remote", fieldBelowOpen: open, fieldBelowClosed: closed, input, list }));

  await page.keyboard.press("Escape");
  await inputs.nth(1).scrollIntoViewIfNeeded();
  await inputs.nth(1).click();
  await inputs.nth(1).pressSequentially("sk");
  await page.waitForFunction(() => document.querySelectorAll('.repo-picker-list:not([hidden]) [role="option"]').length >= 2);
  const second = await page.evaluate(() => {
    const input = document.querySelectorAll("[data-source-value]")[1];
    const list = document.querySelector(".repo-picker-list:not([hidden])");
    const a = input.getBoundingClientRect(), b = list.getBoundingClientRect();
    return { input: { top: Math.round(a.top), bottom: Math.round(a.bottom), left: Math.round(a.left), width: Math.round(a.width) },
      list: { top: Math.round(b.top), bottom: Math.round(b.bottom), left: Math.round(b.left), width: Math.round(b.width), scrollHeight: list.scrollHeight, clientHeight: list.clientHeight },
      viewport: innerHeight, sheetScrollTop: document.querySelector("#sheet").scrollTop };
  });
  console.log(JSON.stringify({ state: "bottom remote", ...second }));
  await shot(page, "4-list-near-bottom");

  // The same field scrolled down to the bottom edge of the phone's screen: the
  // list has no room under it, so it opens above the input instead.
  await page.keyboard.press("Escape");
  await page.evaluate(() => {
    const sheet = document.querySelector("#sheet");
    const input = document.querySelectorAll("[data-source-value]")[1];
    sheet.scrollTop -= innerHeight - 60 - input.getBoundingClientRect().bottom;
    input.blur();
  });
  await inputs.nth(1).click();
  await page.waitForFunction(() => document.querySelectorAll('.repo-picker-list:not([hidden]) [role="option"]').length >= 2);
  const edge = await page.evaluate(() => {
    const input = document.querySelectorAll("[data-source-value]")[1];
    const list = document.querySelector(".repo-picker-list:not([hidden])");
    const a = input.getBoundingClientRect(), b = list.getBoundingClientRect();
    return { input: { top: Math.round(a.top), bottom: Math.round(a.bottom) }, list: { top: Math.round(b.top), bottom: Math.round(b.bottom), width: Math.round(b.width) }, viewport: innerHeight };
  });
  console.log(JSON.stringify({ state: "remote at the screen's bottom edge", ...edge }));
  await shot(page, "5-list-at-screen-bottom");
}, { width: 390, height: 844, plugins: [deviceShim] });
