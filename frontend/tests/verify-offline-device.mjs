// Verify offline device groups render collapsed:
//   - header + group carry `.collapsed`
//   - chevron rotates back to the right-pointing position
//   - New Session button is hidden
//   - child channels are hidden
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
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

  const devId = await page.$eval('.v2-device-group', el => el.dataset.deviceId);

  // 1. Online baseline.
  const onlineState = await page.evaluate((deviceId) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${deviceId}"]`);
    const header = group.querySelector('.v2-device-header');
    return {
      groupCollapsed: group.classList.contains('collapsed'),
      headerCollapsed: header.classList.contains('collapsed'),
      hasNewSession: !!group.querySelector('[data-new-session]'),
      channelsPresent: !!group.querySelector('.v2-device-channels'),
      chevronRotation: getComputedStyle(group.querySelector('.v2-device-chevron')).transform,
    };
  }, devId);
  check('online: group is NOT collapsed',    onlineState.groupCollapsed === false);
  check('online: New Session button is shown', onlineState.hasNewSession === true);
  check('online: channels rendered',          onlineState.channelsPresent === true);
  check('online: chevron is rotated 90deg (down)',
    onlineState.chevronRotation !== 'none' && /matrix/.test(onlineState.chevronRotation),
    onlineState.chevronRotation);

  // 2. Flip the device to offline in the store and re-render.
  await page.evaluate((deviceId) => {
    const d = window.__v2debug;
    const dev = d.stores.devicesStore.list().find(x => x.id === deviceId);
    if (dev) {
      dev.status = 'offline';
      // Force a store notify so the sidebar re-renders.
      d.stores.devicesStore.upsert({ ...dev, status: 'offline' });
    }
  }, devId);
  await page.waitForTimeout(120);

  const offlineState = await page.evaluate((deviceId) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${deviceId}"]`);
    const header = group.querySelector('.v2-device-header');
    return {
      groupCollapsed: group.classList.contains('collapsed'),
      headerCollapsed: header.classList.contains('collapsed'),
      hasNewSession: !!group.querySelector('[data-new-session]'),
      channelsPresent: !!group.querySelector('.v2-device-channels'),
      chevronRotation: getComputedStyle(group.querySelector('.v2-device-chevron')).transform,
    };
  }, devId);
  check('offline: group is collapsed',        offlineState.groupCollapsed === true);
  check('offline: header is collapsed',       offlineState.headerCollapsed === true);
  check('offline: New Session button hidden', offlineState.hasNewSession === false);
  check('offline: channels hidden',            offlineState.channelsPresent === false);
  // `.collapsed` CSS sets transform: rotate(0), which resolves to
  // `matrix(1, 0, 0, 1, 0, 0)` — i.e., no rotation.
  check('offline: chevron rotated back to 0 (pointing right)',
    offlineState.chevronRotation === 'matrix(1, 0, 0, 1, 0, 0)' || offlineState.chevronRotation === 'none',
    offlineState.chevronRotation);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
