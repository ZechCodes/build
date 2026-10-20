/** @vitest-environment jsdom */
import { describe, expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
const render = (source, options) => {
  const host = document.createElement("div");
  host.innerHTML = markdownHtml(source, options);
  return host;
};

describe("GitHub alerts", () => {
  it.each(["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"])("renders %s and nested markdown", (kind) => {
    const host = render(`> [!${kind}]\n> First **words**.\n>\n> - [x] done`);
    const alert = host.querySelector(`.md-alert-${kind.toLowerCase()}`);
    expect(alert).not.toBeNull();
    expect(alert.querySelector(".md-alert-title").textContent).toBe(kind[0] + kind.slice(1).toLowerCase());
    expect(alert.querySelector("strong").textContent).toBe("words");
    expect(alert.querySelector("input").checked).toBe(true);
  });
  it("requires a supported marker alone at the beginning", () => {
    for (const source of ["> [!DANGER]\n> text", "> [!NOTE] extra\n> text", "> text\n> [!NOTE]"])
      expect(render(source).querySelector(".md-alert")).toBeNull();
  });
  it("preserves nested ordered lists and original task source offsets", () => {
    const source = "> [!TIP]\r\n> 3. [ ] check\r\n>    - [x] child";
    const taskMarkers = [];
    const host = render(source, { taskMarkers });
    expect(host.querySelector("ol").start).toBe(3);
    expect(taskMarkers.map((offset) => source[offset])).toEqual([" ", "x"]);
  });
});

describe("fences", () => {
  it("accepts tildes, labels and language aliases with safe Prism highlighting", () => {
    const host = render("~~~js\nconst x = '<img onerror=evil()>';\n~~~");
    expect(host.querySelector(".md-code-label").textContent).toBe("js");
    expect(host.querySelector("code.language-javascript .token.keyword").textContent).toBe("const");
    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector("code").textContent).toBe("const x = '<img onerror=evil()>';\n");
  });
  it("requires the same closing character, sufficient length, and no trailing text", () => {
    const host = render("````text\none\n```\n~~~\n````suffix\n`````\nafter");
    expect(host.querySelector("code").textContent).toBe("one\n```\n~~~\n````suffix\n");
    expect(host.querySelector("p").textContent).toBe("after");
  });
  it("allows three spaces of indent and keeps unknown languages escaped", () => {
    const host = render('   ~~~unknown\n<script>bad()</script>\n   ~~~');
    expect(host.querySelector(".md-code-label").textContent).toBe("unknown");
    expect(host.querySelector("code .token, script")).toBeNull();
  });
  it("rejects backticks inside backtick info strings", () => {
    expect(render("```bad`language\ntext").querySelector("pre")).toBeNull();
  });
});

describe("restricted attribute-free details", () => {
  it("renders native details with inline summary and markdown body", () => {
    const host = render("<details>\n<summary>More **information**</summary>\n\n- item\n\n</details>\nafter");
    const details = host.querySelector("details.md-details");
    expect(details).not.toBeNull();
    expect(details.firstElementChild.tagName).toBe("SUMMARY");
    expect(details.querySelector("summary strong").textContent).toBe("information");
    expect(details.querySelector("li").textContent).toBe("item");
    expect(host.lastElementChild.textContent).toBe("after");
  });
  it("escapes attributes, malformed summaries and unclosed details", () => {
    for (const source of ['<details open>\n<summary>Title</summary>\nbody\n</details>', '<details>\n<summary onclick="evil()">Title</summary>\nbody\n</details>', '<details>\n<summary>Title</summary>\nbody', '<details>\nbody\n</details>'])
      expect(render(source).querySelector("details, summary")).toBeNull();
  });
  it("supports bounded nesting and ignores closers inside fences", () => {
    const host = render("<details>\n<summary>Outer</summary>\n<details>\n<summary>Inner</summary>\n```\n</details>\n```\n</details>\n</details>");
    expect(host.querySelectorAll("details")).toHaveLength(2);
    expect(host.querySelector("code").textContent).toBe("</details>\n");
    const deep = `${"<details>\n<summary>Title</summary>\n".repeat(1000)}end\n${"</details>\n".repeat(1000)}`;
    expect(render(deep).querySelectorAll("details").length).toBeLessThanOrEqual(8);
  });
});

describe("footnotes", () => {
  it("places all return links inside the definition's last paragraph", () => {
    const host = render("Text[^a], again[^a].\n\n[^a]: First paragraph.\n\n    Final **paragraph**.");
    const paragraphs = host.querySelectorAll(".md-footnotes > ol > li > p");
    expect(paragraphs).toHaveLength(2);
    expect(paragraphs[0].querySelector("a")).toBeNull();
    expect(paragraphs[1].querySelectorAll(".md-footnote-backref")).toHaveLength(2);
    expect(paragraphs[1].textContent).toBe("Final paragraph. ↩ ↩");
  });
  it("keeps a return link for definitions that contain no paragraph", () => {
    const host = render("Text[^a].\n\n[^a]: ```text\n    code\n    ```");
    expect(host.querySelector(".md-footnotes code").textContent).toBe("code\n");
    expect(host.querySelectorAll(".md-footnotes .md-footnote-backref")).toHaveLength(1);
  });
  it("orders notes by reference and gives every reference a local accessible backlink", () => {
    const host = render("Second[^b], first[^a], again[^b].\n\n[^a]: First **note**\n[^b]: Other note\n\n    - nested item", { taskLabelScope: "comment:1" });
    expect([...host.querySelectorAll(".md-footnote-ref a")].map((ref) => ref.textContent)).toEqual(["1", "2", "1"]);
    const notes = [...host.querySelectorAll(".md-footnotes > ol > li")];
    expect(notes).toHaveLength(2);
    expect(notes[0].textContent).toContain("Other note");
    expect(notes[0].querySelector("li").textContent).toBe("nested item");
    expect(notes[1].querySelector("strong").textContent).toBe("note");
    expect(host.querySelectorAll(".md-footnote-backref")).toHaveLength(3);
    for (const link of host.querySelectorAll(".md-footnote-ref a, .md-footnote-backref")) {
      expect([...host.querySelectorAll("[id]")].some((el) => `#${el.id}` === link.getAttribute("href"))).toBe(true);
      expect(link.getAttribute("aria-label")).toMatch(/Footnote|Back to reference/);
      expect(link.getAttribute("href")).toMatch(/^#md-fn(?:ref)?:[\da-f:-]+$/);
    }
  });
  it("leaves missing, escaped, code and link-label references literal", () => {
    const host = render("Missing[^none] `[^a]` [label[^a]](https://a.test) \\[^a]\n\n```\n[^hidden]: hidden\n```\n\n[^a]: Real note");
    expect(host.querySelectorAll(".md-footnote-ref")).toHaveLength(0);
    expect(host.querySelector(".md-footnotes")).toBeNull();
    expect(host.textContent).toContain("Missing[^none]");
  });
  it("scopes IDs, escapes content, and preserves checklist offsets in definitions", () => {
    const source = 'Text[^safe]\n\n[^safe]: <img src=x onerror=evil()>\n\n    - [ ] check';
    const options = { taskLabelScope: '\"><svg onload=evil()>' };
    const taskMarkers = [];
    const first = render(source, { ...options, taskMarkers });
    expect(first.querySelector(".md-footnotes li").id).toBe(render(source, options).querySelector(".md-footnotes li").id);
    expect(first.querySelector("img, svg")).toBeNull();
    expect(taskMarkers.map((offset) => source[offset])).toEqual([" "]);
    expect(render(source, { taskLabelScope: "different" }).querySelector(".md-footnotes li").id).not.toBe(first.querySelector(".md-footnotes li").id);
  });
  it("keeps self-references in note bodies literal", () => {
    const host = render("Text[^a]\n\n[^a]: Again[^a]");
    expect(host.querySelectorAll(".md-footnotes")).toHaveLength(1);
    expect(host.querySelectorAll(".md-footnote-ref")).toHaveLength(1);
    expect(host.textContent).toContain("Again[^a]");
  });
});

describe("block rendering regressions", () => {
  it("does not create invisible references while deriving checklist accessible names", () => {
    const host = render("- [ ] Item[^a]\n\n[^a]: Note");
    expect(host.querySelectorAll(".md-footnote-ref")).toHaveLength(1);
    expect(host.querySelectorAll(".md-footnote-backref")).toHaveLength(1);
  });
  it("orders summary footnotes before footnotes in the details body", () => {
    const host = render("<details>\n<summary>Title[^a]</summary>\nBody[^b]\n</details>\n\n[^a]: First\n[^b]: Second");
    expect([...host.querySelectorAll(".md-footnote-ref a")].map((ref) => ref.textContent)).toEqual(["1", "2"]);
  });
  it("stays bounded on long invalid delimiters and many definitions", () => {
    for (const source of ["<details>\n<summary>Title</summary>\n".repeat(5000), "[^unused]: words\n".repeat(5000), `~~~markdown\n${"[".repeat(100000)}\n~~~`]) {
      const started = performance.now();
      markdownHtml(source);
      expect(performance.now() - started).toBeLessThan(1000);
    }
  });
});

describe("fence grammar allowlist", () => {
  it("keeps Object prototype names in the plain-text language fallback", () => {
    for (const label of ["constructor", "__proto__"]) {
      const host = render(`~~~${label}\nconst value = 1\n~~~`);
      expect(host.querySelector("code").hasAttribute("class")).toBe(false);
      expect(host.querySelector("code").textContent).toBe("const value = 1\n");
    }
  });
});

describe("definition collection respects block boundaries", () => {
  it("ends an unclosed list-contained fence at the list's end", () => {
    const host = render("- item\n  ```\n  code\n\nText[^a]\n\n[^a]: Actual note");
    expect(host.querySelector(".md-footnote-ref")).not.toBeNull();
    expect(host.querySelector(".md-footnotes").textContent).toContain("Actual note");
  });
  it("does not collect definition-shaped source inside a list's fence", () => {
    const host = render("- ```\n  [^fake]: source\n  ```\n\nText[^fake]");
    expect(host.querySelector(".md-footnotes")).toBeNull();
    expect(host.querySelector("code").textContent).toContain("[^fake]: source");
  });
});

describe("details matching respects list block boundaries", () => {
  it("ends a list-contained unclosed fence at the list dedent", () => {
    const host = render("<details>\n<summary>Title</summary>\n- item\n  ```\n  code\n</details>");
    expect(host.querySelector("details")).not.toBeNull();
    expect(host.querySelector("summary").textContent).toBe("Title");
    expect(host.querySelector("details code").textContent).toBe("code\n");
  });
  it("keeps list-contained details delimiters inside the list scope", () => {
    const host = render("<details>\n<summary>Title</summary>\n- ```\n  </details>\n  ```\n</details>\nafter");
    expect(host.querySelector("details code").textContent).toBe("</details>\n");
    expect(host.lastElementChild.textContent).toBe("after");
  });
});
