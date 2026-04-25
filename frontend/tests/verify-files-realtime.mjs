// Verify FilesView reacts to agent.file_changes — debounced refetch of
// files_changes, file_diff/file_read for currently-viewed path.

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
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(1500);

  // Intercept every BuildE2EE.send() so we can assert outgoing actions.
  await page.evaluate(() => {
    window.__sent = [];
    const pool = window.__v2debug.e2eePool;
    for (const inst of pool._instances.values()) {
      const orig = inst.send.bind(inst);
      inst.send = (payload) => { window.__sent.push(payload); return orig(payload); };
    }
  });

  // 1) Fire two agent.file_changes in quick succession — should coalesce.
  await page.evaluate((id) => {
    window.__sent = [];
    window.__v2debug.bus.emit('agent.file_changes', { channelId: id, paths: ['a.txt'] });
    window.__v2debug.bus.emit('agent.file_changes', { channelId: id, paths: ['b.txt'] });
  }, chId);
  await page.waitForTimeout(400);    // > 150ms debounce
  const changesCount = await page.evaluate(() =>
    window.__sent.filter(p => p.action === 'files_changes').length
  );
  check('repeated file_changes within 150ms coalesce to one files_changes', changesCount === 1,
    `count=${changesCount}`);

  // 2) Open a file → source view; fire agent.file_changes for that file →
  //    expect a file_read.
  const filePath = 'README.md';
  await page.evaluate((path) => {
    // Simulate click on the Modified tree row by directly dispatching.
    const row = document.querySelector(`[data-file-path="${path}"]`);
    if (row) row.click();
  }, filePath);
  await page.waitForTimeout(400);

  await page.evaluate((id) => {
    window.__sent = [];
    window.__v2debug.bus.emit('agent.file_changes', { channelId: id, paths: ['README.md'] });
  }, chId);
  await page.waitForTimeout(400);
  const trail = await page.evaluate(() => window.__sent.map(p => p.action));
  check('touched current file triggers files_changes',  trail.includes('files_changes'), trail.join(','));
  // Either file_read (source mode) or file_diff (diff mode) is acceptable.
  check('touched current file triggers file_read or file_diff',
    trail.includes('file_read') || trail.includes('file_diff'), trail.join(','));

  // 3) Path not matching → only files_changes, no file_read.
  await page.evaluate((id) => {
    window.__sent = [];
    window.__v2debug.bus.emit('agent.file_changes', { channelId: id, paths: ['unrelated.py'] });
  }, chId);
  await page.waitForTimeout(400);
  const trail3 = await page.evaluate(() => window.__sent.map(p => p.action));
  check('non-touching change → only files_changes', trail3.includes('files_changes') && !trail3.includes('file_read') && !trail3.includes('file_diff'),
    trail3.join(','));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
