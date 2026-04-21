// Verify the user-initiated collapse toggle on device group headers:
//   - Click header → group collapses (class + children hidden).
//   - Click again → expands.
//   - localStorage persists the user's preference across a reload.
//   - Clicks on the New Session button DON'T toggle the group.
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

function readState(page, devId) {
  return page.evaluate((id) => {
    const group = document.querySelector(`.v2-device-group[data-device-id="${id}"]`);
    return {
      collapsed: group?.classList.contains('collapsed'),
      hasNewSession: !!group?.querySelector('[data-new-session]'),
      hasChannels: !!group?.querySelector('.v2-device-channels'),
      pref: localStorage.getItem(`v2.device.${id}.collapsed`),
    };
  }, devId);
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

  // Wipe any persisted pref so we start from the status-based default.
  await page.evaluate((id) => {
    localStorage.removeItem(`v2.device.${id}.collapsed`);
  }, devId);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

  const initial = await readState(page, devId);
  check('initial: group expanded (online default)',  initial.collapsed === false, JSON.stringify(initial));
  check('initial: no pref stored yet',                 initial.pref === null);
  check('initial: New Session + channels rendered',    initial.hasNewSession && initial.hasChannels);

  // Click the header → should collapse.
  await page.click(`.v2-device-group[data-device-id="${devId}"] .v2-device-header`);
  await page.waitForTimeout(100);
  const afterFirst = await readState(page, devId);
  check('click #1: collapsed class on group',           afterFirst.collapsed === true, JSON.stringify(afterFirst));
  check('click #1: New Session hidden',                 afterFirst.hasNewSession === false);
  check('click #1: channels hidden',                    afterFirst.hasChannels === false);
  check('click #1: pref stored "1"',                    afterFirst.pref === '1');

  // Click again → expands.
  await page.click(`.v2-device-group[data-device-id="${devId}"] .v2-device-header`);
  await page.waitForTimeout(100);
  const afterSecond = await readState(page, devId);
  check('click #2: expanded',                           afterSecond.collapsed === false);
  check('click #2: New Session back',                   afterSecond.hasNewSession === true);
  check('click #2: channels back',                      afterSecond.hasChannels === true);
  check('click #2: pref stored "0"',                    afterSecond.pref === '0');

  // Collapse, then reload — pref should survive.
  await page.click(`.v2-device-group[data-device-id="${devId}"] .v2-device-header`);
  await page.waitForTimeout(100);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-device-group', { timeout: 8000 });
  const afterReload = await readState(page, devId);
  check('after reload: still collapsed',                afterReload.collapsed === true, JSON.stringify(afterReload));
  check('after reload: pref still "1"',                 afterReload.pref === '1');

  // Clicking the New Session button should NOT trigger a collapse.
  // Expand first.
  await page.click(`.v2-device-group[data-device-id="${devId}"] .v2-device-header`);
  await page.waitForTimeout(100);
  const beforeNewSession = await readState(page, devId);
  await page.click(`.v2-device-group[data-device-id="${devId}"] [data-new-session]`);
  await page.waitForTimeout(100);
  const afterNewSessionBtn = await readState(page, devId);
  check('clicking New Session does not collapse',       afterNewSessionBtn.collapsed === beforeNewSession.collapsed);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
