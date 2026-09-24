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
const NOT_TEXT = /\btype="(checkbox|radio|file|hidden|button|submit|range|color)"/;
const CONTACT_WORDS = /name|mail|phone|tel|addr|street|city|zip|postal|country|company|org|first|last|given|family|title|contact/i;

const lineOf = (source, index) => source.slice(0, index).split("\n").length;

function fieldTags() {
  return srcJsFiles().flatMap((file) => {
    const source = srcSourceOf(file);
    return [...source.matchAll(FIELD_TAG)]
      .filter((match) => !NOT_TEXT.test(match[0]))
      .map((match) => ({ where: `${file}:${lineOf(source, match.index)}`, tag: match[0] }));
  });
}

const declares = (tag, attribute) => tag.includes("${fieldTraits(") || new RegExp(`\\s${attribute}="[^"]+"`).test(tag);

/** The literal part of a field's id and name: `${…}` pieces are filled at run
 *  time from callers that are checked where they render. */
const namingOf = (tag) =>
  [...tag.matchAll(/\s(id|name)="([^"]*)"/g)].map(([, , value]) => value.replace(/\$\{[^}]*\}/g, ""));

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
  const tags = fieldTags();

  it("finds the fields it guards", () => {
    expect(tags.length).toBeGreaterThan(35);
  });

  it.each(["autocomplete", "autocapitalize"])("declares %s explicitly", (attribute) => {
    expect(tags.filter(({ tag }) => !declares(tag, attribute)).map(({ where }) => where)).toEqual([]);
  });

  it("carries no id or name that reads like a contact or address field", () => {
    const named = tags.filter(({ tag }) => namingOf(tag).some((value) => CONTACT_WORDS.test(value)));
    expect(named.map(({ where, tag }) => `${where} ${namingOf(tag).join(" ")}`)).toEqual([]);
  });

  it("gives every field built with createElement its traits", () => {
    const built = srcJsFiles().filter((file) => /createElement\(\s*["'](input|textarea)["']/.test(srcSourceOf(file)));
    expect(built.length).toBeGreaterThan(0);
    expect(built.filter((file) => !srcSourceOf(file).includes("applyFieldTraits("))).toEqual([]);
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
