// Verify there's no body-level scroll on mobile and that the rail sits
// flush with the bottom of the viewport.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

const browser = await chromium.launch({ headless: true });
const ctx = await browser.newContext({
  viewport: { width: 375, height: 812 },
  isMobile: true,
  hasTouch: true,
});
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

  // Activate a channel so everything is mounted.
  await page.evaluate(() => document.querySelector('.v2-app')?.classList.add('sidebar-open'));
  await page.waitForTimeout(150);
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.locator(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`).click({ force: true });
  await page.evaluate(() => document.querySelector('.v2-app')?.classList.remove('sidebar-open'));
  await page.waitForTimeout(400);

  const state = await page.evaluate(() => {
    const rail = document.getElementById('v2-rail');
    const railRect = rail?.getBoundingClientRect();
    return {
      bodyScrollHeight: document.body.scrollHeight,
      bodyClientHeight: document.body.clientHeight,
      docScrollHeight:  document.documentElement.scrollHeight,
      docClientHeight:  document.documentElement.clientHeight,
      viewportHeight: window.innerHeight,
      bodyOverflow: getComputedStyle(document.body).overflow,
      htmlOverflow: getComputedStyle(document.documentElement).overflow,
      railBottom: Math.round(railRect?.bottom ?? -1),
    };
  });

  check('body overflow is hidden', state.bodyOverflow === 'hidden', state.bodyOverflow);
  check('html overflow is hidden', state.htmlOverflow === 'hidden', state.htmlOverflow);
  check('body scrollHeight does not exceed clientHeight',
        state.bodyScrollHeight <= state.bodyClientHeight + 1,
        `scroll=${state.bodyScrollHeight} client=${state.bodyClientHeight}`);
  check('doc scrollHeight does not exceed clientHeight',
        state.docScrollHeight <= state.docClientHeight + 1,
        `scroll=${state.docScrollHeight} client=${state.docClientHeight}`);
  check('rail is flush with viewport bottom',
        Math.abs(state.railBottom - state.viewportHeight) <= 1,
        `railBottom=${state.railBottom} vh=${state.viewportHeight}`);

  // Verify window can't actually scroll.
  const scrolled = await page.evaluate(() => {
    window.scrollTo(0, 200);
    return window.scrollY;
  });
  check('window.scrollY stays 0 after scrollTo(0, 200)', scrolled === 0, String(scrolled));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
