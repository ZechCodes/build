// Capture the review image for #160: the New project sheet with the GitHub
// repository picker open under a Git remote URL, on a bridge that announces
// github.repos.
// Run from spa/: node test/browser/captureRepoPicker.mjs [output.png] [width] [height]
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";

const output = process.argv[2] || "/tmp/repo-picker.png";
const width = Number(process.argv[3]) || 1440;
const height = Number(process.argv[4]) || 900;

const repo = (name_with_owner, description, isPrivate, daysAgo) => ({
  name_with_owner, description, private: isPrivate,
  ssh_url: `git@github.com:${name_with_owner}.git`, url: `https://github.com/${name_with_owner}`,
  pushed_at: new Date(Date.now() - daysAgo * 86_400_000).toISOString(),
});
const REPOS = [
  repo("ZechCodes/build-web", "Build — local-first agent orchestration platform", true, 0),
  repo("ZechCodes/build-releases", "Signed releases of the Build bridge", false, 1),
  repo("ZechCodes/build-secure-transport", "End-to-end encrypted transport for Build", true, 3),
  repo("Smarter-Dev/smarter-dev", "The Smarter Dev community site", false, 2),
  repo("Smarter-Dev/bot", "Discord bot for the Smarter Dev server", false, 5),
  repo("beginner-codes/bevy", "Dependency injection for Python", false, 40),
  repo("ZechCodes/dotfiles", "", false, 12),
  repo("8ly-dev/buildkit", "Shared CI actions", true, 30),
  repo("ZechCodes/rebuild-notes", "Scratch notes", true, 90),
];

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
    await changes.greetBridge(call, { deviceId: "dev-1" });
    newRepo.openNewRepo(() => {}, { devices: [{ id: "dev-1", name: "the-beast" }], defaultDeviceId: "dev-1", callRpcFor: () => call });
  }, REPOS);
  await page.locator("#nrproject").fill("Build");
  await page.locator("#nraddremote").click();
  const input = page.locator("[data-source-value]");
  await input.pressSequentially("build");
  await page.waitForFunction(() => document.querySelectorAll('[role="option"]').length >= 3);
  await input.press("ArrowDown");
  console.log(await page.locator('[role="option"] .repo-picker-name').allTextContents());
  await page.screenshot({ path: output });
}, { width, height, plugins: [deviceShim] });
