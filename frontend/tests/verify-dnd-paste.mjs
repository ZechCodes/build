// Verify chat overlay drag/drop + image paste.
// We dispatch synthetic DragEvents and ClipboardEvents; this exercises
// the handlers without needing Playwright's native drag API.

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
  await page.click('.v2-channel-sidebar-item');
  await page.waitForTimeout(1500);
  // Ensure overlay is open.
  const overlayOpen = await page.$eval('#v2-chat-overlay', el => el.classList.contains('open'));
  if (!overlayOpen) { await page.click('#v2-rail-chat-toggle'); await page.waitForTimeout(400); }

  // 1) Drag-enter → overlay gets v2-chat-drag-active class.
  await page.evaluate(() => {
    const el = document.getElementById('v2-chat-overlay');
    const dt = new DataTransfer();
    // Mark the data types so our handler sees "Files".
    const file = new File(['hello'], 'dragged.txt', { type: 'text/plain' });
    dt.items.add(file);
    el.dispatchEvent(new DragEvent('dragenter', { bubbles: true, cancelable: true, dataTransfer: dt }));
  });
  await page.waitForTimeout(100);
  const active = await page.$eval('#v2-chat-overlay', el => el.classList.contains('v2-chat-drag-active'));
  check('drag-enter activates drop zone', active);

  // 2) Drop dispatches: file ends up in staging.
  await page.evaluate(() => {
    const el = document.getElementById('v2-chat-overlay');
    const dt = new DataTransfer();
    dt.items.add(new File(['hello'], 'dropped.txt', { type: 'text/plain' }));
    el.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
  });
  await page.waitForTimeout(150);
  const stagedDrop = await page.$$eval('.v2-staging-chip .v2-staging-name',
    els => els.map(e => e.textContent.trim()));
  check('dropped file is staged', stagedDrop.includes('dropped.txt'), stagedDrop.join(','));

  const cls = await page.$eval('#v2-chat-overlay', el => el.classList.contains('v2-chat-drag-active'));
  check('drop clears drag-active class', !cls);

  // 3) Paste image via synthetic ClipboardEvent on the composer.
  const imgUrl = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABAQMAAAAl21bKAAAAA1BMVEX///+nxBvIAAAACklEQVR4nGMAAQAABQAB4k+CqAAAAABJRU5ErkJggg==';
  await page.evaluate(async (dataUrl) => {
    const res = await fetch(dataUrl);
    const blob = await res.blob();
    const file = new File([blob], 'pasted.png', { type: blob.type });
    const dt = new DataTransfer();
    dt.items.add(file);
    const input = document.querySelector('.v2-chat-input');
    input.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData: dt }));
  }, imgUrl);
  await page.waitForTimeout(150);
  const stagedPaste = await page.$$eval('.v2-staging-chip .v2-staging-name',
    els => els.map(e => e.textContent.trim()));
  check('pasted image is staged', stagedPaste.includes('pasted.png'), stagedPaste.join(','));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
