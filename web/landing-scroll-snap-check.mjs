// Native document scrolling, separately from the film/entrance checks.
// Start scripts/preview-landing.py, then run with CHROMIUM_PATH=/usr/bin/chromium.
// LANDING_BROWSER=webkit runs the same contract with Playwright's WebKit;
// WEBKIT_PATH optionally supplies a compatible local WebKit launcher.
import assert from "node:assert/strict";
import { chromium, webkit } from "playwright";

const base = process.env.LANDING_URL || "http://127.0.0.1:4173";
const engine = process.env.LANDING_BROWSER || "chromium";
assert.ok(["127.0.0.1", "localhost", "[::1]"].includes(new URL(base).hostname), "Use a local landing preview");
assert.ok(["chromium", "webkit"].includes(engine));
const browser = await ({ chromium, webkit })[engine].launch({
  headless: true,
  executablePath: engine === "chromium" ? process.env.CHROMIUM_PATH : process.env.WEBKIT_PATH,
});

// offsetTop is relative to an offset parent (.story/.practical), so add
// the ancestors to compare section boundaries with document.scrollTop.
async function sections(page) {
  return page.evaluate(() => [...document.querySelectorAll(".act, .practical-section")].map(element => {
    let top = 0;
    for (let ancestor = element; ancestor; ancestor = ancestor.offsetParent) top += ancestor.offsetTop;
    return { id: element.id, top, height: element.offsetHeight };
  }));
}

// Let native wheel/key dispatch and snapping finish, then require a stable
// position for several frames. This also works where scrollend is absent.
async function settled(page) {
  return page.evaluate(() => new Promise((resolve, reject) => {
    const started = performance.now();
    let previous = -1;
    let stable = 0;
    function sample() {
      const top = document.scrollingElement.scrollTop;
      stable = top === previous ? stable + 1 : 0;
      previous = top;
      if (performance.now() - started > 250 && stable >= 8) return resolve(top);
      if (performance.now() - started > 5000) return reject(new Error("Scrolling did not settle"));
      requestAnimationFrame(sample);
    }
    requestAnimationFrame(sample);
  }));
}

function near(actual, expected, label) {
  assert.ok(Math.abs(actual - expected) <= 1, `${label}: ${actual} should equal ${expected}`);
}

async function fragment(page, id) {
  await page.evaluate(target => { location.hash = target; }, id);
  return settled(page);
}

async function checkKeyboard(page, key, targets, label) {
  await page.evaluate(() => {
    document.activeElement.blur();
    scrollTo(0, 0);
  });
  await settled(page);
  const reached = new Set(["act-1"]);
  for (let step = 0; step < 100 && reached.size < targets.length; step++) {
    await page.keyboard.press(key);
    const top = await settled(page);
    for (const target of targets) {
      if (top >= target.top - 1 && top < target.top + target.height) reached.add(target.id);
    }
  }
  assert.deepEqual([...reached], targets.map(target => target.id), `${label}: ${key} reaches every section`);
}

async function checkProfile(viewport, options = {}) {
  const label = `${engine} ${viewport.width}x${viewport.height} ${JSON.stringify(options)}`;
  const context = await browser.newContext({ viewport, ...options });
  try {
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.goto(`${base}/?film=0`, { waitUntil: "networkidle" });
    await page.mouse.move(viewport.width / 2, viewport.height / 2);
    const targets = await sections(page);
    near(await settled(page), 0, `${label}: opens at the hero`);

    // A single gesture past halfway must advance to act 2, not leave the
    // page between acts. This fails with the former proximity snapping.
    await page.mouse.wheel(0, viewport.height * 0.6);
    near(await settled(page), targets[1].top, `${label}: 60% wheel reaches the next section's offsetTop`);
    assert.equal(await page.evaluate(() => getComputedStyle(document.documentElement).scrollSnapType), "y mandatory");
    if (options.javaScriptEnabled !== false) {
      assert.ok(await page.locator("#act-2").evaluate(act => act.hasAttribute("data-arrived")), `${label}: the next act's entrance runs`);
    }

    // Browsers group synthetic small wheel events differently. Their final
    // position must still be a boundary or readable oversized-section content.
    await fragment(page, "act-1");
    for (let i = 0; i < 6; i++) await page.mouse.wheel(0, viewport.height * 0.1);
    const burst = await settled(page);
    assert.ok(targets.some(target => Math.abs(burst - target.top) <= 1
      || (target.height > viewport.height && burst >= target.top && burst <= target.top + target.height - viewport.height + 1)),
    `${label}: small wheel events settle on a section or inside tall content (${burst})`);

    for (const target of targets.slice(1)) {
      near(await fragment(page, target.id), target.top, `${label}: #${target.id} aligns with the viewport`);
      const heading = await page.locator(`#${target.id} h2`).first().boundingBox();
      assert.ok(heading.y >= 56 && heading.y < viewport.height, `${label}: #${target.id} heading is below the bar`);
    }

    // Oversized snap areas deliberately permit interior travel. Scroll to
    // their lower content before exiting to the following section.
    const tall = targets.find(target => target.height > viewport.height + 200);
    if (tall) {
      await fragment(page, tall.id);
      const delta = Math.min(viewport.height * 0.25, (tall.height - viewport.height) / 2);
      await page.mouse.wheel(0, delta);
      near(await settled(page), tall.top + delta, `${label}: a tall section allows interior scrolling`);
      await page.evaluate(top => scrollTo(0, top), tall.top + tall.height - viewport.height);
      near(await settled(page), tall.top + tall.height - viewport.height, `${label}: a tall section's bottom is reachable`);
      await page.mouse.wheel(0, viewport.height * 0.6);
      near(await settled(page), targets[targets.indexOf(tall) + 1].top, `${label}: scrolling escapes a tall section`);
    }

    await page.locator(".site-nav .cta").click();
    near(await settled(page), targets.find(target => target.id === "act-8").top, `${label}: the nav reaches the waitlist act`);
    await fragment(page, "waitlist");
    await page.locator("#waitlist input").focus();
    await settled(page);
    const input = await page.locator("#waitlist input").boundingBox();
    assert.ok(input.y >= 56 && input.y + input.height <= viewport.height, `${label}: the email input is reachable below the bar`);

    if (viewport.width >= 768 && options.javaScriptEnabled !== false) {
      for (const key of ["PageDown", "Space", "ArrowDown"]) await checkKeyboard(page, key, targets, label);
    }
    await page.locator("#waitlist input").blur();
    await page.keyboard.press("End");
    const end = await settled(page);
    near(end, await page.evaluate(() => document.scrollingElement.scrollHeight - innerHeight), `${label}: End reaches the footer`);
    const footer = await page.locator("footer").boundingBox();
    assert.ok(footer.y >= 56 && footer.y + footer.height <= viewport.height + 1, `${label}: footer links stay reachable`);
    assert.deepEqual(errors, [], `${label}: no browser errors`);
    console.log(`Passed ${label}`);
  } finally {
    await context.close();
  }
}

try {
  await checkProfile({ width: 1440, height: 900 });
  await checkProfile({ width: 390, height: 844 }, { isMobile: true, hasTouch: true });
  await checkProfile({ width: 360, height: 740 }, { isMobile: true, hasTouch: true });
  await checkProfile({ width: 1440, height: 900 }, { reducedMotion: "reduce" });
  await checkProfile({ width: 390, height: 844 }, { javaScriptEnabled: false });
} finally {
  await browser.close();
}
