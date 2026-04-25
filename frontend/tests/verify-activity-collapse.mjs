// Verify activity entries are collapsed by default and click to expand.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

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
  await page.waitForSelector('.v2-channel-sidebar-item');
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(1200);

  // Expand activity panel.
  await page.evaluate(() => {
    document.querySelector('.v2-sidebar-section[data-section="activity"]').classList.remove('collapsed');
  });
  await page.waitForTimeout(150);

  // Seed a couple of activity entries.
  await page.evaluate((id) => {
    window.__v2debug.stores.activityStore.replace(id, [
      { type: 'tool_use', created_at: new Date().toISOString(), data: { id: 'b1', name: 'Bash', input: { command: 'ls -la' } } },
      { type: 'text', created_at: new Date().toISOString(), data: { text: 'All 4 pass. Commit.' } },
    ]);
  }, chId);
  await page.waitForTimeout(250);

  // Details should be collapsed (display:none) by default.
  const beforeVisible = await page.$$eval('#v2-activity-body .v2-ce-detail',
    els => els.map(el => getComputedStyle(el).display));
  check('activity details start hidden', beforeVisible.every(d => d === 'none'),
    beforeVisible.join(','));

  // Click the first row → its detail should become visible.
  const firstRow = await page.$('#v2-activity-body .v2-ce-row[data-toggle]');
  await firstRow.click();
  await page.waitForTimeout(150);
  const firstDetailDisplay = await page.$eval('#v2-activity-body .v2-ce:first-child .v2-ce-detail',
    el => getComputedStyle(el).display);
  check('click expands first entry', firstDetailDisplay === 'flex', firstDetailDisplay);

  // Second row's detail should still be hidden.
  const secondDetailDisplay = await page.$eval('#v2-activity-body .v2-ce:nth-child(2) .v2-ce-detail',
    el => getComputedStyle(el).display);
  check('second entry remains collapsed', secondDetailDisplay === 'none', secondDetailDisplay);

  // Click again → collapses back.
  await firstRow.click();
  await page.waitForTimeout(150);
  const afterCollapse = await page.$eval('#v2-activity-body .v2-ce:first-child .v2-ce-detail',
    el => getComputedStyle(el).display);
  check('click-again collapses first entry', afterCollapse === 'none', afterCollapse);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
