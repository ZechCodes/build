// Mobile usability audit: drive common tasks and snapshot every state.
import { chromium } from 'playwright';
const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const b = await chromium.launch({ headless: true });
const ctx = await b.newContext({
  viewport: { width: 375, height: 812 },
  isMobile: true,
  hasTouch: true,
});
const p = await ctx.newPage();
const notes = [];
const log = (msg) => { console.log('[note]', msg); notes.push(msg); };

await p.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await p.fill('input[name="email"]', EMAIL);
const n = await p.$('input[name="name"]'); if (n) await p.fill('input[name="name"]', 'Dev');
await Promise.all([
  p.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(()=>null),
  p.click('button[type="submit"]'),
]);
await p.goto(`${BASE}/dashboard-v2/`, { waitUntil: 'domcontentloaded' });
await p.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

// ---- Initial state ----
const initial = await p.evaluate(() => {
  const path = document.getElementById('v2-path-bar-menu');
  const chat = document.getElementById('v2-chat-overlay');
  const rail = document.getElementById('v2-rail');
  const activeCh = window.__v2debug?.stores?.uiStore?.getActiveChannel();
  return {
    pathBarMenuRect: path?.getBoundingClientRect().toJSON(),
    chatOpen: chat?.classList.contains('open'),
    railState: document.querySelector('.v2-app')?.dataset.rail,
    activeChannel: activeCh,
    scrollWidth: document.documentElement.scrollWidth,
    viewportWidth: window.innerWidth,
  };
});
log(`initial viewport ${initial.viewportWidth}, page scrollWidth ${initial.scrollWidth} (overflow? ${initial.scrollWidth > initial.viewportWidth})`);
log(`initial chatOpen=${initial.chatOpen} railState=${initial.railState} activeChannel=${initial.activeChannel}`);
log(`hamburger rect: ${JSON.stringify(initial.pathBarMenuRect)}`);
await p.screenshot({ path: '/tmp/audit-01-initial.png' });

// ---- Try to land on a channel ----
await p.tap('#v2-path-bar-menu');
await p.waitForTimeout(200);
const chId = await p.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
await p.tap(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
await p.waitForTimeout(400);
await p.screenshot({ path: '/tmp/audit-02-channel-active.png' });

// ---- Open chat ----
await p.tap('#v2-rail-chat-toggle');
await p.waitForTimeout(500);
const chat = await p.evaluate(() => {
  const ov = document.getElementById('v2-chat-overlay');
  const composer = document.querySelector('#v2-chat-overlay .v2-chat-input-autosize, #v2-chat-overlay textarea');
  const body = document.querySelector('.v2-co-body .v2-chat-messages');
  return {
    overlayRect: ov?.getBoundingClientRect().toJSON(),
    composerRect: composer?.getBoundingClientRect().toJSON(),
    bodyRect: body?.getBoundingClientRect().toJSON(),
    bodyScrollHeight: body?.scrollHeight,
    bodyClientHeight: body?.clientHeight,
  };
});
log(`chat overlay: ${JSON.stringify(chat.overlayRect)}`);
log(`chat composer: ${JSON.stringify(chat.composerRect)}`);
log(`chat body: ${JSON.stringify(chat.bodyRect)} scroll=${chat.bodyScrollHeight} client=${chat.bodyClientHeight}`);
await p.screenshot({ path: '/tmp/audit-03-chat-open.png' });

// ---- Focus the composer (simulate mobile keyboard) ----
await p.locator('#v2-chat-overlay textarea').tap();
await p.waitForTimeout(300);
await p.screenshot({ path: '/tmp/audit-04-composer-focused.png' });

// ---- Scroll the chat body ----
await p.evaluate(() => {
  const body = document.querySelector('.v2-co-body .v2-chat-messages');
  if (body) body.scrollTop = 0;
});
await p.waitForTimeout(200);
await p.screenshot({ path: '/tmp/audit-05-chat-scrolled-top.png' });

// ---- Close chat, try the rail terminal ----
await p.tap('#v2-rail-chat-toggle');
await p.waitForTimeout(300);
await p.tap('#v2-rail-terminal-toggle');
await p.waitForTimeout(500);
const rail = await p.evaluate(() => {
  const railEl = document.getElementById('v2-rail');
  return {
    rect: railEl?.getBoundingClientRect().toJSON(),
    state: document.querySelector('.v2-app')?.dataset.rail,
    termBody: document.querySelector('#v2-rail-body')?.getBoundingClientRect().toJSON(),
  };
});
log(`rail after terminal tap: state=${rail.state} rail rect=${JSON.stringify(rail.rect)}`);
await p.screenshot({ path: '/tmp/audit-06-terminal.png' });

// ---- Tap tree chevron to open file tree drawer ----
await p.tap('#v2-rail-terminal-toggle');   // close terminal
await p.waitForTimeout(200);
await p.tap('#v2-path-bar-tree');
await p.waitForTimeout(300);
await p.screenshot({ path: '/tmp/audit-07-tree.png' });

// ---- Measure tap-target sizes for key controls ----
const sizes = await p.evaluate(() => {
  const ids = ['v2-path-bar-menu', 'v2-path-bar-tree', 'v2-sidebar-close', 'v2-rail-chat-toggle', 'v2-rail-terminal-toggle'];
  const out = {};
  for (const id of ids) {
    const el = document.getElementById(id);
    if (el) {
      const r = el.getBoundingClientRect();
      out[id] = { w: Math.round(r.width), h: Math.round(r.height) };
    } else out[id] = null;
  }
  return out;
});
log(`tap target sizes: ${JSON.stringify(sizes)}`);

await b.close();
console.log('\n=== NOTES ===');
notes.forEach(n => console.log('-', n));
