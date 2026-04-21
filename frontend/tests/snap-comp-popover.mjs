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
await p.waitForTimeout(500);
// Close the chat overlay so it doesn't float over the complication area.
await p.evaluate(() => {
  const ov = document.getElementById('v2-chat-overlay');
  if (ov?.classList.contains('open')) document.getElementById('v2-rail-chat-toggle')?.click();
});
await p.waitForTimeout(200);
await p.evaluate((id) => {
  window.__v2debug.stores.complicationsStore.upsert(id, {
    id: 'git:/demo',
    kind: 'git-status',
    timestamp: Date.now(),
    data: {
      repo: '/demo', branch: 'main', upstream: 'origin/main',
      ahead: 2, behind: 0,
      insertions: 42, deletions: 7,
      staged:   { added: 1, modified: 2, deleted: 0, total: 3 },
      unstaged: { added: 0, modified: 3, deleted: 1, total: 4 },
      untracked: 1, conflicts: 0,
      remote_name: 'ZechCodes/build-web',
      last_fetch: Math.floor(Date.now() / 1000) - 180,
    },
    options: [
      { id: 'push',  label: 'Push',  enabled: true },
      { id: 'fetch', label: 'Fetch', enabled: true },
      { id: 'stash', label: 'Stash', enabled: true },
    ],
  });
}, chId);
await p.waitForTimeout(200);
await p.click('.v2-comp-git[data-comp-id="git:/demo"]');
await p.waitForTimeout(200);
const rect = await p.evaluate(() => {
  const chip = document.querySelector('.v2-comp-git[data-comp-id="git:/demo"]');
  // Only pick up the menu that belongs to THIS chip (important when the
  // real bridge-emitted git-status chips also render popovers).
  const menu = chip?.querySelector('.v2-comp-menu');
  return {
    chip: chip?.getBoundingClientRect().toJSON() ?? null,
    menu: menu?.getBoundingClientRect().toJSON() ?? null,
  };
});
if (!rect.chip) { console.log('no chip!'); await b.close(); process.exit(1); }
const leftMost  = Math.min(rect.chip.x, rect.menu?.x ?? rect.chip.x);
const topMost   = rect.menu?.y ?? rect.chip.y;
const bottomMost = rect.chip.y + rect.chip.height;
const x = Math.max(0, Math.round(leftMost) - 12);
const y = Math.max(0, Math.round(topMost) - 12);
const w = 440;
const h = Math.min(900 - y, Math.round(bottomMost - y) + 24);
await p.screenshot({ path: 'comp-popover.png', clip: { x, y, width: w, height: h } });
await p.screenshot({ path: 'comp-popover-full.png', fullPage: false });
console.log('wrote frontend/tests/comp-popover.png', { x, y, w, h });
console.log('menu text:', await p.evaluate(() => document.querySelector('.v2-comp-git[data-comp-id="git:/demo"] .v2-comp-menu')?.textContent?.slice(0, 300)));
console.log('menu rect:', await p.evaluate(() => {
  const m = document.querySelector('.v2-comp-git[data-comp-id="git:/demo"] .v2-comp-menu');
  return m?.getBoundingClientRect().toJSON();
}));
await b.close();
