/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";

const render = (source) => {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source, { mode: "inline" });
  return host;
};

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

  it("renders image alt and title as escaped attributes with private loading defaults", () => {
    const host = render('![a " <b> & #42](https://example.test/a.png "A title")');
    const image = host.querySelector("img");
    expect(image?.getAttribute("src")).toBe("https://example.test/a.png");
    expect(image?.getAttribute("alt")).toBe('a " <b> & #42');
    expect(image?.getAttribute("title")).toBe("A title");
    expect(image?.getAttribute("loading")).toBe("lazy");
    expect(image?.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(image?.getAttributeNames().sort()).toEqual(["alt", "loading", "referrerpolicy", "src", "title"]);
    expect(host.querySelectorAll("a, b")).toHaveLength(0);
  });

  it("refuses unsafe images and credentials while preserving their literal text", () => {
    for (const destination of ["javascript:alert(1)", "data:image/svg+xml,x", "mailto:a@b.test", "file:///tmp/a", "//a.test/a", "/a", "https://user:secret@a.test/a", "https://user%40name@a.test/a"]) {
      const source = `![alt](${destination})`;
      const host = render(source);
      expect([destination, host.querySelectorAll("img").length]).toEqual([destination, 0]);
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

  it("handles long unclosed marks in bounded time", () => {
    const source = "[!(**~~`<kbd>".repeat(12000);
    const started = performance.now();
    expect(markdownHtml(source, { mode: "inline" }).length).toBeGreaterThan(0);
    expect(performance.now() - started).toBeLessThan(2000);
  });
});
