// Diagnose whether suggested replies render + behave on the client.
// If this passes the client side works; any absence in the real UI
// is then bridge-side (the agent just isn't attaching
// `suggested_actions` to its chat.response).
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
  await page.goto(`${BASE}/dashboard/`, { waitUntil: 'domcontentloaded' });
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  const MSG_ID = 'sugg-' + Date.now();
  await page.evaluate(({ id, msgId }) => {
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId,
      channel_id: id,
      sender: 'Agent',
      content: 'Ready to start?',
      suggested_actions: ['Go ahead', 'Hold on'],
      created_at: new Date().toISOString(),
    });
  }, { id: chId, msgId: MSG_ID });
  await page.waitForTimeout(200);

  const rendered = await page.evaluate((msgId) => {
    const buttons = [...document.querySelectorAll(`.v2-msg[data-msg-id="${msgId}"] .v2-suggestion`)];
    return buttons.map(b => b.getAttribute('data-suggestion'));
  }, MSG_ID);
  check('two suggestion buttons render',  rendered.length === 2, JSON.stringify(rendered));
  check('first suggestion label',          rendered[0] === 'Go ahead');
  check('second suggestion label',         rendered[1] === 'Hold on');

  // Clicking a suggestion should fill composer + attempt to send.
  // Spy on intent.send_message to confirm.
  await page.evaluate(() => {
    window.__lastSend = null;
    window.__v2debug.bus.on('intent.send_message', (p) => { window.__lastSend = p; });
  });
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] .v2-suggestion[data-suggestion="Go ahead"]`).click();
  await page.waitForTimeout(200);

  const after = await page.evaluate((msgId) => {
    const buttons = [...document.querySelectorAll(`.v2-msg[data-msg-id="${msgId}"] .v2-suggestion`)];
    return {
      selected: buttons.find(b => b.classList.contains('selected'))?.getAttribute('data-suggestion'),
      dismissed: buttons.find(b => b.classList.contains('dismissed'))?.getAttribute('data-suggestion'),
      lastSend: window.__lastSend?.text,
    };
  }, MSG_ID);
  check('click marks chosen suggestion .selected',  after.selected === 'Go ahead', JSON.stringify(after));
  check('click marks sibling .dismissed',            after.dismissed === 'Hold on');
  check('click fires intent.send_message with text', after.lastSend === 'Go ahead');

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
