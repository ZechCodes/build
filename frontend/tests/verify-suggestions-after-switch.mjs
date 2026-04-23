// Regression: clicking a suggested reply used to silently no-op
// after the user had switched channels. ChatView.deactivate()
// wasn't removing the click listener it registered on
// #v2-chat-overlay-body in _buildShell, so after a channel switch
// two listeners fired on each click — the stale one marked the
// button `.selected` using old `this`, short-circuiting the fresh
// listener's guard. Page reload cleared the DOM and fixed it.
//
// Verify by:
//   1. activating channel A, 2. switching to channel B, 3. back
//   to A, 4. appending a suggestion, 5. clicking it — expect
//   exactly one intent.send_message fire with the suggestion text.
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
  await page.waitForSelector('.v2-channel-sidebar-item', { timeout: 8000 });

  const ids = await page.$$eval('.v2-sidebar .v2-channel-sidebar-item',
    els => els.map(el => el.getAttribute('data-channel-id')).filter(Boolean));
  if (ids.length < 2) {
    console.log('[SKIP] need at least two channels on this device');
    console.log(`\nResult: 0 passed, 0 failed`);
    process.exit(0);
  }
  const idA = ids[0];
  const idB = ids[1];

  // Activate A, B, A — this is the exact deactivate/activate cycle
  // that leaked the click listener pre-fix.
  await page.evaluate((id) => window.__v2debug.stores.uiStore.setActiveChannel(id), idA);
  await page.waitForTimeout(250);
  await page.evaluate((id) => window.__v2debug.stores.uiStore.setActiveChannel(id), idB);
  await page.waitForTimeout(250);
  await page.evaluate((id) => window.__v2debug.stores.uiStore.setActiveChannel(id), idA);
  await page.waitForTimeout(400);

  // Open chat overlay on A.
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Append a suggestion-bearing message to A.
  const MSG_ID = 'sugg-switch-' + Date.now();
  await page.evaluate(({ id, msgId }) => {
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId, channel_id: id, sender: 'Agent',
      content: 'Pick one',
      suggested_actions: ['Yes please', 'No thanks'],
      created_at: new Date().toISOString(),
    });
  }, { id: idA, msgId: MSG_ID });
  await page.waitForTimeout(200);

  // Count intent.send_message fires so a doubled listener would show
  // up as >1.
  await page.evaluate(() => {
    window.__sendFires = [];
    window.__v2debug.bus.on('intent.send_message', (p) => { window.__sendFires.push(p); });
  });

  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] .v2-suggestion[data-suggestion="Yes please"]`).click();
  await page.waitForTimeout(200);

  const result = await page.evaluate((msgId) => {
    const btn = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-suggestion[data-suggestion="Yes please"]`);
    return {
      selected: btn?.classList.contains('selected'),
      fires:    window.__sendFires.map(p => p.text),
    };
  }, MSG_ID);

  check('suggestion is marked .selected after click',   result.selected === true, JSON.stringify(result));
  check('click fires intent.send_message exactly once', result.fires.length === 1, JSON.stringify(result.fires));
  check('send message payload is the suggestion text',   result.fires[0] === 'Yes please');

  // A second message arrives with suggestions after the user has
  // already sent a reply — the heuristic in _dismissStaleSuggestions
  // dims them, but the user should still be able to click.
  const MSG2_ID = 'sugg-dim-' + Date.now();
  await page.evaluate(({ id, msgId }) => {
    const s = window.__v2debug.stores.messagesStore;
    // Simulate the "another manual reply after suggestion" flow so the
    // dismissed heuristic kicks in on the NEW suggestion too: append
    // an agent suggestion, then a client message AFTER it.
    s.append(id, {
      id: msgId, channel_id: id, sender: 'Agent',
      content: 'Second chance?',
      suggested_actions: ['Sure', 'Pass'],
      created_at: new Date().toISOString(),
    });
    s.append(id, {
      id: msgId + '-reply', channel_id: id, sender: 'client',
      content: 'just thinking',
      created_at: new Date().toISOString(),
    });
    window.__sendFires = [];
  }, { id: idA, msgId: MSG2_ID });
  await page.waitForTimeout(200);

  const preDim = await page.evaluate((msgId) => {
    const btn = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-suggestion[data-suggestion="Sure"]`);
    return btn?.classList.contains('dismissed');
  }, MSG2_ID);
  check('stale-suggestion heuristic dims Sure', preDim === true);

  await page.locator(`.v2-msg[data-msg-id="${MSG2_ID}"] .v2-suggestion[data-suggestion="Sure"]`).click();
  await page.waitForTimeout(150);
  const dimmedClick = await page.evaluate(() => window.__sendFires.map(p => p.text));
  check('dimmed suggestion is still clickable',
    dimmedClick.length === 1 && dimmedClick[0] === 'Sure',
    JSON.stringify(dimmedClick));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
