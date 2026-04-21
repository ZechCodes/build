// Wave 6b smoke — plan toggle + compact confirm + file upload + review toast.
// Run against compose at localhost:8100 with dev@local.

import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

const pageErrors = [];
page.on('pageerror', (err) => pageErrors.push(err.message));
page.on('console', (msg) => { if (msg.type() === 'error') pageErrors.push(`CONS: ${msg.text()}`); });

try {
  await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
  await page.fill('input[name="email"]', EMAIL);
  const nameInput = await page.$('input[name="name"]');
  if (nameInput) await page.fill('input[name="name"]', 'Dev');
  await Promise.all([
    page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
    page.click('button[type="submit"]'),
  ]);

  await page.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(3000);
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

  // Intercept every BuildE2EE.send() call so we can assert outgoing payloads.
  // Also stub uploadFile so the chat composer test doesn't depend on the real
  // device round-tripping chunk acks (which can exceed 10 s in CI).
  await page.evaluate(() => {
    window.__sent = [];
    const pool = window.__v2debug.e2eePool;
    for (const inst of pool._instances.values()) {
      const origSend = inst.send.bind(inst);
      inst.send = (payload) => { window.__sent.push(payload); return origSend(payload); };
      inst.uploadFile = async (channelId, file) => ({
        file_id: 'fake-' + file.name,
        filename: file.name,
        size: file.size,
        mime_type: file.type || 'application/octet-stream',
        path: `/uploads/${file.name}`,
      });
    }
  });

  // Select first channel.
  const firstId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${firstId}"]`);
  // Open chat tab.
  await page.click('.v2-tab-btn[data-tab="chat"]');
  await page.waitForTimeout(400);

  // 1) Plan mode toggle
  await page.click('[data-cmd="plan"]');
  const planActive = await page.$eval('[data-cmd="plan"]', el => el.classList.contains('active'));
  check('plan button active after click', planActive);
  const planStored = await page.evaluate((id) => window.__v2debug.stores.presenceStore.get(id).planMode, firstId);
  check('presenceStore.planMode true', planStored === true);

  // Send a message — should carry plan_mode: true.
  const plainText = `wave6b plan test ${Date.now()}`;
  await page.fill('.v2-chat-input', plainText);
  await page.click('.v2-chat-send');
  await page.waitForTimeout(1000);
  const planSend = await page.evaluate((t) =>
    window.__sent.find(p => p.action === 'message' && p.content === t),
    plainText);
  check('outgoing payload includes plan_mode', planSend && planSend.plan_mode === true, JSON.stringify(planSend?.plan_mode));

  // Turn plan mode off.
  await page.click('[data-cmd="plan"]');
  const planOffStored = await page.evaluate((id) => window.__v2debug.stores.presenceStore.get(id).planMode, firstId);
  check('plan mode toggles off', planOffStored === false);

  // 2) Compact confirm pattern — first click shows "Click to confirm"
  const compactBtn = '[data-cmd-confirm="compact"]';
  await page.click(compactBtn);
  const compactLabel = await page.$eval(`${compactBtn} .v2-cmd-label`, el => el.textContent);
  check('compact first-click shows Click to confirm', compactLabel === 'Click to confirm', compactLabel);
  // Second click within 3 s — sends compact_session.
  await page.click(compactBtn);
  await page.waitForTimeout(600);
  const compactSent = await page.evaluate(() =>
    window.__sent.find(p => p.action === 'compact_session'));
  check('compact_session payload sent after confirm', !!compactSent);

  // 3) Attach file — use setInputFiles on the hidden input.
  const payload = 'hello from wave6b smoke';
  await page.setInputFiles('.v2-chat-file-input', {
    name: 'hello.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from(payload),
  });
  await page.waitForTimeout(200);
  const stagedName = await page.$eval('.v2-staging-chip .v2-staging-name', el => el.textContent.trim());
  check('file chip appears after pick', stagedName === 'hello.txt', stagedName);

  // Send — file should upload and be attached.
  await page.fill('.v2-chat-input', 'carrying hello.txt');
  await page.click('.v2-chat-send');
  // Wait up to 10 s for the message payload to appear (chunked upload).
  let attachedSend = null;
  for (let i = 0; i < 50; i++) {
    attachedSend = await page.evaluate(() =>
      window.__sent.find(p => p.action === 'message' && p.content === 'carrying hello.txt'));
    if (attachedSend) break;
    await page.waitForTimeout(200);
  }
  if (!attachedSend) {
    const trail = await page.evaluate(() => window.__sent.map(p => p.action));
    console.log('  __sent actions:', trail.join(', '));
  }
  check('outgoing message has attachments',
    !!attachedSend && Array.isArray(attachedSend.attachments) && attachedSend.attachments.length === 1,
    JSON.stringify(attachedSend?.attachments));

  // Staging should clear.
  const stagingHiddenAfter = await page.$eval('.v2-chat-staging', el => el.hidden);
  check('staging clears after send', stagingHiddenAfter);

  // 4) Review buttons — switch to files and click Approve all / View PR.
  await page.click('.v2-tab-btn[data-tab="files"]');
  await page.waitForTimeout(400);
  // Select a file so the mode bar has the review buttons rendered.
  const firstFile = await page.$('[data-file-path]');
  if (firstFile) {
    await firstFile.click();
    await page.waitForTimeout(400);
  }
  const hasReviewBtn = await page.$('[data-review="approve-all"]') !== null;
  check('Approve all button present in mode bar', hasReviewBtn);
  if (hasReviewBtn) {
    await page.click('[data-review="approve-all"]');
    await page.waitForTimeout(200);
    const toastText = await page.$eval('.v2-toast-item', el => el.textContent).catch(() => '');
    check('toast appears after review click', toastText.includes('not wired up'));
  }

  await page.screenshot({ path: 'wave6b.png', fullPage: false });
  console.log(`\nResult: ${passed} passed, ${failed} failed, ${pageErrors.length} page errors`);
  for (const e of pageErrors) console.log(`  ${e}`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
