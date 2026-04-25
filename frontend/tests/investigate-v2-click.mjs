// Click through v2: select channel, switch tabs, check each view renders.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

const errors = [];
page.on('pageerror', err => errors.push(`PAGE: ${err.message}`));
page.on('console', msg => {
  if (msg.type() === 'error') errors.push(`CONS: ${msg.text()}`);
});

await page.goto(`${BASE}/auth/dummy/login`, { waitUntil: 'domcontentloaded' });
await page.fill('input[name="email"]', EMAIL);
const nameInput = await page.$('input[name="name"]');
if (nameInput) await page.fill('input[name="name"]', 'Dev');
await Promise.all([
  page.waitForURL(u => !u.pathname.startsWith('/auth/'), { timeout: 10000 }).catch(() => null),
  page.click('button[type="submit"]'),
]);

await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);

// Wait for a channel to appear in the sidebar.
await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 }).catch(() => null);
const channels = await page.$$eval('.v2-channel-sidebar-item', rows =>
  rows.map(r => ({ id: r.getAttribute('data-channel-id'), name: r.textContent.trim() }))
);
console.log('Channels:', channels);
if (!channels.length) { console.log('NO CHANNELS — aborting'); await browser.close(); process.exit(1); }

// Click the first channel.
await page.click(`.v2-channel-sidebar-item[data-channel-id="${channels[0].id}"]`);
await page.waitForTimeout(2000);

async function snapshot(label) {
  const state = await page.evaluate(() => {
    const d = window.__v2debug;
    const active = d.stores.uiStore.getActiveChannel();
    return {
      tab: document.body.dataset.tab,
      hash: location.hash,
      activeChannel: active,
      messages: active ? d.stores.messagesStore.forChannel(active).length : 0,
      activity: active ? d.stores.activityStore.forChannel(active).length : 0,
      chat_innerHTML: document.getElementById('v2-tab-chat')?.innerHTML?.slice(0, 400) || '',
      files_innerHTML: document.getElementById('v2-tab-files')?.innerHTML?.slice(0, 400) || '',
      terminal_innerHTML: document.getElementById('v2-tab-terminal')?.innerHTML?.slice(0, 400) || '',
      rail_body: document.getElementById('v2-rail-body')?.innerHTML?.slice(0, 300) || '',
      topbar: document.getElementById('v2-top-bar')?.textContent?.trim() || '',
    };
  });
  console.log(`\n=== ${label} ===`);
  console.log(JSON.stringify(state, null, 2));
}

await snapshot('after channel click');

// Switch to chat
await page.click('.v2-tab-btn[data-tab="chat"]');
await page.waitForTimeout(800);
await snapshot('chat tab');

// Try sending a message
const sent = await page.evaluate(async () => {
  const input = document.querySelector('.v2-chat-input');
  if (!input) return { ok: false, reason: 'no input' };
  input.value = 'hello from v2 investigation';
  const send = document.querySelector('.v2-chat-send');
  send?.click();
  await new Promise(r => setTimeout(r, 300));
  return { ok: true };
});
console.log('Sent:', sent);
await page.waitForTimeout(1500);
await snapshot('after send');

// Switch to Files
await page.click('.v2-tab-btn[data-tab="files"]');
await page.waitForTimeout(1500);
await snapshot('files tab');

// Switch to Terminal
await page.click('.v2-tab-btn[data-tab="terminal"]');
await page.waitForTimeout(800);
await snapshot('terminal tab');

// Open rail
await page.click('.v2-rail-toggle');
await page.waitForTimeout(500);
await snapshot('rail open');

console.log('\n--- ERRORS ---');
for (const e of errors) console.log(e);

await page.screenshot({ path: 'v2-final.png', fullPage: false });
console.log('screenshot: v2-final.png');

await browser.close();
