// Verify the full files tree (All tab) hydrates even when the initial
// `intent.files_list` fires before the E2EE connection is ready.
// The race:
//   channelRegistry.init activates the current channel synchronously,
//   which calls FilesView.activate → _fetchInitial → bus.emit intent.files_list.
//   At that moment `bindIntentDispatcher()` may not have been called
//   yet (it runs inside `initTransport`). The intent is emitted with no
//   listener and silently dropped. Later, files.list_result never
//   arrives, and the "All" tab shows "Loading…" forever.
//
// FilesView now re-fires _fetchInitial on every `e2ee.connected` for
// its channel's device, which recovers from the race.
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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);

  // Simulate the race: clear any tree we might have cached already,
  // then fire e2ee.connected with the channel's device and confirm
  // FilesView re-fires intent.files_list.
  const device = await page.evaluate((id) => {
    return window.__v2debug.stores.channelsStore.deviceFor(id);
  }, chId);
  check('channel has a device',  !!device, device);

  // Spy on intent.files_list.
  await page.evaluate(() => {
    window.__listCalls = 0;
    window.__v2debug.bus.on('intent.files_list', () => { window.__listCalls++; });
  });
  // Clear the root tree slot to simulate "initial fetch dropped".
  await page.evaluate((id) => {
    const slot = window.__v2debug.stores.filesStore.treeFor(id);
    slot.delete('');
  }, chId);

  // Fire e2ee.connected for this channel's device.
  await page.evaluate((deviceId) => {
    window.__v2debug.bus.emit('e2ee.connected', { deviceId });
  }, device);
  await page.waitForTimeout(80);

  const listCalls = await page.evaluate(() => window.__listCalls);
  check('FilesView re-fires intent.files_list on e2ee.connected',
    listCalls >= 1, `calls=${listCalls}`);

  // The intent fires for the root path (empty string), plus
  // intent.files_changes (not counted here).
  const lastCall = await page.evaluate(() => window.__v2debug_last_list_call || null);
  // We didn't capture the payload — just confirm the call fired. The
  // next assertion is that a different-device e2ee.connected does NOT
  // trigger another call.
  await page.evaluate(() => { window.__listCalls = 0; });
  await page.evaluate(() => {
    window.__v2debug.bus.emit('e2ee.connected', { deviceId: 'not-this-channels-device' });
  });
  await page.waitForTimeout(80);
  const strayCalls = await page.evaluate(() => window.__listCalls);
  check('unrelated-device e2ee.connected does NOT trigger re-fetch',
    strayCalls === 0, `strayCalls=${strayCalls}`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
