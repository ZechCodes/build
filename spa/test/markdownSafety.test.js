/** @vitest-environment jsdom */
// The markdown renderer is an XSS boundary (#229).
//
// Everything an agent or a person writes reaches the page through
// core/markdown.js `markdownHtml`, as innerHTML. The rule that keeps that safe
// is structural: every character of the input is escaped, and the only markup
// that comes out is the renderer's own fixed tags. Links, images and footnote
// fragments each have a separate policy; only table alignment uses style. This
// suite renders hostile input through every mode and every block that can hold
// it, parses the result as a browser would, and checks every element and every
// attribute against that rule — so a new block that forgot to escape fails
// here whatever it forgot.
//
// Since #256 the renderer also writes web links (core/markdownWebLinks.js): an
// `a.md-link` whose href is http:, https: or mailto:, opening in a new browsing
// context with no opener or referrer. Nothing else may carry an href that is
// not a route or an exact generated document-local footnote fragment.

import { afterEach, describe, expect, it } from "vitest";

import { markdownHtml } from "../src/core/markdown.js";
import { holdReferenceSources } from "../src/core/referenceIndex.js";

/// The tags the renderer may emit, and the attributes each may carry.
const ALLOWED = {
  P: ["class"], BR: [], HR: ["class"], STRONG: [], EM: [], DEL: [], KBD: [], CODE: ["class"], PRE: ["class"], UL: [], OL: ["start"], LI: ["class", "id"],
  INPUT: ["type", "class", "disabled", "checked", "aria-label", "aria-labelledby", "data-task-index", "data-task-offset"],
  H1: ["id"], H2: ["id"], H3: ["id"], H4: ["id"], H5: ["id"], H6: ["id"],
  BLOCKQUOTE: ["class"], DIV: ["class", "tabindex", "role", "aria-label"], TABLE: [], THEAD: [], TBODY: [], TR: [],
  TH: ["style", "scope"], TD: ["style", "class"], A: ["class", "href", "title", "target", "rel", "id", "aria-label"], SPAN: ["class", "title", "id"],
  IMG: ["src", "alt", "loading", "referrerpolicy", "title"], DETAILS: ["class"], SUMMARY: [], SUP: ["class"], SECTION: ["class", "aria-label"],
  BUTTON: ["type", "class", "data-md-image-src", "data-md-image-alt", "data-md-image-title"],
};

const ENCODED_SCOPE = "(?:[0-9a-f]+(?:-[0-9a-f]+)*)?";
const TASK_LABEL_ID = new RegExp(`^md-task-label:${ENCODED_SCOPE}:\\d+$`);
const FOOTNOTE_ID = new RegExp(`^md-fn:(${ENCODED_SCOPE}):([1-9]\\d*)$`);
const FOOTNOTE_REF_ID = new RegExp(`^md-fnref:(${ENCODED_SCOPE}):([1-9]\\d*):([1-9]\\d*)$`);
const LANGUAGE_CLASS = /^language-(?:markup|clike|css|javascript|jsx|typescript|tsx|python|rust|json|bash|yaml|toml|markdown)$/;
// Token names and aliases from the renderer's registered Prism grammars.
const PRISM_CLASSES = new Set((
  'annotation assign-left at atrule attr-equals attr-name attr-value attribute bash blockquote bold boolean builtin cdata char class-name '
  + 'closure-params closure-punctuation code code-block code-language code-snippet comment constant content conversion-option css date datetime '
  + 'decorator directive doctype doctype-tag entity environment file-descriptor for-or-select format-spec fragment-specifier front-matter '
  + 'front-matter-block function function-definition function-name function-variable generic generic-function hashbang hr important included-cdata '
  + 'internal-subset interpolation interpolation-punctuation italic javascript key keyword language-css language-javascript language-regex language-yaml '
  + 'lifetime-annotation list literal-property macro module-declaration name named-entity namespace null number operator parameter prolog property '
  + 'punctuation regex regex-delimiter regex-flags regex-source rule scalar script script-punctuation selector selector-function-argument shebang '
  + 'special-attr spread strike string string-interpolation string-property style symbol table table-data table-data-rows table-header table-header-row '
  + 'table-line tag template-punctuation template-string title triple-quoted-string type-definition url url-reference value variable yaml'
).split(' '));
const FIXED_CLASSES = {
  INPUT: ["md-task-checkbox"], P: ["md-alert-title"], HR: ["md-rule"], PRE: ["md-code"], LI: ["task-item"],
  BLOCKQUOTE: ["md-quote", ...["note", "tip", "important", "warning", "caution"].map((kind) => `md-alert md-alert-${kind}`)],
  DIV: ["mdtable", "md-code-block", "md-code-label"], TD: ["md-cell-short"], A: ["md-ref", "md-link", "md-footnote-backref"],
  DETAILS: ["md-details"], SUP: ["md-footnote-ref"], SECTION: ["md-footnotes"],
  BUTTON: ["md-image-load"],
};

function checklistNameAllowed(input, host) {
  const reference = input.getAttribute("aria-labelledby");
  if (reference === null) return Boolean(input.getAttribute("aria-label")?.trim());
  if (!TASK_LABEL_ID.test(reference) || input.hasAttribute("aria-label")) return false;
  const labels = [...host.querySelectorAll("[id]")].filter((element) => element.id === reference);
  return labels.length === 1 && labels[0].classList.contains("md-task-label")
    && Boolean(labels[0].textContent.trim()) && !labels[0].querySelector("input");
}

function imageAddressAllowed(value) {
  try {
    const url = new URL(value);
    return /^https?:\/\/[^/\\]/i.test(value) && Boolean(url.hostname) && !url.username && !url.password
      && !/^https?:\/\/[^/\\]*@/i.test(value)
      && !/[\u0000- \u007f-\u009f]/.test(value);
  } catch {
    return false;
  }
}

function imageAllowed(element) {
  return imageAddressAllowed(element.getAttribute("src")) && element.hasAttribute("alt")
    && element.getAttribute("loading") === "lazy" && element.getAttribute("referrerpolicy") === "no-referrer";
}

function imagePlaceholderAllowed(element) {
  const source = element.dataset.mdImageSrc;
  if (!imageAddressAllowed(source) || element.getAttribute("type") !== "button" || element.className !== "md-image-load"
    || !element.hasAttribute("data-md-image-alt") || element.children.length) return false;
  const alt = element.dataset.mdImageAlt;
  return element.textContent === `Load image from ${new URL(source).hostname}${alt ? ` — ${alt}` : ""}`;
}

function footnoteHrefAllowed(element, value, ids) {
  const target = ids.get(value.slice(1));
  if (!target || target.length !== 1 || element.hasAttribute("target") || element.hasAttribute("rel") || element.hasAttribute("title")) return false;
  const reference = FOOTNOTE_REF_ID.exec(element.id);
  if (reference) return element.parentElement?.className === "md-footnote-ref"
    && value === `#md-fn:${reference[1]}:${reference[2]}` && target[0].tagName === "LI"
    && element.textContent === reference[2] && element.getAttribute("aria-label") === `Footnote ${reference[2]}`;
  const back = FOOTNOTE_REF_ID.exec(value.slice(1));
  return Boolean(back) && element.className === "md-footnote-backref" && !element.hasAttribute("id")
    && element.closest("li")?.id === `md-fn:${back[1]}:${back[2]}` && target[0].parentElement?.className === "md-footnote-ref"
    && element.textContent === "↩" && [ `Back to reference ${back[2]}`, `Back to reference ${back[2]} (${back[3]})` ].includes(element.getAttribute("aria-label"));
}

/** Build routes, external web links, and exact document-local footnote pairs. */
const WEB_HREF = /^(?:https?:\/\/[^/\\]|mailto:[^@]+@[^@]+$)/i;
function hrefAllowed(element, value, ids) {
  if (value.startsWith("#/")) return element.className === "md-ref" && !element.hasAttribute("target") && !element.hasAttribute("rel");
  if (value.startsWith("#md-fn")) return footnoteHrefAllowed(element, value, ids);
  return element.className === "md-link" && WEB_HREF.test(value) && !/[\u0000- \u007f-\u009f]/.test(value)
    && element.getAttribute("target") === "_blank" && element.getAttribute("rel") === "noopener noreferrer nofollow";
}

function spanClassAllowed(element) {
  if (["md-ref-code", "md-ref-missing", "md-task-label", "md-task-label sr-only"].includes(element.className)) return true;
  // Markdown's nested fenced-code alias is sanitized by Prism to this grammar.
  const tokenClasses = [...element.classList].slice(1);
  const knownToken = tokenClasses.every((word) => PRISM_CLASSES.has(word)
    || (element.classList.contains("code-block") && /^language-(?:[a-z][\w-]*)?$/.test(word)));
  return element.classList[0] === "token" && tokenClasses.length > 0 && knownToken
    && LANGUAGE_CLASS.test(element.closest("code")?.className || "") && element.closest("pre")?.className === "md-code";
}

function classAllowed(element) {
  if (element.tagName === "SPAN") return spanClassAllowed(element);
  if (element.tagName === "CODE") return LANGUAGE_CLASS.test(element.className) && element.parentElement?.className === "md-code";
  return FIXED_CLASSES[element.tagName]?.includes(element.className);
}

function idAllowed(element) {
  if (/^H[1-6]$/.test(element.tagName)) return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(element.id);
  if (element.tagName === "LI") return FOOTNOTE_ID.test(element.id) && element.closest("section")?.className === "md-footnotes";
  if (element.tagName === "A") return FOOTNOTE_REF_ID.test(element.id) && element.parentElement?.className === "md-footnote-ref";
  return element.tagName === "SPAN" && TASK_LABEL_ID.test(element.id)
    && ["md-task-label", "md-task-label sr-only"].includes(element.className);
}

function attributeViolation(element, name, value, ids) {
  if (name === "href" && !hrefAllowed(element, value, ids)) return `href=${value}`;
  if (name === "style" && !/^text-align:(left|right|center)$/.test(value)) return `style=${value}`;
  if (name === "class" && !classAllowed(element)) return `class=${value}`;
  if (name === "start" && !/^\d{1,9}$/.test(value)) return `start=${value}`;
  if (name === "scope" && value !== "col") return `scope=${value}`;
  if (name === "id" && !idAllowed(element)) return element.tagName === "SPAN" ? "invalid checklist label" : `id=${value}`;
  if (["target", "rel"].includes(name) && element.className !== "md-link") return `${name}=${value}`;
  if (name === "aria-label" && !accessibleNameAllowed(element, value)) return `aria-label=${value}`;
  return null;
}

function accessibleNameAllowed(element, value) {
  if (element.tagName === "INPUT") return Boolean(value.trim());
  if (element.tagName === "SECTION") return element.className === "md-footnotes" && value === "Footnotes";
  if (element.tagName === "DIV") return element.className === "mdtable" && value === "Table";
  return element.tagName === "A" && (FOOTNOTE_REF_ID.test(element.id) || element.className === "md-footnote-backref");
}

function generatedStructureAllowed(element) {
  if (["HR", "PRE", "BLOCKQUOTE", "DIV", "DETAILS", "SUP", "SECTION"].includes(element.tagName)) return classAllowed(element);
  if (element.tagName === "SUMMARY") return element.parentElement?.className === "md-details" && element.parentElement.firstElementChild === element;
  if (element.tagName === "TH") return element.getAttribute("scope") === "col";
  if (element.tagName === "KBD") return !element.children.length;
  if (element.tagName === "A") return element.hasAttribute("href");
  return true;
}

function elementViolations(element, host) {
  const found = [];
  if (!generatedStructureAllowed(element)) found.push(`invalid ${element.tagName.toLowerCase()} structure`);
  if (element.tagName === "INPUT" && (element.type !== "checkbox" || element.className !== "md-task-checkbox"
    || !/^\d+$/.test(element.dataset.taskIndex || "") || !/^\d+$/.test(element.dataset.taskOffset || "")
    || !checklistNameAllowed(element, host))) found.push("invalid checklist input");
  if (element.tagName === "IMG" && !imageAllowed(element)) found.push("invalid image");
  if (element.tagName === "BUTTON" && !imagePlaceholderAllowed(element)) found.push("invalid image placeholder");
  if (element.tagName === "SECTION" && element.getAttribute("aria-label") !== "Footnotes") found.push("invalid footnote section");
  if (element.className === "mdtable" || element.hasAttribute("role") || element.hasAttribute("tabindex")) {
    if (element.className !== "mdtable" || element.getAttribute("role") !== "region" || ![null, "0"].includes(element.getAttribute("tabindex"))
      || element.getAttribute("aria-label") !== "Table") found.push("invalid table region");
  }
  return found;
}

/** Every way the rule can be broken, found in one browser-parsed rendering. */
function violations(html) {
  const host = document.createElement("div");
  host.innerHTML = html;
  const ids = new Map();
  for (const element of host.querySelectorAll("[id]")) ids.set(element.id, [...(ids.get(element.id) || []), element]);
  const found = [];
  for (const element of host.querySelectorAll("*")) {
    const allowed = ALLOWED[element.tagName];
    if (!allowed) found.push(`<${element.tagName.toLowerCase()}>`);
    found.push(...elementViolations(element, host));
    for (const { name, value } of element.attributes) {
      if (!allowed?.includes(name)) found.push(`${element.tagName.toLowerCase()}[${name}]`);
      const violation = attributeViolation(element, name, value, ids);
      if (violation) found.push(violation);
    }
  }
  return found;
}

const PAYLOADS = [
  "<script>alert(1)</script>",
  '<img src=x onerror="alert(1)">',
  '<button type="button" class="md-image-load" data-md-image-src="https://a.test/pixel">load</button>',
  "<a href=\"javascript:alert(1)\">x</a>",
  '"><svg onload=alert(1)>',
  "<iframe src=//evil.test></iframe>",
  "[click](javascript:alert(1))",
  "[click](data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==)",
  "![pixel](https://evil.test/beacon.png)",
  '![\"><svg onload=alert(1)>](https://a.test/p.png "\"><img src=x onerror=alert(1)>")',
  "![x](javascript:alert(1)) ![x](data:image/svg+xml,<svg/onload=alert(1)>) ![x](mailto:x@y.test)",
  "![x](https://user:secret@a.test/x)",
  "*<svg onload=alert(1)>* _<img src=x onerror=alert(1)>_ ~~<script>x</script>~~",
  "<kbd onclick=alert(1)>x</kbd> <KBD>x</KBD> <kbd><img src=x onerror=alert(1)></kbd>",
  "<kbd>https://a.test **x** &lt;img&gt;</kbd> `` `</code><svg/onload=alert(1)> ` ``",
  "<a@b.test> <mailto:a@b.test>",
  "[^x\"><svg/onload=alert(1)>] [^x]: <img src=x onerror=alert(1)>",
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
  (payload) => `- [ ] ${payload}`,
  (payload) => `> 2. [X] ${payload}`,
  (payload) => `1. ${payload}\n   > ${payload}`,
  (payload) => `> ${payload}\n>\n> - ${payload}`,
  (payload) => `| h |\n|---|\n| ${payload.replace(/\|/g, "\\|")} |`,
  (payload) => `> | h |\n> |:-:|\n> | ${payload.replace(/\|/g, "\\|")} |`,
  (payload) => `\`\`\`\n${payload}\n\`\`\``,
  (payload) => `> \`\`\`\n> ${payload}\n> \`\`\``,
  (payload) => `\`\`\`javascript\n${payload}\n\`\`\``,
  (payload) => `~~~html\n${payload}\n~~~`,
  (payload) => `~~~~markdown\n~~~custom-language\n${payload}\n~~~\n~~~~`,
  (payload) => `> [!WARNING]\n> ${payload}`,
  (payload) => `<details>\n<summary>${payload}</summary>\n\n${payload}\n</details>`,
  (payload) => `Reference[^safe]\n\n[^safe]: ${payload}`,
  (payload) => `*${payload}* ~~${payload}~~`,
];

afterEach(() => holdReferenceSources({}));

describe("hostile input through the one renderer", () => {
  it("emits only the renderer's own tags and attributes, in every block and mode", () => {
    for (const payload of PAYLOADS) {
      for (const context of CONTEXTS) {
        const source = context(payload);
        expect([source, violations(markdownHtml(source))]).toEqual([source, []]);
        expect([source, violations(markdownHtml(source, { mode: "inline" }))]).toEqual([source, []]);
        expect([source, violations(markdownHtml(source, { taskItems: true }))]).toEqual([source, []]);
        expect([source, violations(markdownHtml(source, { taskItems: true, taskLabelScope: '\"><svg onload=alert(1)>' }))]).toEqual([source, []]);
      }
    }
  });

  it("accepts the exact generated rich vocabulary", () => {
    const source = '*em* ~~gone~~ <kbd>Ctrl &amp; K</kbd> ![alt](https://a.test/x.png "title") <a@b.test>\n\n'
      + '> [!CAUTION]\n> Watch[^n]\n\n<details>\n<summary>More</summary>\n\nBody\n</details>\n\n'
      + '```javascript\nconst x = "<svg onload=alert(1)>";\n```\n\n[^n]: **Note**';
    const html = markdownHtml(source, { taskLabelScope: '\"><svg onload=alert(1)>' });
    const host = document.createElement("div");
    host.innerHTML = html;
    for (const selector of ["em", "del", "kbd", "button.md-image-load", ".md-link", ".md-alert-caution", "details > summary", "code.language-javascript .token", ".md-footnotes", ".md-footnote-ref"]) {
      expect(host.querySelector(selector), selector).not.toBeNull();
    }
    expect(violations(html)).toEqual([]);
  });

  it("keeps what was written readable as text", () => {
    const host = document.createElement("div");
    host.innerHTML = markdownHtml("> <script>alert(1)</script>");
    expect(host.textContent).toBe("<script>alert(1)</script>");
  });

  it("rejects inputs outside the fixed checklist vocabulary", () => {
    expect(violations('<input type="text" class="md-task-checkbox" aria-label="x" data-task-index="0" data-task-offset="3">'))
      .toEqual(["invalid checklist input"]);
    expect(violations('<input type="checkbox" class="md-task-checkbox" aria-label="x" data-task-index="0&quot; onclick=x" data-task-offset="3">'))
      .toEqual(["invalid checklist input"]);
    expect(violations('<input type="checkbox" class="md-task-checkbox" aria-label="x" data-task-index="0" data-task-offset="3" onchange="evil()">'))
      .toEqual(["input[onchange]"]);
  });

  it("requires a scoped name to reference one renderer-owned label", () => {
    const input = '<input type="checkbox" class="md-task-checkbox" data-task-index="0" data-task-offset="3" aria-labelledby="md-task-label:61:0">';
    const label = '<span class="md-task-label" id="md-task-label:61:0">Ship</span>';
    expect(violations(input + label)).toEqual([]);
    expect(violations(input)).toEqual(["invalid checklist input"]);
    expect(violations(input + label + label)).toEqual(["invalid checklist input"]);
    expect(violations(input + '<span id="md-task-label:61:0">Ship</span>'))
      .toEqual(["invalid checklist input", "invalid checklist label"]);
    expect(violations(input.replace('aria-labelledby=', 'aria-label="raw" aria-labelledby=') + label))
      .toEqual(["invalid checklist input"]);
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
    expect(violations('<a class="md-link" href="https://a.test" rel="noopener noreferrer nofollow">x</a>')).toEqual(["href=https://a.test"]);
    expect(violations('<a class="md-link" href="javascript:alert(1)" target="_blank" rel="noopener noreferrer nofollow">x</a>'))
      .toEqual(["href=javascript:alert(1)"]);
    expect(violations('<a class="md-ref" href="https://a.test" target="_blank" rel="noopener noreferrer nofollow">x</a>'))
      .toEqual(["href=https://a.test", "target=_blank", "rel=noopener noreferrer nofollow"]);
  });

  it("rejects image sources and attributes outside the generated image policy", () => {
    const image = (src, extra = '') => `<img src="${src}" alt="x" loading="lazy" referrerpolicy="no-referrer"${extra}>`;
    expect(violations(image("https://a.test/x.png"))).toEqual([]);
    for (const src of ["javascript:alert(1)", "data:image/svg+xml,x", "mailto:a@b.test", "//a.test/x", "file:///etc/passwd", "https://user:secret@a.test/x", "https://@a.test/x", "https:///"]) {
      expect(violations(image(src)), src).toContain("invalid image");
      expect(markdownHtml(`![x](${src})`), src).not.toContain("<img ");
      expect(markdownHtml(`![x](${src})`), src).not.toContain('class="md-image-load"');
    }
    expect(violations(image("https://a.test/x", ' onerror="evil()"'))).toContain("img[onerror]");
    expect(violations(image("https://a.test/x").replace('no-referrer', 'origin'))).toContain("invalid image");
    expect(violations(image("https://a.test/x").replace('loading="lazy"', 'loading="eager"'))).toContain("invalid image");
    expect(violations(image("https://a.test/&#9;x"))).toContain("invalid image");
  });

  it("escapes image text and protects literal and attribute contents from later syntax", () => {
    const source = '![**alt** `literal` <svg onload=evil()>[^n]](https://a.test/x_~~q~~_.png "#1 **title**") '
      + '[**label**](https://a.test/_path_ "#1 *title*") '
      + '`[^n] <kbd>x</kbd> https://a.test` <kbd>[^n] **x** https://a.test</kbd>\n\n[^n]: unused';
    const host = document.createElement("div");
    const html = markdownHtml(source);
    host.innerHTML = html;
    expect(violations(html)).toEqual([]);
    expect(host.querySelector("button").dataset.mdImageAlt).toBe('**alt** literal <svg onload=evil()>[^n]');
    expect(host.querySelector("button").dataset.mdImageSrc).toBe('https://a.test/x_~~q~~_.png');
    expect(host.querySelector("button").dataset.mdImageTitle).toBe('#1 **title**');
    expect(host.querySelector("a").getAttribute("title")).toBe('#1 *title*');
    expect(host.querySelectorAll("a")).toHaveLength(1);
    expect(host.querySelector("kbd").innerHTML).toBe('[^n] **x** https://a.test');
    expect(host.querySelectorAll(".md-footnote-ref, .md-footnotes, svg")).toHaveLength(0);
  });

  it("keeps external image requests inert until explicit activation", () => {
    const source = '![external](https://external.test/pixel.png "Title")';
    const html = markdownHtml(source, { imageOrigin: "https://build.test" });
    const host = document.createElement("div");
    host.innerHTML = html;
    expect(host.querySelector("img, [src], [srcset]")).toBeNull();
    expect(host.querySelector("button").type).toBe("button");
    expect(host.querySelector("button").textContent).toBe("Load image from external.test — external");
    expect(violations(html)).toEqual([]);
    const local = markdownHtml('![local](https://build.test/local.png)', { imageOrigin: "https://build.test" });
    host.innerHTML = local;
    expect(host.querySelector("img").getAttribute("src")).toBe("https://build.test/local.png");
    expect(violations(local)).toEqual([]);
    expect(violations(html.replace('type="button"', 'type="submit"'))).toContain("invalid image placeholder");
    expect(violations(html.replace('data-md-image-src="https://external.test/pixel.png"', 'data-md-image-src="javascript:evil()"')))
      .toContain("invalid image placeholder");
    expect(violations(html.replace('data-md-image-alt=', 'onclick="evil()" data-md-image-alt='))).toContain("button[onclick]");
  });

  it("keeps arbitrary HTML inert beside the exact keyboard and details productions", () => {
    for (const source of [
      '<kbd onclick="evil()">key</kbd>', '<KBD>key</KBD>', '<kbd><img src=x onerror=evil()></kbd>',
      '<details open>\n<summary>Title</summary>\nBody\n</details>',
      '<details>\n<summary onclick="evil()">Title</summary>\nBody\n</details>',
      '<DETAILS>\n<summary>Title</summary>\nBody\n</DETAILS>',
      '<details>\n<summary>Title</summary>\nBody',
      '<button type="button" class="md-image-load" data-md-image-src="https://a.test/pixel">load</button>',
    ]) {
      const host = document.createElement("div");
      const html = markdownHtml(source);
      host.innerHTML = html;
      expect(violations(html), source).toEqual([]);
      expect(host.querySelector("kbd, details, summary, img, button"), source).toBeNull();
      expect(host.textContent, source).toContain(source.split("\n")[0]);
    }
  });

  it("rejects extra rich tags, classes, language names and attributes in the oracle", () => {
    for (const html of [
      '<kbd onclick="evil()">x</kbd>', '<em style="color:red">x</em>', '<del id="x">x</del>',
      '<details class="md-details" open><summary>x</summary></details>',
      '<details class="author"><summary>x</summary></details>', '<summary>x</summary>',
      '<blockquote class="md-alert md-alert-evil">x</blockquote>', '<p class="evil">x</p>',
      '<pre class="md-code"><code class="language-evil">x</code></pre>',
      '<span class="token keyword">x</span>', '<iframe src="https://a.test"></iframe>',
      '<pre class="md-code"><code class="language-javascript"><span class="token attacker">x</span></code></pre>',
      '<kbd><em>x</em></kbd>', '<details><summary>x</summary></details>',
      '<div class="mdtable" role="region" aria-label="Table" tabindex="1"><table></table></div>',
      '<a class="md-link" href="https://a.test" target="_blank" rel="noopener noreferrer nofollow" aria-label="fake">x</a>',
      '<ol start="1e9"><li>x</li></ol>', '<div style="text-align:left;background:url(x)">x</div>',
      '<div class="md-code-label" role="region" tabindex="0" aria-label="Table">x</div>',
    ]) expect(violations(html), html).not.toEqual([]);
  });

  it("accepts only real scoped footnote targets and rejects forged or duplicated fragments", () => {
    const html = markdownHtml('Read[^n] again[^n].\n\n[^n]: note', { taskLabelScope: 'a' });
    expect(violations(html)).toEqual([]);
    expect(html).toContain('id="md-fnref:61:1:2"');
    for (const forged of [
      html.replace('href="#md-fn:61:1"', 'href="#author"'),
      html.replace('href="#md-fn:61:1"', 'href="#md-fn:62:1"'),
      html.replace('id="md-fn:61:1"', 'id="md-fn:61:2"'),
      html.replace('aria-label="Footnote 1"', 'aria-label="Footnote 999"'),
      html.replace('class="md-footnote-backref"', 'class="md-link"'),
      html + '<li id="md-fn:61:1">duplicate</li>',
      '<a href="#md-fn:61:1">fake</a>',
      '<a class="md-footnote-backref" href="javascript:evil()" aria-label="Back to reference 1">↩</a>',
    ]) expect(violations(forged), forged).not.toEqual([]);
  });

  it("encodes hostile footnote scopes and keeps independent documents separate", () => {
    const source = 'Read[^safe].\n\n[^safe]: <img src=x onerror=alert(1)> [^missing]';
    const scopes = ['"><svg onload=alert(1)>', 'a', 'a-', '', '💡'];
    const hosts = scopes.map((taskLabelScope) => {
      const html = markdownHtml(source, { taskLabelScope });
      expect(violations(html)).toEqual([]);
      const host = document.createElement("div");
      host.innerHTML = html;
      expect(host.querySelector(".md-footnotes").textContent).toContain('<img src=x onerror=alert(1)> [^missing]');
      return host;
    });
    const ids = hosts.flatMap((host) => [...host.querySelectorAll("[id]")].map((element) => element.id));
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("preserves the refused-address boundaries while linking the allowed shape", () => {
    const refused = ["javascript:alert(1)", "data:text/html,x", "vbscript:evil()", "file:///etc/passwd", "ftp://a.test/x",
      "//a.test/x", "#42", "#/task/1", "src/a.js", "/a.js", "http:a.test", "https:/a.test", "https:///a.test", "https:\\\\a.test"];
    for (const address of refused) {
      const source = `[same](${address})`;
      const host = document.createElement("div");
      host.innerHTML = markdownHtml(source);
      expect(host.querySelector("a"), address).toBeNull();
      expect(host.textContent, address).toBe(source);
    }
    const html = markdownHtml('[same](https://a.test) [mail](mailto:a@b.test)');
    expect((html.match(/<a class="md-link"/g) || []).length).toBe(2);
    expect(violations(html)).toEqual([]);
  });

  it("normalizes web address controls without admitting hidden unsafe schemes", () => {
    const host = document.createElement("div");
    host.innerHTML = markdownHtml('[x](https://a.test/\u0000x)');
    expect(host.querySelector("a").getAttribute("href")).toBe("https://a.test/x");
    expect(markdownHtml('[x](java\u0000script:alert(1))')).not.toContain("<a ");
    expect(violations(host.innerHTML)).toEqual([]);
  });

  it("emits only allowlisted fence languages while preserving unknown labels as text", () => {
    for (const label of ["unknown", "constructor", "__proto__", 'js\"onmouseover=evil()', "javascript-extra"]) {
      const html = markdownHtml(`\`\`\`${label}\n<script>evil()</script>\n\`\`\``);
      const host = document.createElement("div");
      host.innerHTML = html;
      expect(host.querySelector("code").className, label).toBe("");
      expect(host.querySelector("code").textContent, label).toBe('<script>evil()</script>\n');
      expect(host.querySelector(".md-code-label").textContent, label).toBe(label);
      expect(violations(html), label).toEqual([]);
    }
    expect(markdownHtml('```javascript\nconst x = 1;\n```')).toContain('class="language-javascript"');
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

  it("bounds nested details and renders the remaining delimiters as text", () => {
    const source = ('<details>\n<summary>More</summary>\n').repeat(200) + 'body\n' + '</details>\n'.repeat(200);
    const started = performance.now();
    const html = markdownHtml(source);
    expect(performance.now() - started).toBeLessThan(1000);
    expect(violations(html)).toEqual([]);
    expect((html.match(/<details /g) || []).length).toBeLessThanOrEqual(8);
    expect(html).toContain('&lt;details&gt;');
  });

  it("bounds highlighting and reads long malformed rich syntax without rescanning", () => {
    const sources = [
      '<details>\n'.repeat(10000), '<kbd>'.repeat(15000), '[^'.repeat(50000),
      '*_~~'.repeat(25000), '![a]('.repeat(15000), '`'.repeat(100000),
      '```javascript\n' + 'const x = "<svg/onload=evil()>";\n'.repeat(5000) + '```',
    ];
    for (const source of sources) {
      const started = performance.now();
      const html = markdownHtml(source);
      expect([source.slice(0, 20), performance.now() - started < 1500]).toEqual([source.slice(0, 20), true]);
      expect(violations(html)).toEqual([]);
    }
  });

  it("leaves an unclosed fence inside a quote closed at the quote's end", () => {
    const html = markdownHtml("> ```\n> <b>code\nafter");
    expect(violations(html)).toEqual([]);
    expect(html).toContain("</code></pre></blockquote><p>after</p>");
  });
});
