// Verify the paginated multi-question interaction card:
//   - detects `metadata.questions` with >1 entry and renders a
//     .v2-int-stepper card with progress dots
//   - first option click advances to step 2, second to step 3
//   - last step's click fires intent.interaction_response with the
//     full `stepAnswers` array (one entry per question)
//   - Back button returns to the previous step and preserves the
//     earlier answer's `.selected` highlight
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

  const MSG_ID = 'stepper-' + Date.now();
  await page.evaluate(({ id, msgId }) => {
    const questions = [
      { header: 'Scope',   question: 'What scope?',     options: [{id: 'Everything', label: 'Everything'}, {id: 'Only asks', label: 'Only asks'}] },
      { header: 'Plan',    question: 'Rename authors?', options: [{id: 'Rename', label: 'Rename'}, {id: 'Leave', label: 'Leave'}] },
      { header: 'Old sys', question: 'Delete?',         options: [{id: 'Delete', label: 'Delete'}, {id: 'Keep both', label: 'Keep both'}] },
    ];
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId, channel_id: id, sender: 'Agent',
      content: 'summary',
      metadata: JSON.stringify({
        interaction_id: msgId,
        kind: 'question',
        questions,
        allow_freeform: true,
      }),
      created_at: new Date().toISOString(),
    });
    window.__lastInteraction = null;
    window.__v2debug.bus.on('intent.interaction_response', (p) => { window.__lastInteraction = p; });
  }, { id: chId, msgId: MSG_ID });
  await page.waitForTimeout(200);

  // 1. Card renders as a stepper with 3 dots.
  const initial = await page.evaluate((msgId) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-int-stepper`);
    return card ? {
      step:  card.getAttribute('data-step'),
      total: card.getAttribute('data-total'),
      dots:  card.querySelectorAll('.v2-int-dot').length,
      activeIdx: [...card.querySelectorAll('.v2-int-dot')].findIndex(d => d.classList.contains('active')),
      header: card.querySelector('.v2-int-step-header')?.textContent?.trim(),
      label:  card.querySelector('.v2-int-step-label')?.textContent?.trim(),
    } : null;
  }, MSG_ID);
  check('stepper card rendered',             !!initial, JSON.stringify(initial));
  check('stepper shows 3 progress dots',     initial.dots === 3);
  check('first dot is active',                initial.activeIdx === 0);
  check('step 1 body shows "Scope" header',   initial.header === 'Scope');
  check('step 1 label reads "Step 1 of 3"',   initial.label?.startsWith('Step 1 of 3'));

  // 2. Click first option → advance to step 2.
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] [data-step-opt-id="Everything"]`).click();
  await page.waitForTimeout(50);
  const afterStep1 = await page.evaluate((msgId) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-int-stepper`);
    return {
      step:   card.getAttribute('data-step'),
      header: card.querySelector('.v2-int-step-header')?.textContent?.trim(),
      activeIdx: [...card.querySelectorAll('.v2-int-dot')].findIndex(d => d.classList.contains('active')),
      fired:  !!window.__lastInteraction,
    };
  }, MSG_ID);
  check('advanced to step 2 after first click', afterStep1.step === '1', JSON.stringify(afterStep1));
  check('step 2 shows "Plan" header',           afterStep1.header === 'Plan');
  check('step 1 click did NOT submit',          afterStep1.fired === false);

  // 3. Click Back → back to step 1 with the earlier answer highlighted.
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] [data-step-action="back"]`).click();
  await page.waitForTimeout(50);
  const back = await page.evaluate((msgId) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-int-stepper`);
    return {
      step: card.getAttribute('data-step'),
      selected: card.querySelector('.v2-int-opt.selected')?.getAttribute('data-step-opt-id'),
    };
  }, MSG_ID);
  check('Back returns to step 1',              back.step === '0', JSON.stringify(back));
  check('prior answer is still highlighted',    back.selected === 'Everything');

  // 4. Proceed through all 3 steps.
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] [data-step-opt-id="Everything"]`).click();
  await page.waitForTimeout(40);
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] [data-step-opt-id="Rename"]`).click();
  await page.waitForTimeout(40);
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] [data-step-opt-id="Keep both"]`).click();
  await page.waitForTimeout(120);

  const submit = await page.evaluate(() => window.__lastInteraction);
  check('last-step click fires intent.interaction_response',
    !!submit && Array.isArray(submit.stepAnswers),
    JSON.stringify(submit));
  check('stepAnswers in order',
    submit?.stepAnswers?.map(s => s.answer).join('|') === 'Everything|Rename|Keep both',
    JSON.stringify(submit?.stepAnswers?.map(s => s.answer)));
  check('stepAnswers carry headers',
    submit?.stepAnswers?.map(s => s.header).join('|') === 'Scope|Plan|Old sys');

  // Card is visually resolved — shows a summary, no buttons.
  const after = await page.evaluate((msgId) => {
    const card = document.querySelector(`.v2-msg[data-msg-id="${msgId}"] .v2-int-stepper`);
    return {
      resolved: card?.classList.contains('resolved'),
      hasOptions: !!card?.querySelector('.v2-int-options'),
      summary: [...card?.querySelectorAll('.v2-int-step-summary') || []].length,
    };
  }, MSG_ID);
  check('card marked resolved after submit',  after.resolved === true, JSON.stringify(after));
  check('card replaced options with summary', after.hasOptions === false && after.summary === 3);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
