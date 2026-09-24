// @vitest-environment jsdom
//
// Every text field the SPA draws says what it is (#139). iOS Safari guesses a
// field's purpose from its naming when nothing tells it otherwise, and on iOS 27
// it offered "AutoFill Contact" and addresses on the agent compose box. The
// guard below walks every <input> and <textarea> under src/ so a new field
// cannot ship without its traits, or with an id that reads like a contact field.

import { describe, it, expect } from "vitest";

import { applyFieldTraits, fieldTraits } from "../src/core/fieldTraits.js";
import { composerHtml } from "../src/core/composer.js";
import { composeBoxHtml } from "../src/core/compose.js";
import { srcJsFiles, srcSourceOf } from "./treeFiles.js";

// An interpolation holds no `>` or `}` of its own in these templates, so a tag
// runs to the first `>` outside one.
const FIELD_TAG = /<(input|textarea)\b(?:\$\{[^}]*\}|[^>])*>/g;
// A field built in code: `const field = page.createElement("textarea")`, or one
// returned or passed on without a name, which nothing can give traits to.
const CREATED_FIELD = /(?:(?:const|let|var)\s+(\w+)\s*=\s*)?[\w.]*createElement\(\s*["'](?:input|textarea)["']\s*\)/g;
// The controls no keyboard types into. Read-only text fields are not among
// them: they are still text fields, and one attribute keeps them ready for
// the day they become editable.
const NON_TEXT_TYPES = ["checkbox", "radio", "file", "hidden", "button", "submit", "range", "color"];
const CONTACT_WORDS = /name|mail|phone|tel|addr|street|city|zip|postal|country|company|org|first|last|given|family|title|contact/i;

const lineOf = (source, index) => source.slice(0, index).split("\n").length;
const isTextTag = (tag) => !NON_TEXT_TYPES.includes(tag.match(/\stype="([^"]*)"/)?.[1]);

/** The text fields a source draws in templates, with the line each starts on. */
const fieldTagsIn = (source) =>
  [...source.matchAll(FIELD_TAG)]
    .filter((match) => isTextTag(match[0]))
    .map((match) => ({ line: lineOf(source, match.index), tag: match[0] }));

/** Whether a tag sets `attribute` to a non-empty value, either through
 *  fieldTraits or written out. A match must follow whitespace, so
 *  `aria-autocomplete` does not count as `autocomplete`. */
function declares(tag, attribute) {
  if (tag.includes("${fieldTraits(")) return true;
  const opener = `${attribute}="`;
  for (let at = tag.indexOf(opener); at !== -1; at = tag.indexOf(opener, at + 1)) {
    if (/\s/.test(tag[at - 1]) && tag[at + opener.length] !== '"') return true;
  }
  return false;
}

/** The code that follows a creation up to the end of its top-level statement:
 *  the first line that starts at the left margin. */
function scopeAfter(source, match) {
  const rest = source.slice(match.index + match[0].length);
  const end = rest.search(/\n(?=\S)/);
  return end === -1 ? rest : rest.slice(0, end);
}

const madeNonText = (scope, name) =>
  NON_TEXT_TYPES.some((type) => scope.includes(`${name}.type = "${type}"`) || scope.includes(`${name}.setAttribute("type", "${type}")`));

/** The lines where a source builds a text field in code without handing that
 *  same field to applyFieldTraits. */
function untraitedCreationsIn(source) {
  return [...source.matchAll(CREATED_FIELD)]
    .filter((match) => {
      const name = match[1];
      if (!name) return true;
      const scope = scopeAfter(source, match);
      return !madeNonText(scope, name) && !scope.includes(`applyFieldTraits(${name},`);
    })
    .map((match) => lineOf(source, match.index));
}

/** Every src/ file's findings from one of the scans above, as `file:line`. */
const acrossSrc = (scan) => srcJsFiles().flatMap((file) => scan(srcSourceOf(file)).map((line) => `${file}:${line}`));

/** The literal part of a field's id and name: `${…}` pieces are filled at run
 *  time from callers that are checked where they render. */
const namingOf = (tag) =>
  [...tag.matchAll(/\s(id|name)="([^"]*)"/g)].map(([, , value]) => value.replace(/\$\{[^}]*\}/g, ""));

const missing = (attribute) => (source) => fieldTagsIn(source).filter(({ tag }) => !declares(tag, attribute)).map(({ line }) => line);
const contactNamed = (source) =>
  fieldTagsIn(source).filter(({ tag }) => namingOf(tag).some((value) => CONTACT_WORDS.test(value))).map(({ line }) => line);

describe("fieldTraits", () => {
  it("writes each kind as attributes, with the Enter key's label replaceable", () => {
    expect(fieldTraits("prose")).toBe(
      'autocomplete="off" autocorrect="on" autocapitalize="sentences" spellcheck="true" inputmode="text" enterkeyhint="enter"',
    );
    expect(fieldTraits("identifier", "go")).toContain('autocapitalize="off" spellcheck="false" inputmode="text" enterkeyhint="go"');
    expect(fieldTraits("search")).toContain('inputmode="search" enterkeyhint="search"');
    expect(fieldTraits("code")).toContain('autocapitalize="characters"');
  });

  it("refuses a kind it does not know rather than drawing a bare field", () => {
    expect(() => fieldTraits("contact")).toThrow(/contact/);
  });

  it("sets the same attributes on a field built in code", () => {
    const field = document.createElement("textarea");
    applyFieldTraits(field, "identifier", "enter");
    const probe = document.createElement("div");
    probe.innerHTML = `<textarea ${fieldTraits("identifier", "enter")}></textarea>`;
    const expected = [...probe.firstElementChild.attributes].map(({ name, value }) => [name, value]);
    expect([...field.attributes].map(({ name, value }) => [name, value])).toEqual(expected);
  });
});

describe("every text field under src/", () => {
  it("finds the fields it guards", () => {
    expect(acrossSrc((source) => fieldTagsIn(source).map(({ line }) => line)).length).toBeGreaterThan(35);
    expect(acrossSrc((source) => [...source.matchAll(CREATED_FIELD)].map((match) => lineOf(source, match.index))).length).toBeGreaterThan(1);
  });

  it.each(["autocomplete", "autocapitalize"])("declares %s explicitly", (attribute) => {
    expect(acrossSrc(missing(attribute))).toEqual([]);
  });

  it("carries no id or name that reads like a contact or address field", () => {
    expect(acrossSrc(contactNamed)).toEqual([]);
  });

  it("gives every field built with createElement its traits", () => {
    expect(acrossSrc(untraitedCreationsIn)).toEqual([]);
  });
});

describe("the guard itself", () => {
  it("fails a bare template field on both attributes, and skips controls nobody types into", () => {
    const source = '<input type="text">\n<input type="file" hidden>\n<input type="checkbox">';
    expect(missing("autocomplete")(source)).toEqual([1]);
    expect(missing("autocapitalize")(source)).toEqual([1]);
  });

  it("holds a read-only text field to the same rule", () => {
    expect(missing("autocomplete")("<textarea readonly></textarea>")).toEqual([1]);
    expect(missing("autocomplete")('<textarea readonly autocomplete="off"></textarea>')).toEqual([]);
  });

  it("counts neither aria-autocomplete nor an empty value", () => {
    expect(missing("autocomplete")('<input aria-autocomplete="list" autocapitalize="off">')).toEqual([1]);
    expect(missing("autocomplete")('<input autocomplete="" autocapitalize="off">')).toEqual([1]);
    expect(missing("autocomplete")('<input autocomplete="off" autocapitalize="off">')).toEqual([]);
  });

  it("checks each created field, not just the file", () => {
    const source = [
      "function editor(page) {",
      '  const field = page.createElement("textarea");',
      '  applyFieldTraits(field, "identifier");',
      "}",
      "export function probe() { return document.createElement(\"textarea\"); }",
      "function other() {",
      '  const field = document.createElement("input");',
      "  return field;",
      "}",
    ].join("\n");
    expect(untraitedCreationsIn(source)).toEqual([5, 7]);
  });

  it("skips a created input made into a control nobody types into", () => {
    const source = ["function picker() {", '  const choose = document.createElement("input");', '  choose.type = "file";', "}"].join("\n");
    expect(untraitedCreationsIn(source)).toEqual([]);
  });
});

describe("the compose boxes", () => {
  const traitsOf = (html, selector) => {
    const host = document.createElement("div");
    host.innerHTML = html;
    const field = host.querySelector(selector);
    return Object.fromEntries(
      ["autocomplete", "autocorrect", "autocapitalize", "spellcheck", "inputmode", "enterkeyhint"].map((name) => [name, field.getAttribute(name)]),
    );
  };
  const PROSE = { autocomplete: "off", autocorrect: "on", autocapitalize: "sentences", spellcheck: "true", inputmode: "text", enterkeyhint: "enter" };

  it("draws the agent composer (the box in the #139 screenshot) as prose", () => {
    const html = composerHtml({ inputId: "railinput", sendId: "railsend", hintId: "railhint", placeholder: "Message the agent" });
    expect(traitsOf(html, "#railinput")).toEqual(PROSE);
  });

  it("draws the capture box as prose", () => {
    expect(traitsOf(composeBoxHtml({ placeholder: "What should happen?" }), "#compose-text")).toEqual(PROSE);
  });
});
