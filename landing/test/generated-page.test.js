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

let html = "";

before(() => {
  execFileSync("npm", ["run", "build"], { cwd: projectDir, stdio: "pipe" });
  html = readFileSync(generatedPage, "utf8");
}, { timeout: 300_000 });

describe("the generated landing document", () => {
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
    assert.ok(html.includes("Say what needs doing."));
    assert.ok(html.includes("Every change lands in Git."));
    assert.ok(html.includes("data-waitlist"));
    assert.ok(html.includes("Join the waitlist"));
    assert.ok(html.includes("Invite-only alpha."));
    assert.ok(!html.includes("Invite-only alpha. Free and open source."), "the closing line says free once");
  });

  it("shows the readable proof beside each act", () => {
    for (const proof of [
      "it(&quot;keeps archived items in search&quot;, () =&gt; {",
      "Make archived items searchable",
      "Keep archived items in search?",
      "Test · runs archive.test.ts",
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

  it("offers the download where the public installers are, in the bar and the hero", () => {
    const downloads = [...html.matchAll(/<a class="cta[^"]*" href="([^"]+)">([^<]+)<\/a>/g)].map(([, href, text]) => [href, text]);
    assert.deepEqual(downloads, [["/docs#setup", "Download"], ["/docs#setup", "Download Build"]]);
  });

  it("draws the hero's notification field for no one but the eye", () => {
    const field = html.slice(html.indexOf("data-hero-field"), html.indexOf('class="content-container act__inner"', html.indexOf("data-hero-field")));
    assert.match(html, /<div class="hero-field" data-hero-field aria-hidden="true"/);
    assert.equal([...field.matchAll(/data-attention="(\w+)"/g)].length, 3);
    assert.ok(!/<(a|button|input)\b/.test(field), "nothing in the field takes focus");
    assert.ok(!/(OpenCode|Gemini|Cursor|opencode|gemini|cursor)/.test(field), "only supported harnesses");
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
    const href = html.match(/href="\/landing\/generated\/(_astro\/[^"]+\.css)"/)[1];
    const css = readFileSync(`${landingDir}generated/${href}`, "utf8");
    assert.match(css, /prefers-reduced-motion:\s*reduce/);
  });

  it("says workspaces, never worktrees", () => {
    assert.ok(!/worktree/i.test(html));
  });

  it("keeps the practical section without the download chooser", () => {
    assert.ok(html.includes("Your host does the work."));
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
