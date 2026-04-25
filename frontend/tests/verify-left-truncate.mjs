// Verify image embed caption + viewer path bar truncate on the LEFT
// so the filename stays visible when the container is narrow.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const TINY_PNG_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

const LONG_PATH =
  '/Users/zechariahzimmerman/Projects/zech.sh/build/build-web/frontend/tests/very-narrow/deeply/nested/screenshot.png';

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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Inject a long-path image embed.
  const MSG_ID = 'verify-left-trunc-' + Date.now();
  await page.evaluate(({ id, msgId, b64, path }) => {
    const content = `<build-image path="${path}" mime="image/png">\n${b64}\n</build-image>`;
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId, channel_id: id, sender: 'Agent', content,
      created_at: new Date().toISOString(),
    });
  }, { id: chId, msgId: MSG_ID, b64: TINY_PNG_B64, path: LONG_PATH });
  await page.waitForTimeout(150);

  const capState = await page.evaluate((msgId) => {
    const el = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-embed-image-path`);
    return {
      direction: getComputedStyle(el).direction,
      textAlign: getComputedStyle(el).textAlign,
      textContent: el.textContent,
    };
  }, MSG_ID);
  check('image caption direction is rtl',          capState.direction === 'rtl', capState.direction);
  check('image caption text-align is left',        capState.textAlign === 'left', capState.textAlign);
  check('image caption carries the full path',     capState.textContent === LONG_PATH);

  // Click the image so the viewer-path shows something meaningful —
  // wait, the v2-path-text is what the Files view updates with the
  // selected file path. Let me just seed an all-files tree + select
  // a long path via the uiStore / viewState.
  await page.evaluate((longPath) => {
    // Directly set the path text to simulate a long-selected-file
    // state without having to drive the tree.
    const el = document.getElementById('v2-path-text');
    el.textContent = longPath;
    el.classList.remove('empty');
  }, LONG_PATH);
  await page.waitForTimeout(50);

  const pathState = await page.evaluate(() => {
    const el = document.getElementById('v2-path-text');
    return {
      direction: getComputedStyle(el).direction,
      textAlign: getComputedStyle(el).textAlign,
      textContent: el.textContent,
      ellipsis: getComputedStyle(el).textOverflow,
    };
  });
  check('viewer path-bar direction is rtl',   pathState.direction === 'rtl');
  check('viewer path-bar text-align is left', pathState.textAlign === 'left');
  check('viewer path-bar uses ellipsis',      pathState.ellipsis === 'ellipsis');

  // Also confirm the empty state still reads left-to-right.
  await page.evaluate(() => {
    const el = document.getElementById('v2-path-text');
    el.textContent = 'No file selected';
    el.classList.add('empty');
  });
  const emptyDir = await page.evaluate(() =>
    getComputedStyle(document.getElementById('v2-path-text')).direction);
  check('empty path-bar flips back to ltr', emptyDir === 'ltr', emptyDir);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
