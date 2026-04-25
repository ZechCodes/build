// End-to-end check: clicking Approve/Deny on a plan_review actually
// dispatches over the wire (conn.sendInteractionResponse called with
// the right args). Spies on the pool's connFor to capture send calls.
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
  await page.waitForTimeout(500);
  if (!(await page.$eval('#v2-chat-overlay', el => el.classList.contains('open')))) {
    await page.click('#v2-rail-chat-toggle');
    await page.waitForTimeout(250);
  }

  // Proxy the pool's forChannel so every method call on the returned
  // conn gets logged to window.__calls. Return a real conn wrapper so
  // `connFor` sees it as connected.
  await page.evaluate(() => {
    window.__calls = [];
    const d = window.__v2debug;
    const origForChannel = d.e2eePool.forChannel.bind(d.e2eePool);
    d.e2eePool.forChannel = (id) => {
      const orig = origForChannel(id);
      if (!orig) {
        // No real conn; fabricate a minimal stub so the test can proceed.
        return new Proxy({ connected: true, send: () => 'stub', sendInteractionResponse: () => {} }, {
          get(t, p) {
            if (p in t) {
              return typeof t[p] === 'function'
                ? (...args) => { window.__calls.push({ method: p, args }); return t[p](...args); }
                : t[p];
            }
            return undefined;
          },
        });
      }
      return new Proxy(orig, {
        get(t, p) {
          const v = t[p];
          if (typeof v === 'function') {
            return (...args) => {
              window.__calls.push({ method: p, args });
              return v.apply(t, args);
            };
          }
          return v;
        },
      });
    };
  });

  // Seed a plan_review interaction message.
  const MSG = 'wire-plan-' + Date.now();
  const INT_ID = 'int-' + Date.now();
  await page.evaluate(({ chId, msgId, intId }) => {
    window.__v2debug.stores.messagesStore.append(chId, {
      id: msgId,
      channel_id: chId,
      sender: 'Agent',
      content: 'Review my plan please',
      metadata: JSON.stringify({
        interaction_id: intId,
        kind: 'plan_review',
        plan: 'Do the thing.',
      }),
      created_at: new Date().toISOString(),
    });
  }, { chId, msgId: MSG, intId: INT_ID });
  await page.waitForTimeout(150);

  // Click Approve.
  await page.locator(`.v2-msg[data-msg-id="${MSG}"] .v2-plan-btn.approve`).click();
  await page.waitForTimeout(300);

  const approveCalls = await page.evaluate(() =>
    (window.__calls || []).filter(c => c.method === 'sendInteractionResponse'));
  check('approve: sendInteractionResponse was invoked',
    approveCalls.length === 1, JSON.stringify(approveCalls));
  if (approveCalls.length) {
    const args = approveCalls[0].args;
    check('approve: channelId arg',      args[0] === chId,  String(args[0]));
    check('approve: interactionId arg',  args[1] === INT_ID, String(args[1]));
    check('approve: selectedOption arg', args[2] === 'approve', String(args[2]));
    check('approve: freeformResponse arg is null', args[3] === null,  String(args[3]));
    check('approve: selectedOptions arg is null',  args[4] === null,  String(args[4]));
  }

  // Seed another for Deny.
  const MSG2 = 'wire-plan-deny-' + Date.now();
  const INT2 = 'int2-' + Date.now();
  await page.evaluate(({ chId, msgId, intId }) => {
    window.__v2debug.stores.messagesStore.append(chId, {
      id: msgId,
      channel_id: chId,
      sender: 'Agent',
      content: 'Another plan',
      metadata: JSON.stringify({
        interaction_id: intId,
        kind: 'plan_review',
        plan: 'Another thing.',
      }),
      created_at: new Date().toISOString(),
    });
  }, { chId, msgId: MSG2, intId: INT2 });
  await page.waitForTimeout(150);
  await page.evaluate(() => { window.__calls = []; });  // reset
  await page.locator(`.v2-msg[data-msg-id="${MSG2}"] .v2-plan-btn.deny`).click();
  await page.waitForTimeout(300);
  const denyCalls = await page.evaluate(() =>
    (window.__calls || []).filter(c => c.method === 'sendInteractionResponse'));
  check('deny: sendInteractionResponse was invoked',
    denyCalls.length === 1, JSON.stringify(denyCalls));
  if (denyCalls.length) {
    check('deny: selectedOption === "reject"',
      denyCalls[0].args[2] === 'reject', String(denyCalls[0].args[2]));
  }

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
