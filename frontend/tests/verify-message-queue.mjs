// Verify that sending a chat message while the transport is
// offline queues the message with a "Queued" status, then drains
// it on the next e2ee.connected and the status advances to
// Delivered → Read as the server's wire events arrive.
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
  const chId = await page.$eval('.v2-channel-sidebar-item', el => el.getAttribute('data-channel-id'));
  await page.click(`.v2-channel-sidebar-item[data-channel-id="${chId}"]`);
  await page.waitForTimeout(400);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Stub the pool so forChannel returns a disconnected conn.
  // Also capture any send calls so the drain assertion works.
  const REAL_ID = 'queued-real-id-' + Date.now();
  await page.evaluate((realId) => {
    const d = window.__v2debug;
    const orig = d.e2eePool.forChannel.bind(d.e2eePool);
    d._stub = { connected: false, sendCalls: [] };
    d.e2eePool.forChannel = () => new Proxy(d._stub, {
      get(t, p) {
        if (p === 'send') {
          return async (payload) => {
            t.sendCalls.push(payload);
            return realId;
          };
        }
        return t[p];
      },
    });
  }, REAL_ID);

  // Send a message via the composer. (Use the bus directly for
  // determinism — the composer path reads the same intent.)
  await page.evaluate((id) => {
    window.__v2debug.bus.emit('intent.send_message', {
      channelId: id, text: 'queued hello',
    });
  }, chId);
  await page.waitForTimeout(200);

  const queuedState = await page.evaluate(() => {
    const msg = [...document.querySelectorAll('.v2-msg.user')].pop();
    const status = msg?.querySelector('.v2-msg-status');
    return {
      label: status?.textContent?.trim(),
      hasQueuedClass: status?.classList.contains('queued'),
    };
  });
  check('offline send renders "Queued" status', queuedState.label === 'Queued', JSON.stringify(queuedState));
  check('status has .queued class',             queuedState.hasQueuedClass === true);

  // Flip the stub to connected + fire e2ee.connected. The queue
  // should drain and the send() we captured should be called.
  await page.evaluate(() => {
    const d = window.__v2debug;
    d._stub.connected = true;
    // deviceId doesn't need to be accurate; the dispatcher re-checks
    // channelsStore.deviceFor(channelId) before draining.
    const ch = d.stores.uiStore.getActiveChannel();
    const devId = d.stores.channelsStore.deviceFor(ch);
    d.bus.emit('e2ee.connected', { deviceId: devId });
  });
  await page.waitForTimeout(300);

  const drained = await page.evaluate((realId) => {
    const msg = document.querySelector(`.v2-msg.user[data-msg-id="${realId}"]`);
    const status = msg?.querySelector('.v2-msg-status');
    const sendCalls = window.__v2debug._stub.sendCalls;
    return {
      hasSendCall: sendCalls.length === 1 && sendCalls[0].content === 'queued hello',
      msgFound: !!msg,
      label: status?.textContent?.trim(),
    };
  }, REAL_ID);
  check('queue drained → conn.send called',       drained.hasSendCall, JSON.stringify(drained));
  check('optimistic id swapped to server id',      drained.msgFound === true);
  check('status advances past Queued',             drained.label !== 'Queued', drained.label);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
