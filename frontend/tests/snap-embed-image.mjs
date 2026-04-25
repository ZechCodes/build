// Capture visuals of the new image embed + lightbox for eyeballing.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

// Tiny 1×1 red PNG (67 bytes) — hard-coded so the snap is
// deterministic and doesn't need a network fetch.
const SAMPLE_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

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
await p.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
await p.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
await p.waitForTimeout(400);
if (!(await p.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
  await p.click('#v2-rail-chat-toggle');
  await p.waitForTimeout(250);
}

await p.evaluate(({ id, b64 }) => {
  const content =
    `Inline screenshot attached below:\n\n` +
    `<build-image path="demo/screenshot.png" mime="image/png">\n${b64}\n</build-image>\n\n` +
    `Tap to expand.`;
  window.__v2debug.stores.messagesStore.append(id, {
    id: 'snap-embed-image', channel_id: id, sender: 'Agent',
    content, created_at: new Date().toISOString(),
  });
}, { id: chId, b64: SAMPLE_B64 });
await p.waitForTimeout(300);

// Snap inline.
const rect = await p.evaluate(() => {
  const el = document.querySelector('.v2-msg[data-msg-id="snap-embed-image"]');
  return el?.getBoundingClientRect().toJSON();
});
if (rect) {
  const x = Math.max(0, Math.round(rect.x) - 8);
  const y = Math.max(0, Math.round(rect.y) - 8);
  const w = Math.min(1440 - x, Math.round(rect.width) + 16);
  const h = Math.min(900 - y, Math.round(rect.height) + 16);
  await p.screenshot({ path: 'embed-image-inline.png', clip: { x, y, width: w, height: h } });
  console.log('wrote embed-image-inline.png');
}

// Open lightbox + snap.
await p.locator('.v2-msg[data-msg-id="snap-embed-image"] .v2-embed-image img').click({ force: true });
await p.waitForTimeout(150);
await p.screenshot({ path: 'embed-image-lightbox.png' });
console.log('wrote embed-image-lightbox.png');

await browser.close();
