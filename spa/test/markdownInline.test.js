/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { markdownHtml } from "../src/core/markdown.js";
import { inlineHtml } from "../src/core/markdownInline.js";

const render = (source, options = {}) => {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source, { mode: "inline", ...options });
  return host;
};

// An infinite scanner loop must fail in an expendable child rather than
// blocking the Vitest worker and preventing its own timeout from running.
const renderInChild = (source) => {
  const module = pathToFileURL(resolve("src/core/markdownInline.js")).href;
  const script = `import { inlineHtml } from ${JSON.stringify(module)}; process.stdout.write(inlineHtml(${JSON.stringify(source)}, {}, { footnote: () => null }));`;
  return spawnSync(process.execPath, ["--input-type=module", "--eval", script], {
    encoding: "utf8", timeout: 2000, killSignal: "SIGKILL",
  });
};

const CLOSER_BEFORE_OPENER = [
  ["keyboard adjacent", "</kbd><kbd>"],
  ["keyboard separated", "</kbd> text <kbd>tail"],
  ["single backticks", "` text ``tail"],
  ["multiple backticks", "`` text `tail"],
  ["star emphasis", "* text *tail"],
  ["underscore emphasis", "_ text _tail"],
  ["star strong", "** text **tail"],
  ["underscore strong", "__ text __tail"],
  ["strikethrough", "~~ text ~~tail"],
  ["web link", "](closed) [tail"],
  ["image", "](closed) ![tail"],
  ["angle URL", "> text <https://tail.test"],
  ["angle email", "> text <someone@tail.test"],
  ["Build reference", "]] text [[board:tail"],
  ["footnote", "] text [^tail"],
];

describe("the shared inline vocabulary", () => {
  it("renders nested emphasis, underscore strong and strikethrough", () => {
    expect(render("*em* _also_ **strong _nested_** __bold__ ~~gone~~").innerHTML)
      .toBe("<em>em</em> <em>also</em> <strong>strong <em>nested</em></strong> <strong>bold</strong> <del>gone</del>");
    expect(render("***both***").innerHTML).toBe("<em><strong>both</strong></em>");
    expect(render("foo_bar_baz a*b*c \\*literal\\* ~single~").innerHTML).toBe("foo_bar_baz a<em>b</em>c *literal* ~single~");
  });

  it("keeps code completely literal and pairs backtick runs by their length", () => {
    const host = render("``a ` b **bold** ~~gone~~ <kbd>x</kbd> [x](https://a.test)`` and `**literal**` **real**");
    expect([...host.querySelectorAll("code")].map((node) => node.textContent)).toEqual([
      "a ` b **bold** ~~gone~~ <kbd>x</kbd> [x](https://a.test)", "**literal**",
    ]);
    expect(host.querySelectorAll("a, kbd, del")).toHaveLength(0);
    expect(host.querySelectorAll("strong")).toHaveLength(1);
    expect(render("`unclosed **bold**").innerHTML).toBe("`unclosed <strong>bold</strong>");
  });

  it("allows only the explicit attribute-free keyboard grammar", () => {
    const host = render('<kbd>Ctrl & K</kbd> <kbd>**literal**</kbd> <kbd onclick="x">bad</kbd> <KBD>bad</KBD> <kbd><img src=x></kbd>');
    expect([...host.querySelectorAll("kbd")].map((node) => node.textContent)).toEqual(["Ctrl & K", "**literal**"]);
    expect(host.querySelectorAll("img, [onclick], strong")).toHaveLength(0);
    expect(host.textContent).toContain('<kbd onclick="x">bad</kbd>');
    expect(host.textContent).toContain("<kbd><img src=x></kbd>");
  });

  it("renders escaped image metadata without a request-bearing attribute", () => {
    const host = render('![a " <b> & #42](https://example.test/a.png "A title")');
    const button = host.querySelector("button");
    expect(button?.dataset.mdImageSrc).toBe("https://example.test/a.png");
    expect(button?.dataset.mdImageAlt).toBe('a " <b> & #42');
    expect(button?.dataset.mdImageTitle).toBe("A title");
    expect(host.querySelectorAll("img, [src], a, b")).toHaveLength(0);
  });

  it("keeps external images inert until their one-instance load button is activated", () => {
    const host = render('![A "diagram" & <svg> #42](https://images.test/a.png "Image title")');
    expect(host.querySelectorAll("img, [src]")).toHaveLength(0);
    const button = host.querySelector("button.md-image-load");
    expect(button?.getAttribute("type")).toBe("button");
    expect(button?.dataset.mdImageSrc).toBe("https://images.test/a.png");
    expect(button?.dataset.mdImageAlt).toBe('A "diagram" & <svg> #42');
    expect(button?.dataset.mdImageTitle).toBe("Image title");
    expect(button?.textContent).toBe('Load image from images.test — A "diagram" & <svg> #42');
    expect(host.querySelectorAll("svg, a, em, strong")).toHaveLength(0);
  });

  it("requires consent even for the browser's own origin", () => {
    const src = new URL("/same-origin.png", location.href).href;
    const host = render(`![Local diagram](${src})`);
    expect(host.querySelector("button")?.dataset.mdImageSrc).toBe(src);
    expect(host.querySelectorAll("img, [src]")).toHaveLength(0);
  });

  it("refuses unsafe images and credentials while preserving their literal text", () => {
    for (const destination of ["javascript:alert(1)", "data:image/svg+xml,x", "blob:https://a.test/id", "mailto:a@b.test", "file:///tmp/a", "//a.test/a", "/a", "https://user:secret@a.test/a", "https://user%40name@a.test/a"]) {
      const source = `![alt](${destination})`;
      const host = render(source);
      expect([destination, host.querySelectorAll("img, .md-image-load").length]).toEqual([destination, 0]);
      expect(host.textContent).toBe(source);
    }
  });

  it("recognizes angle http and email autolinks without rendering other HTML", () => {
    const host = render("<https://a.test/?a=1&b=2> <someone@example.test> <mailto:other@example.test> <script>alert(1)</script>");
    expect([...host.querySelectorAll("a")].map((node) => node.getAttribute("href"))).toEqual([
      "https://a.test/?a=1&b=2", "mailto:someone@example.test", "mailto:other@example.test",
    ]);
    expect(host.querySelectorAll("script")).toHaveLength(0);
    expect(host.textContent).toBe("https://a.test/?a=1&b=2 someone@example.test mailto:other@example.test <script>alert(1)</script>");
  });

  it("keeps footnote identifiers literal before interpreting emphasis", () => {
    const host = document.createElement("div");
    host.innerHTML = markdownHtml("An underscore note[^_id_] and **a note[^other]**; missing[^_missing_].\n\n[^_id_]: First\n[^other]: Second");
    expect([...host.querySelectorAll(".md-footnote-ref a")].map((node) => node.textContent)).toEqual(["1", "2"]);
    expect(host.querySelector("strong .md-footnote-ref")).not.toBeNull();
    expect(host.textContent).toContain("missing[^_missing_]");
  });

  it("protects entire generated unresolved-reference spans from emphasis", () => {
    const host = document.createElement("div");
    host.innerHTML = inlineHtml("@workspace:_missing_ and [[a_b_:x]] and **real**", { workspace: () => null });
    const missing = [...host.querySelectorAll(".md-ref-missing")];
    expect(missing.map((node) => node.textContent)).toEqual(["@workspace:_missing_", "[[a_b_:x]]"]);
    expect(missing.map((node) => node.getAttribute("title"))).toEqual([
      "Not found: no workspace called “_missing_” is on record here.",
      "Not found: no workspace called “a_b_” is on record here.",
    ]);
    expect(host.querySelectorAll("em, .md-ref-missing strong, .md-ref-missing del")).toHaveLength(0);
    expect(host.querySelector("strong")?.textContent).toBe("real");
  });

  it.each(CLOSER_BEFORE_OPENER)("always advances past a closer before an unmatched %s opener", (_kind, source) => {
    const child = renderInChild(source);
    expect(child.error, `${source}: child must finish before its bounded timeout`).toBeUndefined();
    expect(child.status, child.stderr).toBe(0);
    expect(child.signal).toBeNull();
    // Only enter the public API in this worker after the same inline scanner
    // has returned successfully in the bounded child.
    const html = markdownHtml(source, { mode: "inline" });
    expect(html).toBe(child.stdout);
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.textContent).toBe(source);
    expect(host.querySelectorAll("kbd, code, em, strong, del, img")).toHaveLength(0);
  });

  it("bounds malformed linked-image nesting in the expendable child", () => {
    const source = "[![badge](https://img.test/x.svg)](".repeat(1500) + "[";
    const child = renderInChild(source);
    expect(child.error).toBeUndefined();
    expect(child.status, child.stderr).toBe(0);
    expect(child.signal).toBeNull();
  });

  it("handles long unclosed marks in bounded time", () => {
    const source = "[!(**~~`<kbd>".repeat(12000);
    const started = performance.now();
    expect(markdownHtml(source, { mode: "inline" }).length).toBeGreaterThan(0);
    expect(performance.now() - started).toBeLessThan(2000);
  });
});
