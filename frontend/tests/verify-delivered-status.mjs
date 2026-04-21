// Verify user-message status updates in real time:
//   1. Optimistic insert renders "Sending" immediately after submit.
//   2. A simulated `message.delivered` bus event flips it to "Delivered".
//   3. A simulated `message.read` bus event flips it to "Read".
//
// To keep this deterministic we stub `e2eePool.forChannel(...).send` so it
// returns a known id synchronously, then emit the wire events directly on
// the bus. No real transport round-trip — we're validating that v2 wires
// the optimistic row to the server-assigned id so subsequent events land.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await context.newPage();

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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(800);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(300);
  }

  // Stub the e2ee conn: capture send payloads and return a known id so we
  // can target it with the simulated delivered/read events.
  const REAL_ID = 'test-real-id-' + Date.now();
  await page.evaluate((rid) => {
    const d = window.__v2debug;
    const orig = d.e2eePool.forChannel.bind(d.e2eePool);
    d._origForChannel = orig;
    d.e2eePool.forChannel = (id) => {
      const conn = orig(id);
      if (!conn) return conn;
      return new Proxy(conn, {
        get(t, p) {
          if (p === 'send') return async (_payload) => rid;
          return t[p];
        },
      });
    };
  }, REAL_ID);

  // Submit a user message via the intent bus (what the composer does).
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('intent.send_message', {
      channelId: id,
      text: 'hello from verify-delivered-status',
    });
  }, chId);
  // Let the optimistic append paint.
  await page.waitForTimeout(150);

  const optimisticRow = await page.evaluate(() => {
    const nodes = [...document.querySelectorAll('.v2-msg.user .v2-msg-status')];
    const last = nodes[nodes.length - 1];
    return last ? { cls: last.className, text: last.textContent?.trim() } : null;
  });
  check('optimistic user row renders with Sending',
    optimisticRow && optimisticRow.text === 'Sending' && /sending/.test(optimisticRow.cls),
    JSON.stringify(optimisticRow));

  // Wait for the stubbed send() to resolve + replaceId to fire.
  await page.waitForTimeout(200);
  const rowAfterSwap = await page.evaluate((rid) => {
    const el = document.querySelector(`.v2-msg.user[data-msg-id="${rid}"]`);
    return el ? { id: el.getAttribute('data-msg-id') } : null;
  }, REAL_ID);
  check('optimistic row adopts real id after conn.send() resolves',
    rowAfterSwap && rowAfterSwap.id === REAL_ID,
    JSON.stringify(rowAfterSwap));

  // Simulate delivered event (what BuildE2EE would emit on server ack).
  await page.evaluate(({ id, rid }) => {
    window.__v2debug.bus.emit('message.delivered', { channelId: id, msgId: rid });
  }, { id: chId, rid: REAL_ID });
  await page.waitForTimeout(80);
  const afterDelivered = await page.evaluate((rid) => {
    const el = document.querySelector(`.v2-msg.user[data-msg-id="${rid}"] .v2-msg-status`);
    return el ? { cls: el.className, text: el.textContent?.trim() } : null;
  }, REAL_ID);
  check('row flips to Delivered on message.delivered',
    afterDelivered && afterDelivered.text === 'Delivered' && /delivered/.test(afterDelivered.cls),
    JSON.stringify(afterDelivered));

  // Simulate read event.
  await page.evaluate(({ id, rid }) => {
    window.__v2debug.bus.emit('message.read', { channelId: id, msgIds: [rid] });
  }, { id: chId, rid: REAL_ID });
  await page.waitForTimeout(80);
  const afterRead = await page.evaluate((rid) => {
    const el = document.querySelector(`.v2-msg.user[data-msg-id="${rid}"] .v2-msg-status`);
    return el ? { cls: el.className, text: el.textContent?.trim() } : null;
  }, REAL_ID);
  check('row flips to Read on message.read',
    afterRead && afterRead.text === 'Read' && /\bread\b/.test(afterRead.cls),
    JSON.stringify(afterRead));

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
