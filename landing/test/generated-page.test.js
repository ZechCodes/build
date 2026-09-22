// The contract for what `npm run build` hands skriftapp: one complete document,
// no inline script or style (the app's CSP allows neither), assets under
// /landing/generated/_astro/, and the two server slots the practical section
// still needs filled at request time.
import { before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
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

  it("tells the story in eight acts, in order, each with a stage hook", () => {
    const acts = [...html.matchAll(/data-act="(\d)"/g)].map((match) => match[1]);
    assert.deepEqual(acts, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    assert.ok(html.includes('<canvas class="stage" data-stage aria-hidden="true"></canvas>'));
    assert.ok(html.includes("data-film"));
    assert.ok(html.includes("data-overlays"));
  });

  it("opens on the headline and closes on the waitlist", () => {
    assert.ok(html.includes("Your agents. Your machine. Your call."));
    assert.ok(html.includes("Say what needs doing."));
    assert.ok(html.includes("Every change lands in Git."));
    assert.ok(html.includes("data-waitlist"));
    assert.ok(html.includes("Join the waitlist"));
    assert.ok(html.includes("Invite-only alpha. Free and open source."));
  });

  it("shows the readable proof beside each act", () => {
    for (const proof of [
      "it(&quot;keeps archived items in search&quot;, () =&gt; {",
      "Make archived items searchable",
      "Keep archived items in search?",
      "Test · runs archive.test.ts",
      "archive: keep archived results searchable",
      "Keep archived items searchable and visibly labeled.",
    ]) {
      assert.ok(html.includes(proof), proof);
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
    assert.ok(html.includes("Your hosts do the work."));
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
