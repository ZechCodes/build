// Verify the sidebar three-panel layout + activity behaviors.
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

  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  await page.click('.v2-channel-sidebar-item');
  await page.waitForTimeout(2000);

  // Expand all three.
  await page.evaluate(() => {
    // Force all sections to collapsed=false for baseline measurement.
    document.querySelectorAll('.v2-sidebar-section[data-section]').forEach(s => {
      if (s.dataset.section !== 'devices') s.classList.remove('collapsed');
    });
  });
  await page.waitForTimeout(200);

  // Measure heights of the 3 panels.
  const heights3 = await page.evaluate(() => {
    const sec = (n) => document.querySelector(`.v2-sidebar-section[data-section="${n}"]`);
    return {
      devices: sec('devices').getBoundingClientRect().height,
      tasks:   sec('tasks').getBoundingClientRect().height,
      activity: sec('activity').getBoundingClientRect().height,
    };
  });
  const avg3 = (heights3.devices + heights3.tasks + heights3.activity) / 3;
  const allClose = Object.values(heights3).every(h => Math.abs(h - avg3) < 8);
  check('three open panels share ~1/3 each', allClose, JSON.stringify(heights3));

  // Collapse activity → expect devices + tasks ~1/2 each.
  await page.evaluate(() => {
    document.querySelector('.v2-sidebar-section[data-section="activity"]').classList.add('collapsed');
  });
  await page.waitForTimeout(200);
  const heights2 = await page.evaluate(() => {
    const sec = (n) => document.querySelector(`.v2-sidebar-section[data-section="${n}"]`);
    return {
      devices: sec('devices').getBoundingClientRect().height,
      tasks:   sec('tasks').getBoundingClientRect().height,
      activity: sec('activity').getBoundingClientRect().height,
    };
  });
  check('activity collapsed shrinks to header', heights2.activity < 40, `h=${heights2.activity}`);
  const bigAvg = (heights2.devices + heights2.tasks) / 2;
  const twoClose = Math.abs(heights2.devices - bigAvg) < 10 && Math.abs(heights2.tasks - bigAvg) < 10;
  check('remaining two panels share ~1/2 each', twoClose, JSON.stringify(heights2));

  // Devices header should NOT toggle.
  const devicesOriginal = await page.$eval('.v2-sidebar-section[data-section="devices"]',
    el => el.classList.contains('collapsed'));
  await page.click('.v2-sidebar-section[data-section="devices"] .v2-sidebar-section-header');
  await page.waitForTimeout(100);
  const devicesAfter = await page.$eval('.v2-sidebar-section[data-section="devices"]',
    el => el.classList.contains('collapsed'));
  check('devices header is not toggleable', devicesOriginal === devicesAfter);

  // Seed activity + verify time format + ticker. Inject synthetic entries.
  await page.evaluate((id) => {
    const d = window.__v2debug;
    d.stores.activityStore.replace(id, [
      { type: 'text', created_at: new Date().toISOString(), data: { text: 'thinking fresh' } },
      { type: 'tool_use', created_at: new Date(Date.now() - 40 * 1000).toISOString(), data: { id: 'x', name: 'Read', input: { file_path: 'a.txt' } } },
      { type: 'tool_use', created_at: new Date(Date.now() - 5 * 60 * 1000).toISOString(), data: { id: 'y', name: 'Bash', input: { command: 'ls' } } },
      { type: 'tool_use', created_at: new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString(), data: { id: 'z', name: 'Write', input: { file_path: 'z.txt' } } },
    ]);
  }, await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id')));
  await page.waitForTimeout(300);

  // Expand activity.
  await page.evaluate(() => {
    document.querySelector('.v2-sidebar-section[data-section="activity"]').classList.remove('collapsed');
  });
  await page.waitForTimeout(300);

  const times = await page.$$eval('#v2-activity-body .v2-ce-time',
    els => els.map(el => el.textContent.trim()));
  console.log('  times:', times);
  check('labels use new format (no "ago", hh:mm past 1h)',
    times.some(t => /^\d+s$/.test(t) || t === 'now') &&
    times.some(t => /^\d+m$/.test(t)) &&
    times.some(t => /^\d\d:\d\d$/.test(t)),
    times.join(' | '));

  // Wait 1.2s and confirm the "Ns" timestamp has incremented (ticker).
  const before = times.find(t => /^\d+s$/.test(t)) || 'now';
  await page.waitForTimeout(1200);
  const after = await page.$$eval('#v2-activity-body .v2-ce-time',
    els => els.map(el => el.textContent.trim()));
  const afterSec = after.find(t => /^\d+s$/.test(t));
  check('time ticker advances every second', before !== afterSec, `${before} → ${afterSec}`);

  // Auto-scroll check: body should be scrolled to bottom.
  const scroll = await page.evaluate(() => {
    const b = document.querySelector('#v2-activity-body .v2-console-list');
    if (!b) return null;
    return { top: b.scrollTop, max: b.scrollHeight - b.clientHeight };
  });
  check('activity list auto-scrolls to latest', scroll && (scroll.max === 0 || (scroll.max - scroll.top) < 10),
    JSON.stringify(scroll));

  await page.screenshot({ path: 'sidebar.png', fullPage: false });
  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
