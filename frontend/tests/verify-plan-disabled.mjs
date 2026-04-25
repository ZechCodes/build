// Verify the resolved plan_review buttons:
//   - carry the `disabled` attribute,
//   - have `pointer-events: none` so clicks and hovers can't fire,
//   - don't pick up the hover background that the live variants use.
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

  // Seed a plan_review with resolved_at set so the card renders in
  // its resolved state (Approve as .selected, buttons disabled).
  const MSG_ID = 'plan-disabled-' + Date.now();
  await page.evaluate(({ chId, msgId }) => {
    window.__v2debug.stores.messagesStore.append(chId, {
      id: msgId, channel_id: chId, sender: 'Agent',
      content: 'Plan test',
      metadata: JSON.stringify({
        interaction_id: msgId,
        kind: 'plan_review',
        plan: 'do it',
        resolved_at: new Date().toISOString(),
        selected_option: 'approve',
      }),
      created_at: new Date().toISOString(),
    });
  }, { chId, msgId: MSG_ID });
  await page.waitForTimeout(200);

  const state = await page.evaluate((msgId) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-plan-card`);
    const approve = card?.querySelector('.v2-plan-btn.approve');
    const deny    = card?.querySelector('.v2-plan-btn.deny');
    const cs = (el) => getComputedStyle(el);
    return {
      approveDisabled: approve?.disabled,
      denyDisabled:    deny?.disabled,
      approvePointer:  approve ? cs(approve).pointerEvents : null,
      denyPointer:     deny    ? cs(deny).pointerEvents    : null,
    };
  }, MSG_ID);
  check('approve button is disabled',         state.approveDisabled === true, JSON.stringify(state));
  check('deny button is disabled',            state.denyDisabled === true);
  check('approve pointer-events = none',      state.approvePointer === 'none');
  check('deny pointer-events = none',         state.denyPointer === 'none');

  // Hover on the deny button — background shouldn't flip to the
  // hover red tint (because pointer-events: none suppresses hover).
  const preHoverBg = await page.evaluate((msgId) => {
    const deny = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-plan-btn.deny`);
    return getComputedStyle(deny).backgroundColor;
  }, MSG_ID);
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] .v2-plan-btn.deny`).hover({ force: true });
  await page.waitForTimeout(50);
  const afterHoverBg = await page.evaluate((msgId) => {
    const deny = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-plan-btn.deny`);
    return getComputedStyle(deny).backgroundColor;
  }, MSG_ID);
  check('deny hover background does not change when disabled',
    preHoverBg === afterHoverBg,
    `pre=${preHoverBg} after=${afterHoverBg}`);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
