// Verify the per-device reconnect pill:
//   - Hidden when SSE + E2EE are healthy.
//   - Visible with "Reconnecting to X…" when the active channel's
//     device goes offline / E2EE drops.
//   - Suppressed (hidden) while the SSE pill is visible — no point
//     nagging about E2EE when the underlying stream is down.
//   - Returns when SSE comes back.
//   - Shows "Disconnected from X" + Retry button once the self-heal
//     loop gives up.
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
  const devId = await page.$eval('.v2-device-group', el => el.dataset.deviceId);
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);

  // Baseline — healthy.
  const initial = await page.evaluate(() => {
    const pill = document.querySelector('.v2-reconnect-pill');
    return { hasOpen: pill?.classList.contains('open') };
  });
  check('baseline: reconnect pill hidden', initial.hasOpen === false, JSON.stringify(initial));

  // Simulate E2EE disconnect by flipping the device offline + emit
  // e2ee.disconnected. The pill should show "Reconnecting to …".
  await page.evaluate((devId) => {
    const d = window.__v2debug;
    d.stores.devicesStore.setStatus(devId, 'offline');
    d.bus.emit('e2ee.disconnected', { deviceId: devId });
  }, devId);
  await page.waitForTimeout(200);

  const reconnecting = await page.evaluate(() => {
    const pill = document.querySelector('.v2-reconnect-pill');
    const label = pill?.querySelector('.v2-reconnect-pill-label')?.textContent?.trim();
    return {
      open: pill?.classList.contains('open'),
      failed: pill?.classList.contains('failed'),
      label,
      retryHidden: pill?.querySelector('.v2-reconnect-pill-retry')?.hidden,
    };
  });
  check('reconnect pill opens on e2ee drop', reconnecting.open === true, JSON.stringify(reconnecting));
  check('label reads "Reconnecting to …"',     /reconnecting to/i.test(reconnecting.label || ''));
  check('retry button hidden while retrying',  reconnecting.retryHidden === true);

  // Suppression while SSE is down.
  await page.evaluate(() => {
    window.__v2debug.bus.emit('sse.disconnected', {});
  });
  await page.waitForTimeout(100);
  const suppressed = await page.evaluate(() =>
    document.querySelector('.v2-reconnect-pill')?.classList.contains('open'));
  check('reconnect pill hidden while SSE pill is visible', suppressed === false);

  // SSE back → pill returns. (The coordinator will refetch devices
  // on this event and may flip our test device back to online — so
  // we re-assert the offline state after a tick.)
  await page.evaluate(() => {
    window.__v2debug.bus.emit('sse.connected', {});
  });
  await page.waitForTimeout(200);
  await page.evaluate((devId) => {
    const d = window.__v2debug;
    d.stores.devicesStore.setStatus(devId, 'offline');
    d.bus.emit('e2ee.disconnected', { deviceId: devId });
  }, devId);
  await page.waitForTimeout(100);
  const back = await page.evaluate(() =>
    document.querySelector('.v2-reconnect-pill')?.classList.contains('open'));
  check('reconnect pill returns when SSE comes back', back === true);

  // Simulate the self-heal loop exhausting its retries.
  await page.evaluate(() => {
    window.__v2debug.bus.emit('reconnect.state', {
      deviceId: window.__v2debug.stores.channelsStore.deviceFor(
        window.__v2debug.stores.uiStore.getActiveChannel()),
      phase: 'failed',
      attempt: 5,
    });
  });
  await page.waitForTimeout(100);
  const givenUp = await page.evaluate(() => {
    const pill = document.querySelector('.v2-reconnect-pill');
    const retry = pill?.querySelector('.v2-reconnect-pill-retry');
    return {
      label: pill?.querySelector('.v2-reconnect-pill-label')?.textContent?.trim(),
      failed: pill?.classList.contains('failed'),
      retryVisible: retry && retry.hidden === false,
    };
  });
  check('pill flips to failed state on give-up',       givenUp.failed === true, JSON.stringify(givenUp));
  check('failed label reads "Disconnected from …"',    /disconnected from/i.test(givenUp.label || ''));
  check('retry button visible when failed',            givenUp.retryVisible === true);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
