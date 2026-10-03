// The contract for what `npm run build` hands skriftapp: one complete document,
// no inline script or style (the app's CSP allows neither), assets under
// /landing/generated/_astro/, and the two server slots the practical section
// still needs filled at request time.
import { before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const projectDir = fileURLToPath(new URL("..", import.meta.url));
const landingDir = fileURLToPath(new URL("../../skriftapp/buildapp/landing/", import.meta.url));
const generatedPage = `${landingDir}generated/index.html`;

// The notifications lab (#311): unlisted, so its path is written here once.
const LAB_PATH = "/lab/notifications-133c027df9b5/";
const labPage = `${landingDir}generated${LAB_PATH}index.html`;
const PREVIEW_PATH = "/lab/wall-646fe5bc6ee6/";
const previewPage = `${landingDir}generated${PREVIEW_PATH}index.html`;

let html = "";
let lab = "";

// Every stylesheet a page links, in the order it links them, as one.
function linkedCss(page) {
  return [...page.matchAll(/href="\/landing\/generated\/(_astro\/[^"]+\.css)"/g)]
    .map(([, href]) => readFileSync(`${landingDir}generated/${href}`, "utf8"))
    .join("\n");
}

// The static imports in a built module: `import` as a statement, followed by
// a specifier, a binding list, a namespace or a default binding; never
// `import(` (dynamic) or `import.meta`.
function staticImports(source) {
  return source.match(/(?:^|[;})\s])import(?:\s*["'{*]|\s+[\w$])/g) ?? [];
}

before(() => {
  execFileSync("npm", ["run", "build"], { cwd: projectDir, stdio: "pipe" });
  html = readFileSync(generatedPage, "utf8");
  lab = readFileSync(labPage, "utf8");
}, { timeout: 300_000 });

describe("the home page's stylesheet", () => {
  it("is one sheet, the lab's styles kept out of it", () => {
    assert.equal(html.match(/<link rel="stylesheet"[^>]*>/g).length, 1);
    assert.ok(!linkedCss(html).includes(".lab-panel"));
  });

  it("lets the entrance hide the hero's copy over the film's first paint, by coming later", () => {
    // Equal specificity: whichever comes later wins (#311 review).
    const css = linkedCss(html);
    const film = css.indexOf("[data-mode=film] #act-1 .act__copy>*");
    const hero = css.indexOf("[data-hero=entrance] #act-1 .act__copy>*");
    assert.ok(film >= 0 && hero >= 0);
    assert.ok(hero > film, "hero.css after film.css");
  });
});

describe("the home page's script", () => {
  it("is one module that imports no other at load, shared with no other page", () => {
    const entries = [...html.matchAll(/<script type="module" src="\/landing\/generated\/(_astro\/[^"]+\.js)"/g)].map(([, src]) => src);
    assert.equal(entries.length, 1);
    const source = readFileSync(`${landingDir}generated/${entries[0]}`, "utf8");
    // A static import is one more round trip before the hero can start,
    // after its boot has already chosen the film (#311 review).
    assert.deepEqual(staticImports(source), []);
  });

  it("is read for every form of static import, and only those", () => {
    for (const form of ['import"./a.js";', 'x();import{a as b}from"./a.js"', "import * as a from './a.js'", 'import a,{b}from"./a.js"', '}\nimport "./a.js"']) {
      assert.equal(staticImports(form).length, 1, form);
    }
    for (const form of ['import("./a.js")', "import ('./a.js')", "import.meta.url", "reimport(a)", 'a.import"x"']) {
      assert.deepEqual(staticImports(form), [], form);
    }
  });
});

describe("the notifications lab", () => {
  it("is a complete document that asks not to be indexed or followed", () => {
    assert.match(lab, /^<!DOCTYPE html>/i);
    assert.ok(lab.includes('<meta name="robots" content="noindex, nofollow">'));
  });

  it("is linked from nowhere on the home page", () => {
    assert.ok(!html.includes("/lab/"));
    assert.ok(!html.includes("notifications-133c027df9b5"));
  });

  it("preserves the original lane field with no inline script or style element", () => {
    assert.ok(lab.includes("data-hero-field"));
    assert.ok(lab.includes('data-hero="playing"'));
    const scripts = lab.match(/<script[^>]*>/g) ?? [];
    assert.equal(scripts.length, 1);
    assert.match(scripts[0], /\ssrc="\/landing\/generated\/_astro\/[^"]+\.js"/);
    assert.ok(!lab.includes("<style"));
  });

  it("keeps a desktop height for the preserved lane cards", () => {
    assert.match(linkedCss(lab), /--pill-height:\s*clamp\(34px,\s*2\.36vw,\s*42px\)/);
  });

  it("has every control the brief asks for", () => {
    for (const control of ["play", "restart", "scrub", "loop", "endless"]) assert.ok(lab.includes(`data-lab="${control}"`), control);
    const choices = { speed: ["0.25", "0.5", "1"], variant: ["a", "b", "c", "d", "e"], shape: ["dome", "full"], blur: ["copy", "filter"] };
    for (const [key, values] of Object.entries(choices)) {
      for (const value of values) assert.ok(lab.includes(`data-lab-key="${key}" data-lab-value="${value}"`), `${key} ${value}`);
    }
  });

  it("keeps the wall's styles to the lab", () => {
    assert.ok(linkedCss(lab).includes(".hero-wall"));
    assert.ok(!linkedCss(html).includes(".hero-wall"));
  });
});

describe("the generated landing document", () => {
  it("renders the notification wall on the first home paint", () => {
    assert.match(html, /<div class="hero-field notification-wall" data-hero-field aria-hidden="true"/);
    const field = html.slice(html.indexOf("data-hero-field"), html.indexOf('class="content-container act__inner"', html.indexOf("data-hero-field")));
    assert.ok((field.match(/data-wall-first-note/g) ?? []).length >= 240, "a full wall is present before JavaScript");
    assert.ok(field.includes('class="wall-note__text">Reading auth.ts</span>'), "cards carry actual activity");
    assert.ok(field.includes('class="wall-note__agent">Claude Code</span>'), "cards identify agents");
  });

  it("removes the temporary preview route, boot, and independent bundle", () => {
    assert.ok(!existsSync(previewPage));
    assert.ok(!existsSync(`${landingDir}generated/hero336-boot.js`));
    assert.ok(!existsSync(`${landingDir}generated/hero336/preview.js`));
    assert.ok(!html.includes(PREVIEW_PATH));
    assert.ok(!lab.includes(PREVIEW_PATH));
  });

  it("is a complete HTML document", () => {
    assert.match(html, /^<!DOCTYPE html>/i);
    assert.ok(html.includes("<head>") && html.includes("</html>"));
  });

  it("carries no inline script and no style element", () => {
    const scripts = html.match(/<script[^>]*>/g) ?? [];
    assert.ok(scripts.length > 0, "the page loads at least one script");
    for (const tag of scripts) assert.match(tag, /\ssrc=/);
    assert.ok(!html.includes("<style"), "no <style> element");
  });

  it("loads its bundles from /landing/generated/_astro/", () => {
    assert.match(html, /src="\/landing\/generated\/_astro\/[^"]+\.js"/);
    assert.match(html, /href="\/landing\/generated\/_astro\/[^"]+\.css"/);
  });

  it("leaves the two server-filled slots and no retired ones", () => {
    assert.ok(html.includes("{{activity_section}}"));
    assert.ok(html.includes("{{repository_url}}"));
    assert.ok(!html.includes("{{install_command}}"));
    assert.ok(!html.includes("{{platforms}}"));
  });

  it("titles the GitHub links as the public source, without an invitation", () => {
    const titles = [...html.matchAll(/href="\{\{repository_url\}\}" title="([^"]*)"/g)].map(
      (match) => match[1],
    );
    assert.deepEqual(titles, ["Build’s source code on GitHub", "Build’s source code on GitHub", "Build’s source code on GitHub"]);
    assert.ok(!/invitation/i.test(titles.join(" ")));
  });

  it("tells the story in eight acts, in order, each with a stage hook", () => {
    const acts = [...html.matchAll(/data-act="(\d)"/g)].map((match) => match[1]);
    assert.deepEqual(acts, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    assert.ok(html.includes('<canvas class="stage" data-stage aria-hidden="true"></canvas>'));
    assert.ok(html.includes("data-film"));
    assert.ok(html.includes("data-overlays"));
  });

  it("opens on the headline and closes on the waitlist", () => {
    assert.ok(html.includes('<span class="hero-line">Your agents are moving fast.</span> <span class="hero-line hero-line--quiet">Know what needs you.</span>'));
    assert.ok(html.includes("Free · Open source · Runs on your machines"));
    assert.ok(html.includes("Write the task. Hand it off."));
    assert.ok(html.includes("Make the last edit yourself."));
    assert.ok(html.includes("data-waitlist"));
    assert.ok(html.includes("Join the waitlist"));
    assert.ok(html.includes("Accounts are invite-only while Build is in alpha."));
  });

  it("shows the readable proof beside each act", () => {
    for (const proof of [
      "it(&quot;keeps archived items in search&quot;, () =&gt; {",
      "Make archived items searchable",
      "Keep archived items in search?",
      "Match the label the sidebar uses.",
      "archive: keep archived results searchable",
      "Archived results remain visible and now carry a clear label.",
      "Ready for review",
    ]) {
      assert.ok(html.includes(proof), proof);
    }
  });

  it("keeps the sample diff valid and carries the human-added label into the review", () => {
    assert.ok(!html.includes("archived: true\n"), "the agent's object literal ends its line with a comma");
    assert.ok(html.includes("+   archived: true,"), "the close-up diff");
    assert.ok(html.includes("+   archived: true,</span>"), "the document diff");
    const label = "label: &quot;Archived&quot;,";
    const act7 = html.slice(html.indexOf('data-act="7"'), html.indexOf('data-act="8"'));
    assert.ok(act7.includes(label), "act 7's proof keeps the label a person added in act 6");
    const review = html.slice(html.indexOf('data-panel="review"'), html.indexOf('data-act="1"'));
    assert.ok(review.includes(label), "the review close-up keeps the label too");
    assert.ok(html.includes('data-diff-add>+3<'), "the close-up hunk counts three added lines before the person's");
    assert.ok(html.includes("+4 −1"), "the document proof counts four after it");
  });

  it("offers no download, only the waitlist, in the bar and the hero", () => {
    const calls = [...html.matchAll(/<a class="cta[^"]*" href="([^"]+)">([^<]+)<\/a>/g)].map(([, href, text]) => [href, text]);
    assert.deepEqual(calls, [["#act-8", "Join the waitlist"], ["#act-8", "Join the waitlist"]]);
    assert.ok(!/download/i.test(html));
  });

  it("draws the hero's notification wall for no one but the eye", () => {
    const field = html.slice(html.indexOf("data-hero-field"), html.indexOf('class="content-container act__inner"', html.indexOf("data-hero-field")));
    assert.match(html, /<div class="hero-field notification-wall" data-hero-field aria-hidden="true"/);
    assert.ok(field.includes("data-wall-routine"));
    assert.ok(!/<(a|button|input)\b/.test(field), "nothing in the field takes focus");
    assert.ok(!/(OpenCode|Gemini|Cursor|opencode|gemini|cursor)/.test(field), "only supported harnesses");
  });

  it("marks first-frame cards with their supported harnesses", () => {
    const field = html.slice(html.indexOf("data-hero-field"), html.indexOf('class="content-container act__inner"', html.indexOf("data-hero-field")));
    assert.ok(!/<svg\b|<use\b/.test(field), "no SVG per card");
    const cards = [...field.matchAll(/<div class="wall-first-note"[^>]*>/g)].map(([tag]) => tag);
    assert.ok(cards.length >= 240);
    for (const tag of cards) assert.match(tag, /data-harness="(claude|codex|pi)"/);
    for (const id of ["claude", "codex", "pi"]) assert.ok(field.slice(0, field.indexOf(">")).includes(`--mark-${id}:url(data:image/svg+xml,`), id);
  });

  it("keeps the legacy lane motion out of the home stylesheet", () => {
    const css = linkedCss(html);
    assert.ok(css.includes(".wall-routine"), "the home stylesheet carries the new field");
    assert.ok(!css.includes("@keyframes hero-drift"));
    assert.ok(!css.includes("@keyframes hero-sway"));
  });

  it("paints the harness marks through a mask older WebKit and Chromium read too", () => {
    const css = linkedCss(html).replace(/\s+/g, "");
    const masks = [...css.matchAll(/(?<![\w-])mask-([a-z]+):([^;}]+)/g)];
    assert.ok(masks.length >= 6, `${masks.length} mask declarations`);
    for (const [, property, value] of masks) {
      // The old WebKit compositing keyword for the standard "intersect" is
      // "source-in"; all other mask longhands use the same value.
      const prefixed = property === "composite" && value === "intersect" ? "source-in" : value;
      assert.ok(css.includes(`-webkit-mask-${property}:${prefixed}`), `-webkit-mask-${property}:${prefixed}`);
    }
  });

  it("packs narrower first-frame cards on a phone", () => {
    const css = linkedCss(html).replace(/\s+/g, "");
    assert.ok(css.includes(".wall-first-frame"));
    assert.match(css, /--pitch-x:176px/);
    assert.match(css, /--card-width:164px/);
  });

  it("decides the entrance in the head, before the page's script", () => {
    const boot = html.indexOf('src="/landing/generated/hero-boot.js"');
    const main = html.search(/<script type="module" src="\/landing\/generated\/_astro\//);
    assert.ok(boot > 0 && boot < main && boot < html.indexOf("<body"));
  });

  it("ships no development scrubber", () => {
    const bundles = readdirSync(`${landingDir}generated/_astro`).filter((name) => name.endsWith(".js"));
    assert.ok(bundles.length > 0);
    for (const bundle of bundles) {
      const source = readFileSync(`${landingDir}generated/_astro/${bundle}`, "utf8");
      assert.ok(!source.includes("Hero entrance scrubber"), bundle);
    }
  });

  it("offers the anchor the invite pages link to", () => {
    assert.ok(html.includes('id="waitlist"'));
  });

  it("ships a stylesheet that respects reduced motion", () => {
    const css = linkedCss(html);
    assert.match(css, /prefers-reduced-motion:\s*reduce/);
  });

  it("says which of the film's screens are concepts, not the shipped app", () => {
    const act = (number) => html.slice(html.indexOf(`data-act="${number}"`), html.indexOf(`data-act="${number + 1}"`));
    assert.ok(act(5).includes("Workflow editor concept. This feature is not available in Build."));
    assert.ok(act(7).includes("Review screen concept. In Build today, review changes in the Changes view."));
    assert.ok(!act(5).includes("Saved as"), "the document proof claims no saved workflow");
    assert.ok(!act(7).includes("Triage"), "the document proof claims no triage");
  });

  it("says workspaces, never worktrees", () => {
    assert.ok(!/worktree/i.test(html));
  });

  it("keeps the practical section without the download chooser", () => {
    assert.ok(html.includes("Install Build where your agents run."));
    assert.ok(html.includes("Which agents can I use?"));
    assert.ok(!html.includes("download-chooser"));
    assert.ok(!html.includes("curl "));
  });

  it("references only same-origin files that exist on disk", () => {
    const references = new Set(
      [...html.matchAll(/(?:src|href)="\/landing\/([^"?#]+)/g)].map((match) => match[1]),
    );
    assert.ok(references.size > 0);
    for (const reference of references) {
      assert.ok(existsSync(landingDir + reference), reference);
    }
  });

  it("names no external origin", () => {
    const origins = [...html.matchAll(/https?:\/\/([^/"']+)/g)].map((match) => match[1]);
    for (const origin of origins) {
      assert.ok(
        ["getbuild.ing", "github.com"].includes(origin),
        `unexpected origin ${origin}`,
      );
    }
  });
});
