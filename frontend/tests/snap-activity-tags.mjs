// Seeds synthetic activity entries and screenshots the activity panel
// so the new outline tag styling can be eyeballed.
import { chromium } from 'playwright';
const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const b = await chromium.launch({ headless: true });
const ctx = await b.newContext({ viewport: { width: 1440, height: 900 } });
const p = await ctx.newPage();
await p.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await p.fill('input[name="email"]', EMAIL);
const n = await p.$('input[name="name"]'); if (n) await p.fill('input[name="name"]', 'Dev');
await Promise.all([
  p.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(()=>null),
  p.click('button[type="submit"]'),
]);
await p.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
await p.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
await p.waitForTimeout(800);

// Make sure activity panel is open.
await p.evaluate(() => {
  const head = [...document.querySelectorAll('.v2-sidebar-section-header')]
    .find(h => h.textContent.toLowerCase().includes('activity'));
  const section = head?.closest('.v2-sidebar-section');
  if (section && !section.classList.contains('open')) head?.click();
});

// Seed a few synthetic activity entries so every tag style is visible.
await p.evaluate((id) => {
  const s = window.__v2debug.stores.activityStore;
  const now = Date.now();
  const tu = (off, name, input, toolUseId) =>
    s.appendToolUse(id, { toolUseId, name, input, at: new Date(now - off).toISOString() });
  tu(5000, 'Read',  { file_path: 'src/app.js' }, 't1');
  tu(4000, 'Grep',  { pattern: 'foo' },          't2');
  tu(3500, 'Glob',  { pattern: '**/*.ts' },      't3');
  tu(3000, 'Edit',  { file_path: 'a.js' },       't4');
  tu(2000, 'Write', { file_path: 'new.md' },     't5');
  tu(1000, 'Bash',  { command: 'ls -la' },       't6');
  tu(600,  'Agent', { description: 'do a thing' }, 't7');
  s.appendReasoning(id, 'thinking about the change…', new Date(now - 200).toISOString());
}, chId);
await p.waitForTimeout(300);
// Collapse Tasks so Activity has the whole lower half of the sidebar.
await p.evaluate(() => {
  const head = [...document.querySelectorAll('.v2-sidebar-section-header')]
    .find(h => h.textContent.toLowerCase().includes('tasks'));
  const section = head?.closest('.v2-sidebar-section');
  if (section?.classList.contains('open')) head?.click();
});
await p.waitForTimeout(200);
await p.screenshot({ path: 'activity-tags.png', clip: { x: 0, y: 0, width: 320, height: 900 } });
console.log('wrote frontend/tests/activity-tags.png');
await b.close();
