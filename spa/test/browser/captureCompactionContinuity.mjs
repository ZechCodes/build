// Review artifact for #366. From spa/:
// node test/browser/captureCompactionContinuity.mjs /tmp/compaction-review [drag|click|Enter] [--tasks] [--near-bottom]
// Uses Chromium CDP frames and the system ffmpeg; no Playwright ffmpeg install.
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";
import { compactionContinuity, observeCompactionMenu } from "./compactionMenuContinuity.mjs";

const output = resolve(process.argv[2] || "/tmp/compaction-review");
const gesture = process.argv[3] || "drag";
const nearBottom = process.argv.includes("--near-bottom");
const changingTasks = process.argv.includes("--tasks") || nearBottom;
if (!["drag", "click", "Enter"].includes(gesture)) throw new Error("Gesture must be drag, click, or Enter");
const name = `compaction-${gesture.toLowerCase()}-300ms${changingTasks ? "-tasks" : ""}${nearBottom ? "-near-bottom" : ""}`;
const frames = await mkdtemp(join(tmpdir(), "build-compaction-frames-"));
await mkdir(output, { recursive: true });
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("BRIDGE_")));

const menuBounds = (page) => page.evaluate(() => {
  const box = document.querySelector(".rail-surface-menu .splitmenu").getBoundingClientRect();
  const panel = document.querySelector("#rail-panel").getBoundingClientRect();
  return { top: box.top, bottom: box.bottom, bottomBound: Math.min(window.innerHeight, panel.bottom) };
});

async function reopenNearBottom(page) {
  const menuHeight = await page.locator(".rail-surface-menu .splitmenu").evaluate((menu) => menu.getBoundingClientRect().height);
  await page.keyboard.press("Escape");
  await settled(page);
  await page.evaluate((menuHeight) => {
    const panel = document.querySelector("#rail-panel");
    const caret = document.querySelector(".rail-surface-menu .caret").getBoundingClientRect();
    panel.style.bottom = "auto";
    panel.style.height = `${caret.bottom + 6 + menuHeight + 12 - panel.getBoundingClientRect().top}px`;
  }, menuHeight);
  await page.locator(".rail-surface-menu .caret").click();
  await settled(page);
}

async function record(page, act) {
  const session = await page.context().newCDPSession(page);
  const captures = [];
  const writes = [];
  const acknowledgments = [];
  const onFrame = ({ data, sessionId, metadata }) => {
    const path = join(frames, `${String(captures.length).padStart(6, "0")}.jpg`);
    captures.push({ path, timestamp: metadata.timestamp });
    writes.push(writeFile(path, Buffer.from(data, "base64")));
    writes.push(session.send("Page.screencastFrameAck", { sessionId }).catch((error) => acknowledgments.push(error)));
  };
  session.on("Page.screencastFrame", onFrame);
  await session.send("Page.startScreencast", { format: "jpeg", quality: 95, everyNthFrame: 1 });
  // A brief DOM update can fall between compositor screencast frames. Insert
  // an explicit screenshot at the same wall-clock time for that review step.
  const captureFrame = async (data) => {
    const path = join(frames, `${String(captures.length).padStart(6, "0")}-manual.jpg`);
    captures.push({ path, timestamp: Date.now() / 1000 });
    await writeFile(path, data);
  };
  let failure;
  try {
    await act(captureFrame);
  } catch (error) {
    failure = error;
  } finally {
    await session.send("Page.stopScreencast");
    session.off("Page.screencastFrame", onFrame);
    await Promise.all(writes);
    await session.detach();
  }
  if (acknowledgments.length) throw acknowledgments[0];
  if (!captures.length) throw new Error("Chromium produced no screencast frames");
  captures.sort((left, right) => left.timestamp - right.timestamp);
  const list = captures.map((frame, index) => {
    const duration = Math.max(1 / 60, (captures[index + 1]?.timestamp ?? frame.timestamp + 0.5) - frame.timestamp);
    return `file '${frame.path}'\nduration ${duration}`;
  }).join("\n");
  const input = join(frames, "frames.ffconcat");
  await writeFile(input, `${list}\nfile '${captures.at(-1).path}'\n`);
  const result = spawnSync("nice", ["-n", "10", "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
    "-f", "concat", "-safe", "0", "-i", input, "-fps_mode", "vfr", "-c:v", "libvpx-vp9", "-pix_fmt", "yuv420p",
    join(output, `${name}.webm`)], { env, encoding: "utf8" });
  if (result.error || result.status !== 0) throw new Error(`ffmpeg failed: ${result.error || result.stderr}`);
  if (failure) throw failure;
}

try {
  await withLayoutPage(async ({ page, basePath }) => {
    await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false, settingsDelayMs: 300 });
    if (nearBottom) await reopenNearBottom(page);
    await observeCompactionMenu(page);
    const bounds = [{ phase: "before", ...await menuBounds(page) }];
    await record(page, async (captureFrame) => {
      await page.waitForTimeout(400);
      const slider = page.locator('.rail-surface-menu [role="slider"]');
      const box = await slider.boundingBox();
      const y = box.y + box.height / 2;
      if (gesture === "Enter") {
        for (let stop = 0; stop < 3; stop += 1) await page.keyboard.press("ArrowRight");
        await page.keyboard.press("Enter");
      } else if (gesture === "click") {
        await page.mouse.click(box.x + box.width * 0.68, y);
      } else {
        await page.mouse.move(box.x + 10, y);
        await page.mouse.down();
        await page.mouse.move(box.x + box.width * 0.68, y, { steps: 20 });
        await page.waitForTimeout(200);
        await page.mouse.up();
      }
      if (changingTasks) {
        await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
        await page.evaluate(() => window.__setMenuTasks([{
          id: "menu-task", number: 366, title: "Keep the menu open", state: "open", status: "in_progress",
          assignee: { kind: "agent", agent_id: "menu-agent" },
        }]));
        await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "attached" });
        bounds.push({ phase: "tasks-visible", ...await menuBounds(page) });
        await captureFrame(await page.screenshot({ type: "jpeg", path: join(output, `${name}-tasks-visible.jpg`) }));
        await page.waitForTimeout(40);
        await page.evaluate(() => window.__setMenuTasks([]));
        await page.locator('.rail-surface-menu [data-action="tasks"]').waitFor({ state: "detached" });
        bounds.push({ phase: "tasks-removed", ...await menuBounds(page) });
        if (await page.evaluate(() => window.__menuSettingsAnswered.length)) {
          throw new Error("Task structure changes must complete during the 300ms pending reply");
        }
      }
      await page.waitForFunction(() => window.__menuSettingsAnswered.length === 1);
      await page.waitForFunction(() => JSON.parse(document.querySelector('[data-group="compact"] .menu-slider').dataset.options)
        .some((option) => option.id === "compact:300000" && option.selected));
      await settled(page);
      await page.waitForTimeout(600);
      await page.screenshot({ path: join(output, `${name}.png`) });
      bounds.push({ phase: "after-reply", ...await menuBounds(page) });
      const observation = await compactionContinuity(page, { stop: true });
      await writeFile(join(output, `${name}.json`), `${JSON.stringify({ settingsDelayMs: 300, gesture, changingTasks, nearBottom, bounds, ...observation }, null, 2)}\n`);
      if (observation.violations.length || observation.motionEvents.length || !observation.sameMenu
        || !observation.sameSlider || !observation.sliderFocused) {
        throw new Error(`Menu continuity failed: ${JSON.stringify(observation)}`);
      }
      if (bounds.some((box) => box.top < 7.5 || box.bottom > box.bottomBound - 7.5)) {
        throw new Error(`Menu bounds failed: ${JSON.stringify(bounds)}`);
      }
    });
  }, { width: 1180, height: 840 });
} finally {
  await rm(frames, { recursive: true, force: true });
}
console.log(`Wrote ${join(output, name)}.{webm,png,json}`);
