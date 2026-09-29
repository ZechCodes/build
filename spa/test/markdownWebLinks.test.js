/** @vitest-environment jsdom */
// Web links in what an agent writes (#256): `[text](url)`, `[text](url "title")`
// and a bare `https://…` in prose become links that open outside Build. Only
// http:, https: and mailto: are ever a link; anything else stays the words
// the agent typed.

import { afterEach, describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";
import { safeWebHref } from "../src/core/markdownWebLinks.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";

/** The rendered markup, parsed as a browser would. */
function rendered(source, options) {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source, options);
  return host;
}

const webLinksIn = (host) => [...host.querySelectorAll("a.md-link")];
const hrefs = (source, options) => webLinksIn(rendered(source, options)).map((link) => link.getAttribute("href"));

afterEach(() => holdReferenceSources({}));

describe("inline links", () => {
  it("renders the reported message as a link", () => {
    const host = rendered("See [OpenAI's documentation](https://developers.openai.com/api/docs/models/gpt-6.1-sol).");
    const [link] = webLinksIn(host);
    expect(link.textContent).toBe("OpenAI's documentation");
    expect(link.getAttribute("href")).toBe("https://developers.openai.com/api/docs/models/gpt-6.1-sol");
    expect(host.textContent).toBe("See OpenAI's documentation.");
  });

  it("opens outside Build and tells the target nothing", () => {
    const [link] = webLinksIn(rendered("[docs](https://example.test/a)"));
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer nofollow");
  });

  it("carries a title in either quote", () => {
    expect(webLinksIn(rendered('[a](https://example.test "The title")'))[0].getAttribute("title")).toBe("The title");
    expect(webLinksIn(rendered("[a](https://example.test 'Single')"))[0].getAttribute("title")).toBe("Single");
  });

  it("keeps balanced parentheses in the address", () => {
    expect(hrefs("[w](https://en.wikipedia.org/wiki/Foo_(bar)) after")).toEqual(["https://en.wikipedia.org/wiki/Foo_(bar)"]);
  });

  it("keeps the label's own marks", () => {
    const [link] = webLinksIn(rendered("[**bold** and `code`](https://example.test)"));
    expect(link.innerHTML).toBe("<strong>bold</strong> and <code>code</code>");
  });

  it("links mailto: but only as an inline link", () => {
    expect(hrefs("[mail](mailto:someone@example.test)")).toEqual(["mailto:someone@example.test"]);
    expect(hrefs("mailto:someone@example.test")).toEqual([]);
  });

  it("links in inline mode too", () => {
    expect(hrefs("a [b](https://example.test/b)\nhttps://example.test/c", { mode: "inline" }))
      .toEqual(["https://example.test/b", "https://example.test/c"]);
  });

  it("reads a link inside a list, a quote, a heading and a table cell", () => {
    const source = "# [h](https://h.test)\n\n- [l](https://l.test)\n\n> [q](https://q.test)\n\n| c |\n|---|\n| [t](https://t.test) |";
    expect(hrefs(source)).toEqual(["https://h.test", "https://l.test", "https://q.test", "https://t.test"]);
  });

  it("takes the plain words in plain mode", () => {
    expect(markdownHtml("See [the docs](https://example.test/x) now", { mode: "plain" })).toBe("See the docs now");
  });
});

describe("bare URLs", () => {
  it("links http and https in prose", () => {
    expect(hrefs("see https://example.test/a and http://example.test/b")).toEqual(["https://example.test/a", "http://example.test/b"]);
  });

  it("leaves trailing punctuation outside the link", () => {
    for (const mark of [".", ",", ")", "]", "!", "?", ":", ";"]) {
      expect([mark, hrefs(`go to https://example.test/path${mark} now`)]).toEqual([mark, ["https://example.test/path"]]);
    }
    expect(hrefs("(see https://example.test/a)")).toEqual(["https://example.test/a"]);
    expect(hrefs("see https://example.test/a...")).toEqual(["https://example.test/a"]);
  });

  it("keeps balanced parentheses and an escaped ampersand", () => {
    expect(hrefs("https://en.wikipedia.org/wiki/Foo_(bar).")).toEqual(["https://en.wikipedia.org/wiki/Foo_(bar)"]);
    expect(hrefs("https://example.test/?a=1&b=2")).toEqual(["https://example.test/?a=1&b=2"]);
  });

  it("stops at a quote or an angle bracket", () => {
    expect(hrefs('"https://example.test/q" and <https://example.test/r>')).toEqual(["https://example.test/q", "https://example.test/r"]);
  });

  it("does not link a scheme with nothing after it, or a scheme glued to a word", () => {
    expect(hrefs("https:// and xhttps://example.test")).toEqual([]);
  });

  it("does not link a URL twice when it is an inline link's label", () => {
    expect(hrefs("[https://a.test](https://b.test)")).toEqual(["https://b.test"]);
  });
});

describe("code stays literal", () => {
  it("links nothing inside a code span", () => {
    const host = rendered("`[x](https://a.test)` and `https://b.test` beside https://c.test");
    expect(webLinksIn(host).map((link) => link.getAttribute("href"))).toEqual(["https://c.test"]);
    expect(host.querySelectorAll("code")[0].textContent).toBe("[x](https://a.test)");
  });

  it("links nothing inside a fence", () => {
    expect(hrefs("```\n[x](https://a.test) https://b.test\n```")).toEqual([]);
  });
});

describe("references beside web links", () => {
  const place = { deviceId: "d1", projectId: "p1" };
  const holdOneTask = () => holdReferenceSources({
    feed: { projects: [{ id: "p1", deviceId: "d1", projectKey: "d1/p1", name: "Build" }], workspaces: [], items: [] },
    tasks: { "d1/p1": [{ id: "task-42", number: 42, title: "Rebuild" }] },
  });

  it("links a reference beside a web link and never inside one", () => {
    holdOneTask();
    const host = rendered("[see #42](https://a.test/#42) then #42 and https://b.test/#42", { place });
    expect(webLinksIn(host).map((link) => link.getAttribute("href"))).toEqual(["https://a.test/#42", "https://b.test/#42"]);
    expect(host.querySelectorAll("a.md-ref")).toHaveLength(1);
    expect(host.querySelectorAll("a a")).toHaveLength(0);
  });

  it("leaves a bracketed workspace reference to the reference reader", () => {
    const host = rendered("[[board:a.js]] and [[board:b.js]](https://x.test)");
    expect(webLinksIn(host).map((link) => link.getAttribute("href"))).toEqual(["https://x.test"]);
    expect(host.textContent).toBe("[[board:a.js]] and [[board:b.js]](https://x.test)");
  });
});

// Every refusal is its own negative control: the same shape with an allowed
// address links, so the refusal is the scheme's and not the shape's.
describe("addresses that are refused", () => {
  const REFUSED = [
    "javascript:alert(1)",
    "JaVaScRiPt:alert(1)",
    "data:text/html;base64,PHNjcmlwdD4=",
    "vbscript:msgbox(1)",
    "file:///etc/passwd",
    "//evil.test/x",
    "#42",
    "#/task/1",
    "src/core/markdown.js",
    "/absolute/path",
    "ftp://example.test/x",
    "http:evil.test",
    "https:/evil.test",
    "https:///evil.test",
    "https:\\\\evil.test",
    "javascript&#58;alert(1)",
  ];

  it("renders each refused inline link as the words typed", () => {
    for (const url of REFUSED) {
      const source = `[x](${url})`;
      const host = rendered(source);
      expect([url, host.querySelectorAll("a").length]).toEqual([url, 0]);
      expect([url, host.textContent]).toEqual([url, source]);
    }
  });

  it("links the same shape with an allowed address (negative control)", () => {
    expect(hrefs("[x](https://example.test/x)")).toEqual(["https://example.test/x"]);
  });

  it("refuses a scheme hidden by case, whitespace or control characters", () => {
    for (const url of ["JaVa\tScRiPt:alert(1)", " javascript:alert(1)", "java\nscript:alert(1)", "java\u0000script:alert(1)",
      "\u0001javascript:alert(1)", "javascript\u007f:alert(1)", "data\u0085:x"]) {
      expect([JSON.stringify(url), safeWebHref(url)]).toEqual([JSON.stringify(url), null]);
    }
    expect(safeWebHref(" https://example.test/\t")).toBe("https://example.test/");
    expect(safeWebHref("HTTPS://Example.test")).toBe("HTTPS://Example.test");
  });

  it("escapes the address and the label", () => {
    const host = rendered(`[<img src=x onerror=alert(1)>](https://a.test/?q="'><svg/onload=alert(1)>) https://b.test/"onmouseover=x`);
    expect(host.querySelectorAll("img, svg")).toHaveLength(0);
    const [first, second] = webLinksIn(host);
    expect(first.getAttribute("href")).toBe("https://a.test/?q=\"'><svg/onload=alert(1)>");
    expect(first.textContent).toBe("<img src=x onerror=alert(1)>");
    expect(second.getAttribute("href")).toBe("https://b.test/");
    for (const link of webLinksIn(host)) expect(link.getAttributeNames().sort()).toEqual(["class", "href", "rel", "target"]);
  });
});
