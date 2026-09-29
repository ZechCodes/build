/** @vitest-environment jsdom */
// The markdown renderer is an XSS boundary (#229).
//
// Everything an agent or a person writes reaches the page through
// core/markdown.js `markdownHtml`, as innerHTML. The rule that keeps that safe
// is structural: every character of the input is escaped, and the only markup
// that comes out is the renderer's own fixed tags. The only href it writes is a
// route core/router.js wrote; the only style is a table cell's alignment. This
// suite renders hostile input through every mode and every block that can hold
// it, parses the result as a browser would, and checks every element and every
// attribute against that rule — so a new block that forgot to escape fails
// here whatever it forgot.
//
// Since #256 the renderer also writes web links (core/markdownWebLinks.js): an
// `a.md-link` whose href is http:, https: or mailto:, opening in a new browsing
// context with no opener or referrer. Nothing else may carry an href that is
// not a route.

import { afterEach, describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";

/// The tags the renderer may emit, and the attributes each may carry.
const ALLOWED = {
  P: [], BR: [], HR: ["class"], STRONG: [], CODE: ["class"], PRE: ["class"], UL: [], OL: ["start"], LI: [],
  H1: ["id"], H2: ["id"], H3: ["id"], H4: ["id"], H5: ["id"], H6: ["id"],
  BLOCKQUOTE: ["class"], DIV: ["class"], TABLE: [], THEAD: [], TBODY: [], TR: [],
  TH: ["style"], TD: ["style"], A: ["class", "href", "title", "target", "rel"], SPAN: ["class", "title"],
};

/** A route anywhere, or a web link that opens outside with no opener. */
const WEB_HREF = /^(?:https?:\/\/[^/\\]|mailto:)/i;
function hrefAllowed(element, value) {
  if (value.startsWith("#/")) return !element.hasAttribute("target") && !element.hasAttribute("rel");
  return element.className === "md-link" && WEB_HREF.test(value)
    && element.getAttribute("target") === "_blank" && element.getAttribute("rel") === "noopener noreferrer nofollow";
}

/** Every way the rule can be broken, found in one rendering. */
function violations(html) {
  const host = document.createElement("div");
  host.innerHTML = html;
  const found = [];
  for (const element of host.querySelectorAll("*")) {
    const allowed = ALLOWED[element.tagName];
    if (!allowed) found.push(`<${element.tagName.toLowerCase()}>`);
    for (const { name, value } of element.attributes) {
      if (!allowed?.includes(name)) found.push(`${element.tagName.toLowerCase()}[${name}]`);
      if (name === "href" && !hrefAllowed(element, value)) found.push(`href=${value}`);
      if (name === "style" && !/^text-align:(left|right|center)$/.test(value)) found.push(`style=${value}`);
    }
  }
  return found;
}

const PAYLOADS = [
  "<script>alert(1)</script>",
  '<img src=x onerror="alert(1)">',
  "<a href=\"javascript:alert(1)\">x</a>",
  '"><svg onload=alert(1)>',
  "<iframe src=//evil.test></iframe>",
  "[click](javascript:alert(1))",
  "[click](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
  "![pixel](https://evil.test/beacon.png)",
  "<javascript:alert(1)>",
  "<https://evil.test>",
  "`</code><script>alert(1)</script>`",
  "**<b onclick=alert(1)>bold</b>**",
  "&lt;script&gt; and &amp;lt;",
  "[x](JaVa\tScRiPt:alert(1))",
  "[x]( javascript:alert(1))",
  "[x](vbscript:msgbox(1)) [y](file:///etc/passwd) [z](//evil.test)",
  '[x](https://a.test/"onmouseover="alert(1)) https://b.test/"onmouseover="alert(1)',
  '[x](https://a.test "\"><img src=x onerror=alert(1)>")',
  `[x](https://a.test 'x" onmouseover="alert(1)') [y](https://a.test "<img src=x onerror=alert(1)>")`,
  "[<img src=x onerror=alert(1)>](https://a.test) https://a.test/<svg/onload=alert(1)>",
  "[x](https://a.test/`code`) `[y](https://b.test)`",
  // Review #260 bypass probes.
  "[x](&#106;avascript:alert(1)) [y](\uFF4Aavascript:alert(1)) [z](<javascript:alert(1)>) [w](blob:https://a.test/b)",
  "[x](https:\t//evil.test) [y](mailto:a@b.test\r\nBcc:c@d.test) [z](https://a.test/\u0000\"onx=1)",
  "[a **b](https://a.test) c** [[x](https://a.test)](https://b.test)",
];

/** Each payload in every block that can hold it. */
const CONTEXTS = [
  (payload) => payload,
  (payload) => `# ${payload}`,
  (payload) => `###### ${payload}`,
  (payload) => `${payload}\n===`,
  (payload) => `${payload}\n\n---\n\n${payload}\n***`,
  (payload) => `* ---\n> ${payload}\n> - - -`,
  (payload) => `- ${payload}`,
  (payload) => `1. ${payload}\n   > ${payload}`,
  (payload) => `> ${payload}\n>\n> - ${payload}`,
  (payload) => `| h |\n|---|\n| ${payload.replace(/\|/g, "\\|")} |`,
  (payload) => `> | h |\n> |:-:|\n> | ${payload.replace(/\|/g, "\\|")} |`,
  (payload) => `\`\`\`\n${payload}\n\`\`\``,
  (payload) => `> \`\`\`\n> ${payload}\n> \`\`\``,
];

afterEach(() => holdReferenceSources({}));

describe("hostile input through the one renderer", () => {
  it("emits only the renderer's own tags and attributes, in every block and mode", () => {
    for (const payload of PAYLOADS) {
      for (const context of CONTEXTS) {
        const source = context(payload);
        expect([source, violations(markdownHtml(source))]).toEqual([source, []]);
        expect([source, violations(markdownHtml(source, { mode: "inline" }))]).toEqual([source, []]);
      }
    }
  });

  it("keeps what was written readable as text", () => {
    const host = document.createElement("div");
    host.innerHTML = markdownHtml("> <script>alert(1)</script>");
    expect(host.textContent).toBe("<script>alert(1)</script>");
  });

  it("writes no link for a javascript:, data:, vbscript:, file: or protocol-relative URL", () => {
    for (const url of ["javascript:alert(1)", "data:text/html,<script>alert(1)</script>", "JaVaScRiPt:alert(1)",
      "vbscript:msgbox(1)", "file:///etc/passwd", "//evil.test/x", "java\tscript:alert(1)"]) {
      const html = markdownHtml(`[x](${url}) <${url}> ${url}`);
      expect(html).not.toContain("<a ");
    }
  });

  // The negative control for the refusals above and for violations(): the same
  // shapes with an allowed address do link, and pass the rule.
  it("writes a web link for http, https and mailto, and the rule accepts it", () => {
    const html = markdownHtml("[x](https://a.test) [y](mailto:a@b.test) http://c.test");
    expect(html.match(/<a class="md-link"/g)).toHaveLength(3);
    expect(violations(html)).toEqual([]);
  });

  it("rejects a web link that could reach its opener (violations() has teeth)", () => {
    expect(violations('<a class="md-link" href="https://a.test" target="_blank">x</a>')).toEqual(["href=https://a.test"]);
    expect(violations('<a class="md-link" href="javascript:alert(1)" target="_blank" rel="noopener noreferrer nofollow">x</a>'))
      .toEqual(["href=javascript:alert(1)"]);
    expect(violations('<a class="md-ref" href="https://a.test" target="_blank" rel="noopener noreferrer nofollow">x</a>'))
      .toEqual(["href=https://a.test"]);
  });

  it("answers plain text with no markup in plain mode", () => {
    for (const payload of PAYLOADS) {
      expect(markdownHtml(payload, { mode: "plain" })).not.toMatch(/<[a-z!/]/i);
    }
  });
});

// A label is a name somebody chose — a workspace's, an agent's, a task's
// title — and a title attribute is written from it and from what the agent
// typed. Neither may leave its attribute or its element.
describe("names and references that try to leave their attribute", () => {
  const hostile = '"><img src=x onerror=alert(1)> \' onmouseover=\'alert(1)';
  const place = { deviceId: "d1", projectId: "p1" };
  const holdHostileNames = () => holdReferenceSources({
    feed: {
      projects: [{ id: "p1", deviceId: "d1", projectKey: "d1/p1", name: hostile }],
      workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "board", projectKey: "d1/p1",
        directories: [{ source_id: "source-1", name: "a" }, { source_id: "source-2", name: "b" }] }],
      items: [{ projectKey: "d1/p1", entity_id: "ws-1", agents: [{ id: "agent-1", name: hostile }] }],
    },
    tasks: { "d1/p1": [{ id: "task-1", number: 1, title: hostile }] },
  });

  it("escapes a hostile name in every label and title", () => {
    holdHostileNames();
    const source = "#1 #1/c/tc-1 @agent:agent-1 @project:p1 [[board:commit:abcdef12]] [[board:a/x.js#L2]]";
    const html = markdownHtml(source, { place });
    expect(violations(html)).toEqual([]);
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelectorAll("a.md-ref")).toHaveLength(6);
    expect(host.querySelector("a.md-ref").getAttribute("title")).toContain(hostile);
  });

  it("escapes a hostile path and a hostile missing reference", () => {
    holdHostileNames();
    const html = markdownHtml('[[board:a/"><svg onload=alert(1)>.js]] [[nowhere:"onclick=alert(1).js]] #99', { place });
    expect(violations(html)).toEqual([]);
  });

  it("escapes a hostile name through a list, a quote and a table", () => {
    holdHostileNames();
    const html = markdownHtml("- @agent:agent-1\n  > #1\n\n> | h |\n> |---|\n> | @project:p1 |", { place });
    expect(violations(html)).toEqual([]);
  });
});

describe("nesting that tries to exhaust the reader", () => {
  it("bounds a deep quote and reads what is past the bound as text", () => {
    const deep = `${">".repeat(5000)} deep`;
    const started = performance.now();
    const html = markdownHtml(deep);
    expect(performance.now() - started).toBeLessThan(1000);
    expect(violations(html)).toEqual([]);
    expect((html.match(/<blockquote/g) || []).length).toBeLessThanOrEqual(8);
    expect(html).toContain("deep");
  });

  it("bounds a deep list", () => {
    const deep = Array.from({ length: 400 }, (_, level) => `${"  ".repeat(level)}- level ${level}`).join("\n");
    const started = performance.now();
    const html = markdownHtml(deep);
    expect(performance.now() - started).toBeLessThan(1000);
    expect(violations(html)).toEqual([]);
    expect((html.match(/<ul>/g) || []).length).toBeLessThanOrEqual(8);
    expect(html).toContain("level 399");
  });

  it("stays linear on a long document of mixed quotes and lists", () => {
    const block = "> - item\n>   > quoted <b>x</b>\n> | a |\n> |---|\n> | 1 |\n\n- one\n  - two\n    > three\n\n";
    const started = performance.now();
    const html = markdownHtml(block.repeat(2000));
    expect(performance.now() - started).toBeLessThan(3000);
    expect(violations(html)).toEqual([]);
  });

  // #238: the list reader looked for the next filled line again at every blank
  // one, so a run of blank lines after an item took quadratic time.
  it("reads a long run of blank lines after a list item in linear time, plain and quoted", () => {
    const blanks = "\n".repeat(40000);
    for (const source of [`- a${blanks}- b`, `> - a${blanks.replace(/\n/g, "\n>")} - b`]) {
      const started = performance.now();
      const html = markdownHtml(source);
      expect(performance.now() - started).toBeLessThan(500);
      expect(html).toContain("<li>a</li><li>b</li>");
    }
  });

  // #256: the web-link readers see brackets, parentheses and addresses, and
  // none of them may go back over what it has read.
  it("reads long runs of link punctuation in linear time", () => {
    const n = 100000;
    const sources = [
      "[".repeat(n), "]".repeat(n), "(".repeat(n), ")".repeat(n), "](".repeat(n / 2), "[](".repeat(n / 3),
      "[a](".repeat(n / 4), "[a](b".repeat(n / 5), "[a](b(".repeat(n / 6), `[a](b ${"&quot;".repeat(n / 6)}`,
      '[a](b "'.repeat(n / 7), "https://".repeat(n / 8), `https://a.test/${")".repeat(n)}`, `https://a.test/${"(".repeat(n)}.`,
      `https://a.test/${".".repeat(n)}x`, "`[a](".repeat(n / 5), `[${"`x`".repeat(n / 3)}](https://a.test)`,
    ];
    for (const source of sources) {
      for (const mode of ["block", "inline"]) {
        const started = performance.now();
        markdownHtml(source, { mode });
        expect([source.slice(0, 12), mode, performance.now() - started < 500]).toEqual([source.slice(0, 12), mode, true]);
      }
    }
  });

  it("leaves an unclosed fence inside a quote closed at the quote's end", () => {
    const html = markdownHtml("> ```\n> <b>code\nafter");
    expect(violations(html)).toEqual([]);
    expect(html).toContain("</code></pre></blockquote><p>after</p>");
  });
});
