// Snap three embed states: normal, wide (needs horizontal scroll),
// and tall (vertical clip). Helps eyeball the constraints we added
// (min-width, max-height, overflow-x auto, overflow-y hidden).
import { chromium } from 'playwright';
import { readFileSync } from 'node:fs';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

// Reuse existing screenshots from frontend/tests as fixtures.
function b64(path) {
  return readFileSync(path).toString('base64');
}
const SRC_NORMAL = b64('activity-tags.png');       // ~320×900 tall
const SRC_WIDE   = b64('rail-desktop.png');        // ~1440×42 wide
const SRC_TALL   = b64('comp-popover.png');        // ~440×360 tall

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const p = await ctx.newPage();

await p.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await p.fill('input[name="email"]', EMAIL);
const n = await p.$('input[name="name"]'); if (n) await p.fill('input[name="name"]', 'Dev');
await Promise.all([
  p.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  p.click('button[type="submit"]'),
]);
await p.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
await p.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
await p.waitForTimeout(400);
if (!(await p.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
  await p.click('#v2-rail-chat-toggle');
  await p.waitForTimeout(250);
}

await p.evaluate(({ id, normal, wide, tall }) => {
  const send = (msgId, tag) =>
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId, channel_id: id, sender: 'Agent',
      content: tag, created_at: new Date().toISOString(),
    });
  send('embed-normal', `Normal:\n\n<build-image path="/x/activity-tags.png" mime="image/png">\n${normal}\n</build-image>`);
  send('embed-wide',   `Wide:\n\n<build-image path="/x/rail-desktop.png" mime="image/png">\n${wide}\n</build-image>`);
  send('embed-tall',   `Tall:\n\n<build-image path="/x/comp-popover.png" mime="image/png">\n${tall}\n</build-image>`);
}, { id: chId, normal: SRC_NORMAL, wide: SRC_WIDE, tall: SRC_TALL });
await p.waitForTimeout(400);

for (const name of ['embed-normal', 'embed-wide', 'embed-tall']) {
  // Scroll each message into view, wait for fonts/layout, then clip.
  await p.evaluate((m) => {
    const el = document.querySelector(`.v2-msg[data-msg-id="${m}"]`);
    el?.scrollIntoView({ block: 'center', behavior: 'instant' });
  }, name);
  await p.waitForTimeout(200);
  const rect = await p.evaluate((m) => {
    const el = document.querySelector(`.v2-msg[data-msg-id="${m}"]`);
    return el?.getBoundingClientRect().toJSON();
  }, name);
  if (!rect || rect.width < 10 || rect.height < 10) {
    console.log(`skip ${name}  rect=${JSON.stringify(rect)}`);
    continue;
  }
  const x = Math.max(0, Math.round(rect.x) - 8);
  const y = Math.max(0, Math.round(rect.y) - 8);
  const w = Math.min(1440 - x, Math.round(rect.width) + 16);
  const h = Math.min(900 - y, Math.round(rect.height) + 16);
  await p.screenshot({ path: `${name}.png`, clip: { x, y, width: w, height: h } });
  console.log(`wrote ${name}.png  ${w}×${h}`);
}

await browser.close();
