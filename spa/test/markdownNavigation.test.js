/** @vitest-environment jsdom */
import { afterEach, describe, expect, it, vi } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { installMarkdownNavigation } from "../src/core/markdownNavigation.js";

let remove;
afterEach(() => { remove?.(); document.body.innerHTML = ""; });
const mount = () => {
  document.body.innerHTML = markdownHtml("Text[^a]\n\n[^a]: Note", { taskLabelScope: "nav" });
  window.history.replaceState(null, "", "#/projects/p/tasks/1");
  remove = installMarkdownNavigation(document);
};

describe("footnote navigation within a routed SPA", () => {
  it("scrolls and focuses a note and its backlink without changing the route", () => {
    mount();
    for (const selector of [".md-footnote-ref a", ".md-footnote-backref"]) {
      const anchor = document.querySelector(selector);
      const target = document.getElementById(anchor.getAttribute("href").slice(1));
      target.scrollIntoView = vi.fn();
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      anchor.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
      expect(location.hash).toBe("#/projects/p/tasks/1");
      expect(target.scrollIntoView).toHaveBeenCalledWith({ block: "nearest" });
      expect(document.activeElement).toBe(target);
    }
  });
  it("leaves ordinary links alone and prevents a missing note from navigating", () => {
    mount();
    const anchor = document.querySelector(".md-footnote-ref a");
    document.getElementById(anchor.hash.slice(1)).remove();
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    anchor.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    const normal = document.createElement("a");
    normal.href = "#/projects";
    document.body.append(normal);
    const normalEvent = new MouseEvent("click", { bubbles: true, cancelable: true });
    normal.dispatchEvent(normalEvent);
    expect(normalEvent.defaultPrevented).toBe(false);
  });
  it("returns cleanup and locates duplicate fallback IDs in the nearest rendered document", () => {
    const html = markdownHtml("Text[^a]\n\n[^a]: Note");
    document.body.innerHTML = `<article>${html}</article><article>${html}</article>`;
    remove = installMarkdownNavigation(document);
    const second = document.querySelectorAll("article")[1];
    const target = second.querySelector(".md-footnotes li");
    target.scrollIntoView = vi.fn();
    second.querySelector(".md-footnote-ref a").dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(target);
    remove();
    const event = new MouseEvent("click", { bubbles: true, cancelable: true });
    second.querySelector(".md-footnote-ref a").dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
  });
});
