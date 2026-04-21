// Verify that a Skrift `sk:notification` with build:device:offline
// collapses the sidebar group live (no reload) — and the reverse
// for online.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await ctx.newPage();

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const n = await page.$('input[name="name"]');
  if (n) await page.fill('input[name="name"]', 'Dev');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);
  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const devId = await page.$eval('.v2-device-group', el => el.dataset.deviceId);

  // Baseline: device starts online → expanded.
  const initial = await page.evaluate((id) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${id}"]`);
    return {
      collapsed: group?.classList.contains('collapsed'),
      hasNewSession: !!group?.querySelector('[data-new-session]'),
      hasChannels:    !!group?.querySelector('.v2-device-channels'),
    };
  }, devId);
  check('baseline: group expanded',                initial.collapsed === false);
  check('baseline: New Session button visible',    initial.hasNewSession === true);
  check('baseline: channels rendered',              initial.hasChannels === true);

  // Dispatch a build:device:offline notification.
  await page.evaluate((id) => {
    document.dispatchEvent(new CustomEvent('sk:notification', {
      detail: { type: 'build:device:offline', device_id: id },
    }));
  }, devId);
  await page.waitForTimeout(150);
  const afterOffline = await page.evaluate((id) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${id}"]`);
    return {
      collapsed: group?.classList.contains('collapsed'),
      hasNewSession: !!group?.querySelector('[data-new-session]'),
      hasChannels:    !!group?.querySelector('.v2-device-channels'),
    };
  }, devId);
  check('offline: group collapsed live',            afterOffline.collapsed === true);
  check('offline: New Session button hidden',       afterOffline.hasNewSession === false);
  check('offline: channels hidden',                  afterOffline.hasChannels === false);

  // Heartbeat-missed is treated the same as offline — double-check.
  // Flip back to online first.
  await page.evaluate((id) => {
    document.dispatchEvent(new CustomEvent('sk:notification', {
      detail: { type: 'build:device:online', device_id: id },
    }));
  }, devId);
  await page.waitForTimeout(150);
  const backOnline = await page.evaluate((id) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${id}"]`);
    return { collapsed: group?.classList.contains('collapsed') };
  }, devId);
  check('online notification re-expands the group', backOnline.collapsed === false);

  await page.evaluate((id) => {
    document.dispatchEvent(new CustomEvent('sk:notification', {
      detail: { type: 'build:device:heartbeat-missed', device_id: id },
    }));
  }, devId);
  await page.waitForTimeout(150);
  const afterHeartbeat = await page.evaluate((id) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${id}"]`);
    return { collapsed: group?.classList.contains('collapsed') };
  }, devId);
  check('heartbeat-missed collapses the group', afterHeartbeat.collapsed === true);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
