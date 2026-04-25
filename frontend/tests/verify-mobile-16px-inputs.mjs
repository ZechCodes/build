// Verify all text-entry controls compute to ≥16px at mobile widths.
// (iOS Safari auto-zooms focused inputs below 16px.)
// Also assert they stay at their smaller desktop sizes at 1440×900.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

async function doRun({ width, height, isMobile }) {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({
    viewport: { width, height },
    isMobile,
    hasTouch: isMobile,
  });
  const page = await ctx.newPage();

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

  // Open the sidebar drawer on mobile so the channel item is onscreen.
  if (isMobile) {
    await page.evaluate(() => document.querySelector('.v2-app')?.classList.add('sidebar-open'));
    await page.waitForTimeout(200);
  }
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.locator(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`).click({ force: true });
  await page.waitForTimeout(400);
  // Close the drawer so it doesn't obstruct the rail buttons.
  if (isMobile) {
    await page.evaluate(() => document.querySelector('.v2-app')?.classList.remove('sidebar-open'));
    await page.waitForTimeout(100);
  }

  // Open chat overlay (composer).
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.locator('#v2-rail-chat-toggle').click({ force: true });
    await page.waitForTimeout(300);
  }

  // Open terminal rail.
  await page.locator('#v2-rail-terminal-toggle').click({ force: true });
  await page.waitForTimeout(400);

  // "+ New Session" button to reveal the session-name input. Re-open
  // the drawer on mobile since we just closed it above.
  if (isMobile) {
    await page.evaluate(() => document.querySelector('.v2-app')?.classList.add('sidebar-open'));
    await page.waitForTimeout(100);
  }
  const sessionBtn = page.locator('button', { hasText: /New Session/i }).first();
  if (await sessionBtn.count()) {
    await sessionBtn.click({ force: true });
    await page.waitForTimeout(200);
  }

  const sizes = await page.evaluate(() => {
    const selectors = [
      '.v2-chat-input',
      '.v2-new-session-input',
      '.v2-term-input',
    ];
    const generics = [...document.querySelectorAll('input, textarea, select')]
      .filter(el => el.offsetParent !== null || el === document.activeElement);
    const out = [];
    for (const sel of selectors) {
      const el = document.querySelector(sel);
      if (el) out.push({
        sel,
        fs: parseFloat(getComputedStyle(el).fontSize),
        present: true,
      });
      else out.push({ sel, present: false });
    }
    for (const el of generics) {
      const id = el.id || el.name || el.placeholder || el.className.slice(0, 30) || el.tagName;
      out.push({
        sel: `[${el.tagName.toLowerCase()}] ${id}`,
        fs: parseFloat(getComputedStyle(el).fontSize),
        present: true,
      });
    }
    return out;
  });

  console.log(`\n=== ${isMobile ? 'MOBILE' : 'DESKTOP'} (${width}×${height}) ===`);
  for (const s of sizes) {
    if (!s.present) { console.log(`  [skip] ${s.sel} not in DOM`); continue; }
    console.log(`  ${s.fs}px  ${s.sel}`);
  }

  await browser.close();
  return sizes;
}

let passed = 0, failed = 0;
function check(label, ok, note) {
  if (ok) { passed++; console.log(`[PASS] ${label}${note ? ` — ${note}` : ''}`); }
  else    { failed++; console.log(`[FAIL] ${label}${note ? ` — ${note}` : ''}`); }
}

const mobile  = await doRun({ width: 375,  height: 812, isMobile: true  });
const desktop = await doRun({ width: 1440, height: 900, isMobile: false });

console.log('\n=== MOBILE CHECKS ===');
for (const s of mobile.filter(x => x.present && Number.isFinite(x.fs))) {
  check(`${s.sel} ≥ 16px`, s.fs >= 16, `${s.fs}px`);
}

console.log('\n=== DESKTOP SANITY ===');
// The v2-chat-input on desktop should stay 13px (not bumped to 16).
const desktopChat = desktop.find(s => s.sel === '.v2-chat-input' && s.present);
if (desktopChat) check('.v2-chat-input keeps 13px on desktop', desktopChat.fs === 13, `${desktopChat.fs}px`);
const desktopTerm = desktop.find(s => s.sel === '.v2-term-input' && s.present);
if (desktopTerm) check('.v2-term-input keeps 12.5px on desktop', Math.abs(desktopTerm.fs - 12.5) < 0.01, `${desktopTerm.fs}px`);

console.log(`\nResult: ${passed} passed, ${failed} failed`);
if (failed) process.exitCode = 1;
