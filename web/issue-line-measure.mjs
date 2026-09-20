// The issue lines, measured under the real stylesheet at a phone's width.
//
//   ISSUES_REPO=<this checkout> node web/issue-line-measure.mjs
//
// Exits non-zero on a failed check.
//
// # Why this exists
//
// #40's three defects — a ninety-pixel gutter, rows wrapping to three lines,
// and a full message gap between them — all shipped with a green suite. They
// had to: jsdom has no layout, so a gutter, a wrap and a gap do not exist
// there at all. The unit tests asserted the right elements in the right order
// and were right to pass.
//
// Then the fix shipped with the rhythm at 8px instead of 4, because the rule
// reads a variable that one stylesheet set without setting the gap beside it.
// Nothing without a layout engine could see that either. This found it.
//
// So the rule this file stands for: a claim about SPACE is only ever checked
// by something that lays out space.
//
// # What it measures, and against what
//
// The rows are the renderer's own output, committed at fixtures/issue-lines.html
// and held to the renderer by spa/test/issueLineFixture.test.js — so this needs
// no build tool and no bridge able to produce tracking notices, and the markup
// cannot quietly stop matching the product.
//
// They are injected into a live conversation in the running SPA, so the
// measurement is taken against the real stylesheet, the real timeline width
// and the real cascade. Reading the CSS file instead would prove nothing: the
// bug WAS the cascade.

import { existsSync, readFileSync } from "node:fs";
import { chromium } from "/home/zech/.local/share/mise/installs/npm-playwright/1.63.0/node_modules/playwright/index.mjs";

const APP = process.env.APP_URL || "http://localhost:8090";

/** The ONE checkout this runs off, refused rather than defaulted — the same
 *  reason the other browser passes refuse it. */
const REPO = process.env.ISSUES_REPO;
if (!REPO) {
  console.error("set ISSUES_REPO to the checkout under test — this run must not mix checkouts");
  process.exit(2);
}
const fixture = `${REPO}/web/fixtures/issue-lines.html`;
if (!existsSync(fixture)) {
  console.error(`no fixture at ${fixture} — regenerate with ISSUE_LINE_FIXTURE=write npx vitest run test/issueLineFixture.test.js`);
  process.exit(2);
}
const rows = readFileSync(fixture, "utf8");
const seed = JSON.parse(readFileSync("/tmp/live-seed.json", "utf8"));

/** How close consecutive lines must sit to read as a list rather than as
 *  separate messages (#40 asked for about 4px). */
const TIGHT_MAX = 6;
/** How far a real message either side must stay, so the run does not swallow
 *  the conversation around it. */
const MESSAGE_MIN = 10;

const results = [];
const record = (ok, what) => {
  results.push(ok);
  console.log(`${ok ? "PASS" : "FAIL"}  ${what}`);
};

const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", headless: true, args: ["--no-sandbox"] });

async function measure(width, label) {
  const context = await browser.newContext({ viewport: { width, height: 900 }, deviceScaleFactor: 2 });
  try {
    const page = await context.newPage();
    await page.goto(`${APP}/auth/dummy/login`, { waitUntil: "load" });
    await page.fill('input[name="email"]', "qa@localhost");
    await Promise.all([
      page.waitForNavigation({ waitUntil: "load" }),
      page.evaluate(() => document.querySelector('form[action="/auth/dummy-login"]').submit()),
    ]);

    // A seeded workspace, not the project page: live-seed.mjs gives each one an
    // agent conversation with a posted message, so there is a real timeline to
    // measure in. A project's own rail has no conversation until somebody
    // presses the bubble that mints one.
    const ws = seed.workspaces[0];
    await page.goto(`${APP}/app/#/device/${seed.deviceId}/project/${seed.projectId}/workspace/${ws.workspaceId}/changes`, { waitUntil: "load" });
    await page.waitForTimeout(8000);

    // On a phone the rail is a collapsed popover: the conversation is not on
    // screen until the AGENT's own bubble is pressed. Only pressed when there
    // is no timeline already — pressing one on a desktop that has it open
    // CLOSES it, which is how an earlier version of this lost the desktop it
    // had a moment before.
    if (!(await page.$(".thread-items"))) {
      const bubble = await page.$("#agent-rail [data-agent]:not([data-agent=''])");
      if (bubble) {
        await bubble.click().catch(() => {});
        await page.waitForTimeout(4000);
      }
    }

    const found = await page.evaluate((html) => {
      const host = document.querySelector("#agent-rail .thread-items") || document.querySelector(".thread-items");
      if (!host) return { error: "no conversation on screen to measure in" };
      // Where a real conversation's rows go, so they inherit the timeline's
      // width, its gap, and every rule that applies to a message.
      host.innerHTML = new DOMParser().parseFromString(html, "text/html").querySelector(".thread-items").innerHTML;
      const timeline = host.getBoundingClientRect();
      const box = (el) => el.getBoundingClientRect();
      const lines = [...host.querySelectorAll(".thread-issue-line")];
      const messages = [...host.querySelectorAll(".thread-message:not(.thread-issue-line)")];
      const textLeft = (el) => Math.round(box(el.querySelector(".thread-body") || el).left - timeline.left);
      const gaps = [];
      for (let i = 1; i < lines.length; i++) gaps.push(Math.round(box(lines[i]).top - box(lines[i - 1]).bottom));
      return {
        timelineWidth: Math.round(timeline.width),
        messageTextLeft: messages[0] ? textLeft(messages[0]) : null,
        lines: lines.map((el) => {
          const anchor = el.querySelector("a, span");
          return {
            text: el.textContent.replace(/\s+/g, " ").trim(),
            left: Math.round(box(el).left - timeline.left),
            height: Math.round(box(el).height),
            rows: Math.round(box(el).height / parseFloat(getComputedStyle(anchor).lineHeight)),
            // #49 took the title off the line so it would fit: whether it
            // actually does is the thing to measure now, and a clipped span is
            // the line not fitting.
            clipped: [...el.querySelectorAll("span")].some((one) => one.scrollWidth > one.clientWidth + 1),
            overflows: anchor ? anchor.scrollWidth > Math.ceil(box(anchor).width) + 1 : false,
          };
        }),
        gapsBetweenLines: gaps,
        // Every gap between an issue line and a NEIGHBOUR that is not one,
        // taken from the DOM order rather than from an assumption about where
        // the plain messages sit — they do not all bracket the run, and an
        // earlier version of this measured two rows that were nowhere near
        // each other and reported -455.
        // #50: a fenced block scrolls; nothing around it grows. `scrollWidth`
        // past `clientWidth` on the block is it scrolling; the same on any
        // ancestor is it having escaped instead.
        code: [...host.querySelectorAll("pre.md-code")].map((block) => {
          const card = block.closest(".thread-comment-card") || block.parentElement;
          const grew = (el) => (el ? el.scrollWidth > el.clientWidth + 1 : false);
          return {
            scrolls: grew(block),
            blockWidth: Math.round(box(block).width),
            cardGrew: grew(card),
            columnGrew: grew(host),
            widerThanColumn: Math.round(box(block).right) > Math.round(timeline.right) + 1,
          };
        }),
        gapsToMessages: [...host.children].flatMap((el, index, all) => {
          const next = all[index + 1];
          if (!next) return [];
          const isLine = (one) => one.classList.contains("thread-issue-line");
          if (isLine(el) === isLine(next)) return [];
          return [Math.round(box(next).top - box(el).bottom)];
        }),
      };
    }, rows);

    console.log(`\n──── ${label} (${width} px) ────`);
    console.log(JSON.stringify(found, null, 2));
    await page.screenshot({ path: `/tmp/issue-lines-${width}.png` });
    // The timeline on its own as well, with the chrome that floats over it
    // taken down first: the composer is pinned below the scroller and sits on
    // top of the lower rows, and the rows are the subject. Measured before
    // this, never after — the numbers above come from the page as it really
    // is, and only the picture is tidied.
    await page.evaluate(() => {
      for (const selector of [".rail-composer", ".rail-panel-head", "#console-region", ".rail-strip"]) {
        for (const el of document.querySelectorAll(selector)) el.style.visibility = "hidden";
      }
    });
    const timelineElement = await page.$("#agent-rail .thread-items, .thread-items");
    if (timelineElement) await timelineElement.screenshot({ path: `/tmp/issue-lines-${width}-rows.png` }).catch(() => {});

    if (found.error) return record(false, `${label}: ${found.error}`);
    record(found.lines.length > 0, `${label}: there are lines to measure — ${found.lines.length}`);
    record(found.lines.every((l) => l.rows === 1), `${label}: every line is ONE line — heights ${found.lines.map((l) => l.height).join(", ")}`);
    record(
      found.lines.every((l) => l.left === found.messageTextLeft),
      `${label}: no gutter — lines at ${[...new Set(found.lines.map((l) => l.left))].join("/")}, message text at ${found.messageTextLeft}`,
    );
    record(found.gapsBetweenLines.every((g) => g <= TIGHT_MAX), `${label}: tight rhythm — gaps ${found.gapsBetweenLines.join(", ")}`);
    record(
      found.gapsToMessages.length > 0 && found.gapsToMessages.every((g) => g > MESSAGE_MIN),
      `${label}: a real message either side keeps its gap — ${found.gapsToMessages.join(", ")}`,
    );
    record(!found.lines.some((l) => l.overflows), `${label}: nothing overflows its row`);
    record(found.code.length > 0, `${label}: there is a code block to measure — ${found.code.length}`);
    record(
      found.code.every((c) => c.scrolls),
      `${label}: the code block scrolls — ${found.code.map((c) => `${c.blockWidth}px`).join(", ")}`,
    );
    record(
      found.code.every((c) => !c.cardGrew && !c.columnGrew && !c.widerThanColumn),
      `${label}: and nothing around it grew — card ${found.code.map((c) => c.cardGrew)}, column ${found.code.map((c) => c.columnGrew)}, past the edge ${found.code.map((c) => c.widerThanColumn)}`,
    );
    // The point of #49: with the title gone the whole line fits, even at 390.
    record(
      !found.lines.some((l) => l.clipped),
      `${label}: every line fits whole, nothing clipped — ${found.lines.map((l) => l.text.slice(0, 28)).join(" | ")}`,
    );
  } finally {
    // Always, whatever threw: a browser context left open holds the run alive
    // and the next one inherits its cookies.
    await context.close();
  }
}

try {
  await measure(390, "phone");
  await measure(1440, "desktop");
} finally {
  await browser.close();
}

console.log(`\n${results.filter(Boolean).length}/${results.length} passed`);
console.log("screenshots: /tmp/issue-lines-390.png, /tmp/issue-lines-1440.png");
process.exit(results.every(Boolean) ? 0 : 1);
