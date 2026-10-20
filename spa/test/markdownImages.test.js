/** @vitest-environment jsdom */
import { afterEach, describe, expect, it } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { installMarkdownImages } from "../src/core/markdownImages.js";

let dispose;
afterEach(() => { dispose?.(); document.body.replaceChildren(); });

function mount(source = '![Diagram](https://images.test/a.png "A title")') {
  document.body.innerHTML = `<div class="markdown">${markdownHtml(source)}</div>`;
  dispose = installMarkdownImages(document);
  return document.querySelector(".markdown");
}

describe("external markdown image activation", () => {
  it("loads exactly one button instance with fixed safe image attributes", () => {
    const host = mount('![Diagram](https://images.test/a.png "A title") ![Diagram](https://images.test/a.png)');
    const [first, second] = host.querySelectorAll("button");
    expect(host.querySelectorAll("img")).toHaveLength(0);
    first.click();
    first.click();
    expect(host.querySelectorAll("img")).toHaveLength(1);
    expect(second.isConnected).toBe(true);
    const image = host.querySelector("img");
    expect(image.getAttribute("src")).toBe("https://images.test/a.png");
    expect(image.getAttribute("alt")).toBe("Diagram");
    expect(image.getAttribute("title")).toBe("A title");
    expect(image.getAttribute("loading")).toBe("lazy");
    expect(image.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(image.getAttributeNames().sort()).toEqual(["alt", "loading", "referrerpolicy", "src", "title"]);
  });

  it.each(["javascript:alert(1)", "data:image/svg+xml,x", "mailto:a@b.test", "//evil.test/x", "/x", "https://user:secret@evil.test/x", "https://@evil.test/x", "https://evil.test:bad/x"])("revalidates hostile latent sources: %s", (src) => {
    const host = mount();
    const button = host.querySelector("button");
    button.dataset.mdImageSrc = src;
    button.click();
    expect(host.querySelectorAll("img")).toHaveLength(0);
    expect(button.isConnected).toBe(true);
  });

  it("keeps hostile alt and title data as text attributes", () => {
    const host = mount();
    const button = host.querySelector("button");
    button.dataset.mdImageAlt = '<svg onload="alert(1)">';
    button.dataset.mdImageTitle = 'x" onerror="alert(1)';
    button.click();
    const image = host.querySelector("img");
    expect(image.getAttribute("alt")).toBe('<svg onload="alert(1)">');
    expect(image.getAttribute("title")).toBe('x" onerror="alert(1)');
    expect(image.hasAttribute("onerror")).toBe(false);
    expect(host.querySelectorAll("svg")).toHaveLength(0);
  });

  it("resets to an inert placeholder on a cached repaint", () => {
    const source = "![Diagram](https://images.test/a.png)";
    const host = mount(source);
    host.querySelector("button").click();
    expect(host.querySelectorAll("img")).toHaveLength(1);
    host.innerHTML = markdownHtml(source);
    expect(host.querySelectorAll("img")).toHaveLength(0);
    expect(host.querySelectorAll("button")).toHaveLength(1);
  });

  it("delegates new buttons and removes the listener on cleanup", () => {
    const host = mount();
    host.innerHTML = markdownHtml("![New](https://other.test/b.png)");
    host.querySelector("button").click();
    expect(host.querySelector("img").getAttribute("src")).toBe("https://other.test/b.png");
    host.innerHTML = markdownHtml("![Another](https://other.test/c.png)");
    dispose();
    host.querySelector("button").click();
    expect(host.querySelectorAll("img")).toHaveLength(0);
  });

  it("ignores matching controls outside markdown and leaves Build attachments alone", () => {
    mount();
    const outside = document.createElement("button");
    outside.className = "md-image-load";
    outside.dataset.mdImageSrc = "https://outside.test/a.png";
    document.body.append(outside);
    const attachment = document.createElement("img");
    attachment.className = "thread-attachment";
    attachment.setAttribute("src", "blob:build-attachment");
    document.body.append(attachment);
    outside.click();
    expect(outside.isConnected).toBe(true);
    expect(attachment.getAttribute("src")).toBe("blob:build-attachment");
    expect(document.querySelectorAll("img")).toHaveLength(1);
  });
});
