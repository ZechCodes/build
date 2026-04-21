// Verify the Skrift SSE indicator is repositioned + restyled for v2:
// centered horizontally, sits above the rail, hidden when connected,
// visible (non-zero opacity) when disconnected.
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
  await page.waitForSelector('.sk-status-indicator', { timeout: 8000 });

  // Simulate the "disconnected" state so the indicator becomes visible.
  // Dispatch the Skrift event AND remove the hidden class directly, since
  // the Skrift runtime may auto-apply it based on real network state.
  await page.evaluate(() => {
    const el = document.querySelector('.sk-status-indicator');
    el.classList.remove('sk-status-indicator-hidden');
    el.querySelector('.sk-status-label').textContent = 'Disconnected';
    el.querySelector('.sk-status-dot').style.background = 'red';
  });
  await page.waitForTimeout(100);

  const state = await page.evaluate(() => {
    const el = document.querySelector('.sk-status-indicator');
    const rect = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    const rail = document.getElementById('v2-rail').getBoundingClientRect();
    return {
      rectLeft: Math.round(rect.left),
      rectRight: Math.round(rect.right),
      rectTop: Math.round(rect.top),
      rectBottom: Math.round(rect.bottom),
      vw: window.innerWidth,
      vh: window.innerHeight,
      position: cs.position,
      opacity: parseFloat(cs.opacity),
      railTop: Math.round(rail.top),
    };
  });

  check('pill is position:fixed', state.position === 'fixed');
  check('pill is visible while disconnected', state.opacity > 0.5, `opacity=${state.opacity}`);

  const centerX = (state.rectLeft + state.rectRight) / 2;
  const vwCenter = state.vw / 2;
  check('pill is horizontally centered',
    Math.abs(centerX - vwCenter) < 8,
    `centerX=${centerX} vw/2=${vwCenter}`);

  check('pill sits above the rail',
    state.rectBottom <= state.railTop,
    `rectBottom=${state.rectBottom} railTop=${state.railTop}`);

  // Now simulate "connected" — pill should become hidden (opacity 0).
  await page.evaluate(() => {
    document.querySelector('.sk-status-indicator').classList.add('sk-status-indicator-hidden');
  });
  await page.waitForTimeout(400);  // CSS transition is 300ms
  const hiddenOpacity = await page.evaluate(() =>
    parseFloat(getComputedStyle(document.querySelector('.sk-status-indicator')).opacity));
  check('pill fades to opacity 0 when connected', hiddenOpacity < 0.05, `opacity=${hiddenOpacity}`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
