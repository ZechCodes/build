// Dump raw v2 HTML to see what scripts are loaded.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
const page = await context.newPage();

await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await page.fill('input[name="email"]', EMAIL);
const nameInput = await page.$('input[name="name"]');
if (nameInput) await page.fill('input[name="name"]', 'Dev');
await Promise.all([
  page.waitForURL((u) => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  page.click('button[type="submit"]'),
]);

// Request the v2 HTML directly via fetch to see the raw server response.
const body = await page.evaluate(async () => {
  const r = await fetch('/dashboard/', { credentials: 'include' });
  return await r.text();
});

console.log(body);

await browser.close();
