// Snap the new rail at desktop + mobile so we can eyeball the layout:
// icon-only Terminal/Chat buttons on each side, complications strip
// scrolling in the middle, no "Connected" label.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

async function snap({ width, height, isMobile, label }) {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({
    viewport: { width, height }, isMobile, hasTouch: isMobile,
  });
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

  if (isMobile) {
    await p.evaluate(() => document.querySelector('.v2-app')?.classList.add('sidebar-open'));
    await p.waitForTimeout(150);
  }
  const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await p.locator(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`).click({ force: true });
  await p.waitForTimeout(500);
  if (isMobile) {
    await p.evaluate(() => document.querySelector('.v2-app')?.classList.remove('sidebar-open'));
  }
  // Close chat overlay if pinned so we can see the rail.
  if (await p.$eval('#v2-chat-overlay', el => el.classList.contains('open'))) {
    await p.locator('#v2-rail-chat-toggle').click({ force: true });
    await p.waitForTimeout(200);
  }

  const rect = await p.evaluate(() => {
    const el = document.getElementById('v2-rail');
    return el?.getBoundingClientRect().toJSON();
  });
  if (!rect) { console.log('no rail'); await browser.close(); return; }
  const x = 0;
  const y = Math.max(0, Math.round(rect.y) - 6);
  const w = Math.round(rect.width);
  const h = Math.min(height - y, Math.round(rect.height) + 6);
  await p.screenshot({ path: `rail-${label}.png`, clip: { x, y, width: w, height: h } });
  console.log(`wrote rail-${label}.png  y=${y} h=${h}`);
  await browser.close();
}

await snap({ width: 1440, height: 900, isMobile: false, label: 'desktop' });
await snap({ width: 375, height: 812, isMobile: true, label: 'mobile' });
