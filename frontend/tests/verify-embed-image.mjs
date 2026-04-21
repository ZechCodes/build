// Verify that a <build-image> tag in a chat message renders as an
// inline image, and that clicking it opens a lightbox with zoom +
// double-tap + Escape-to-close behavior.
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'http://127.0.0.1:8100';
const EMAIL = process.env.EMAIL || 'dev@local';

// Tiny 1×1 red PNG — 67 bytes → base64.
const TINY_PNG_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

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

  const MSG_ID = 'verify-image-' + Date.now();
  await page.evaluate(({ id, msgId, b64 }) => {
    const content = `Here is a screenshot:\n\n<build-image path="/tmp/pixel.png" mime="image/png">\n${b64}\n</build-image>\n\nThat's it.`;
    window.__v2debug.stores.messagesStore.append(id, {
      id: msgId, channel_id: id, sender: 'Agent', content,
      created_at: new Date().toISOString(),
    });
  }, { id: chId, msgId: MSG_ID, b64: TINY_PNG_B64 });
  await page.waitForTimeout(150);

  // 1. Embed rendered as a figure, not as text.
  const embed = await page.evaluate((msgId) => {
    const msg = document.querySelector(`.v2-msg[data-msg-id="${msgId}"]`);
    const fig = msg?.querySelector('.v2-embed-image');
    const img = fig?.querySelector('img');
    return {
      present: !!fig,
      path: fig?.dataset.path,
      mime: fig?.dataset.mime,
      caption: fig?.querySelector('.v2-embed-image-path')?.textContent?.trim(),
      src: img?.src?.slice(0, 40),
      hasRawB64InText: msg?.innerText?.includes('iVBORw0KGgo'),
    };
  }, MSG_ID);
  check('build-image renders as a figure',       embed.present,                JSON.stringify(embed));
  check('figure carries data-path',              embed.path === '/tmp/pixel.png', embed.path);
  check('figure carries data-mime',              embed.mime === 'image/png',   embed.mime);
  check('caption shows the path',                embed.caption === '/tmp/pixel.png');
  check('img src is a data:image/png base64 URL', /^data:image\/png;base64,/.test(embed.src || ''), embed.src);
  check('raw base64 is NOT floating as text',    !embed.hasRawB64InText);

  // 2. Click the image → lightbox opens with the same src.
  await page.locator(`.v2-msg[data-msg-id="${MSG_ID}"] .v2-embed-image img`).click({ force: true });
  await page.waitForTimeout(120);
  const lightboxOpen = await page.evaluate(() => {
    const lb = document.querySelector('.v2-lightbox');
    const img = lb?.querySelector('.v2-lightbox-img');
    return {
      open: lb?.classList.contains('open'),
      pathText: lb?.querySelector('.v2-lightbox-path')?.textContent?.trim(),
      srcOk: /^data:image\/png;base64,/.test(img?.src || ''),
      transform: img?.style.transform,
    };
  });
  check('lightbox opens on image click', lightboxOpen.open === true, JSON.stringify(lightboxOpen));
  check('lightbox shows the path',       lightboxOpen.pathText === '/tmp/pixel.png');
  check('lightbox img carries the src',  lightboxOpen.srcOk);

  // 3. Wheel event zooms the image.
  await page.evaluate(() => {
    const lb = document.querySelector('.v2-lightbox');
    const ev = new WheelEvent('wheel', { deltaY: -200, clientX: 720, clientY: 400, bubbles: true, cancelable: true });
    lb.dispatchEvent(ev);
  });
  await page.waitForTimeout(80);
  const afterWheel = await page.evaluate(() => {
    const img = document.querySelector('.v2-lightbox-img');
    const m = img?.style.transform.match(/scale\(([\d.]+)\)/);
    return m ? parseFloat(m[1]) : null;
  });
  check('wheel zoom scales > 1', afterWheel !== null && afterWheel > 1.0, String(afterWheel));

  // 4. Double-click toggles to ~2.5× (or from zoomed-in back to 1×).
  // Dispatch the dblclick event directly — Playwright's .dblclick()
  // fires on the img, but pointer-capture on the host can swallow the
  // synthesized browser dblclick before bubbling. Going native-event
  // avoids the race.
  await page.evaluate(() => {
    const lb = document.querySelector('.v2-lightbox');
    const img = lb.querySelector('.v2-lightbox-img');
    const rect = img.getBoundingClientRect();
    const cx = rect.left + rect.width / 2;
    const cy = rect.top + rect.height / 2;
    lb.dispatchEvent(new MouseEvent('dblclick', {
      bubbles: true, cancelable: true, clientX: cx, clientY: cy,
    }));
  });
  await page.waitForTimeout(80);
  const afterDbl = await page.evaluate(() => {
    const img = document.querySelector('.v2-lightbox-img');
    const m = img?.style.transform.match(/scale\(([\d.]+)\)/);
    return m ? parseFloat(m[1]) : null;
  });
  // Since we already zoomed with wheel to > 1.05, dblclick should
  // reset scale to 1.
  check('double-click resets scale when zoomed', afterDbl === 1, String(afterDbl));

  // 5. Escape closes the lightbox.
  await page.keyboard.press('Escape');
  await page.waitForTimeout(80);
  const closed = await page.evaluate(() => !document.querySelector('.v2-lightbox')?.classList.contains('open'));
  check('Escape closes the lightbox', closed);

  console.log(`\nResult: ${passed} passed, ${failed} failed`);
  if (failed) process.exitCode = 1;
} finally {
  await browser.close();
}
