// Verify that clicking an option on a plan_review / question /
// tool-approval interaction card highlights the chosen option
// (optimistic update in messagesStore, no dependency on the server
// echo coming back in real time).
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

async function seedInteraction(page, chId, msgId, meta, content = '') {
  await page.evaluate(({ chId, msg }) => {
    window.__v2debug.stores.messagesStore.append(chId, msg);
  }, {
    chId,
    msg: {
      id: msgId,
      channel_id: chId,
      sender: 'Agent',
      content: content || 'Pick one',
      metadata: JSON.stringify(meta),
      created_at: new Date().toISOString(),
    },
  });
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

  // ---- 1. plan_review → Approve ----
  const PLAN_APPROVE = 'int-plan-approve';
  await seedInteraction(page, chId, PLAN_APPROVE, {
    interaction_id: PLAN_APPROVE,
    kind: 'plan_review',
    plan: 'Refactor the foo module.',
  }, "Here's the plan");
  await page.waitForTimeout(120);
  await page.locator(`.v2-msg[data-msg-id="${PLAN_APPROVE}"] .v2-plan-btn.approve`).click();
  await page.waitForTimeout(150);
  const approve = await page.evaluate((id) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${id}"] .v2-plan-card`);
    const approveBtn = card?.querySelector('.v2-plan-btn.approve');
    const denyBtn = card?.querySelector('.v2-plan-btn.deny');
    return {
      approveSelected: approveBtn?.classList.contains('selected'),
      denySelected: denyBtn?.classList.contains('selected'),
      approveDisabled: approveBtn?.disabled,
      denyDisabled: denyBtn?.disabled,
    };
  }, PLAN_APPROVE);
  check('plan approve: Approve is .selected',  approve.approveSelected === true, JSON.stringify(approve));
  check('plan approve: Deny is NOT selected',  approve.denySelected === false);
  check('plan approve: both buttons disabled', approve.approveDisabled && approve.denyDisabled);

  // ---- 2. plan_review → Deny ----
  const PLAN_DENY = 'int-plan-deny';
  await seedInteraction(page, chId, PLAN_DENY, {
    interaction_id: PLAN_DENY,
    kind: 'plan_review',
    plan: 'Rewrite auth.',
  });
  await page.waitForTimeout(120);
  await page.locator(`.v2-msg[data-msg-id="${PLAN_DENY}"] .v2-plan-btn.deny`).click();
  await page.waitForTimeout(150);
  const deny = await page.evaluate((id) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${id}"] .v2-plan-card`);
    const approveBtn = card?.querySelector('.v2-plan-btn.approve');
    const denyBtn = card?.querySelector('.v2-plan-btn.deny');
    return {
      approveSelected: approveBtn?.classList.contains('selected'),
      denySelected: denyBtn?.classList.contains('selected'),
    };
  }, PLAN_DENY);
  check('plan deny: Deny is .selected',       deny.denySelected === true, JSON.stringify(deny));
  check('plan deny: Approve is NOT selected', deny.approveSelected === false);

  // ---- 3. question / single-select option ----
  const QUESTION = 'int-question';
  await seedInteraction(page, chId, QUESTION, {
    interaction_id: QUESTION,
    kind: 'question',
    options: [
      { id: 'yes', label: 'Yes' },
      { id: 'no',  label: 'No'  },
    ],
    allow_freeform: false,
  }, 'Proceed?');
  await page.waitForTimeout(120);
  await page.locator(`.v2-msg[data-msg-id="${QUESTION}"] .v2-int-opt[data-opt-id="yes"]`).click();
  await page.waitForTimeout(150);
  const question = await page.evaluate((id) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${id}"] .v2-int-card`);
    return {
      resolved: card?.classList.contains('resolved'),
      yesSelected: card?.querySelector('[data-opt-id="yes"]')?.classList.contains('selected'),
      noSelected:  card?.querySelector('[data-opt-id="no"]')?.classList.contains('selected'),
      freeformGone: !card?.querySelector('.v2-int-freeform'),
    };
  }, QUESTION);
  check('question: card is resolved',        question.resolved === true,   JSON.stringify(question));
  check('question: Yes is .selected',        question.yesSelected === true);
  check('question: No is NOT selected',      question.noSelected === false);
  check('question: freeform form is gone',   question.freeformGone === true);

  // ---- 4. tool approval with freeform text ----
  const TOOL = 'int-tool';
  await seedInteraction(page, chId, TOOL, {
    interaction_id: TOOL,
    kind: 'tool_approval',
    options: [
      { id: 'approve', label: 'Approve' },
      { id: 'deny',    label: 'Deny'    },
    ],
    allow_freeform: true,
  }, 'Run rm -rf /tmp/x?');
  await page.waitForTimeout(120);
  // Type freeform then click Send (no option selected — just text).
  const freeformText = 'only if it is safe';
  await page.locator(`.v2-msg[data-msg-id="${TOOL}"] .v2-int-freeform textarea`).fill(freeformText);
  await page.locator(`.v2-msg[data-msg-id="${TOOL}"] .v2-int-submit`).click();
  await page.waitForTimeout(150);
  const tool = await page.evaluate((id) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${id}"] .v2-int-card`);
    const ff = card?.querySelector('.v2-int-freeform-response');
    return {
      resolved: card?.classList.contains('resolved'),
      freeformResponse: ff?.textContent?.trim(),
      freeformFormGone: !card?.querySelector('.v2-int-freeform'),
    };
  }, TOOL);
  check('tool: card is resolved',             tool.resolved === true,  JSON.stringify(tool));
  check('tool: freeform text echoes back',    tool.freeformResponse === freeformText);
  check('tool: freeform input form is gone',  tool.freeformFormGone === true);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
