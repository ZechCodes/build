// Review images for #185 on a phone (390x844): the Add project sheet asks for
// the device, then shows folders before the optional name; a remote fills the
// name; Create with no folder says so in the sheet.
// Run from spa/: node test/browser/captureNewProjectFlow.mjs <out-dir>
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const out = process.argv[2] || "/tmp/new-project-flow";
await mkdir(out, { recursive: true });
const shot = (page, name) => page.screenshot({ path: join(out, `${name}.png`) });

await withLayoutPage(async ({ page, basePath }) => {
  await mountLayout(page, '<div class="scrim" id="scrim"><div class="sheet" id="sheet"></div></div>', { basePath });
  await loadBrowserModules(page, { changes: "src/core/changeEvents.js", newRepo: "src/sheets/newRepo.js" }, basePath);
  await page.evaluate(async () => {
    const { changes, newRepo } = window.__layoutModules;
    const call = async (method) => {
      if (method === "session.hello") return { api_version: "2.0.0", capabilities: ["github.repos"] };
      if (method === "github.repos") return { repos: [] };
      if (method === "settings.get") return { projects_dir: "/home/zech/Projects" };
      return {};
    };
    await changes.greetBridge(call, { deviceId: "beast" });
    const devices = [{ id: "beast", name: "The Beast" }, { id: "air", name: "Zechariahs-MacBook-Air.local" }, { id: "mini", name: "Zechariahs-Mac-mini.local" }];
    newRepo.openNewRepo(() => {}, { devices, defaultDeviceId: "", callRpcFor: () => call });
  });
  await page.waitForTimeout(300);
  await shot(page, "1-device-choice");
  await page.selectOption("#nrdevice", "beast");
  await page.waitForTimeout(200);
  await shot(page, "2-empty-form");
  await page.locator("#nrdo").click();
  await page.waitForTimeout(100);
  console.log(JSON.stringify({ error: await page.locator("#nrerr").textContent(), focused: await page.evaluate(() => document.activeElement?.id || document.activeElement?.textContent) }));
  await shot(page, "3-no-folder-error");
  await page.locator("#nraddremote").click();
  await page.locator("[data-source-value]").pressSequentially("git@github.com:ZechCodes/Skrift.git");
  await page.keyboard.press("Escape");
  await page.locator("#nrproject").scrollIntoViewIfNeeded();
  console.log(JSON.stringify({ name: await page.locator("#nrproject").inputValue(), label: await page.locator('label[for="nrproject"]').textContent() }));
  await shot(page, "4-remote-names-project");
}, { width: 390, height: 844, plugins: [deviceShim] });
