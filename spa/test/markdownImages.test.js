/** @vitest-environment jsdom */
import { afterEach, describe, expect, it, vi } from "vitest";
import { markdownHtml } from "../src/core/markdown.js";
import { installMarkdownImages } from "../src/core/markdownImages.js";
import { attachmentListHtml } from "../src/core/attachmentTiles.js";
import { createThreadState, wireThreadAttachments } from "../src/core/thread.js";
import { composerHtml, mountComposerAttachments } from "../src/core/composer.js";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { painted } from "./waits.js";

let dispose;
afterEach(() => { dispose?.(); document.body.replaceChildren(); });

function mount(source = '![Diagram](https://images.test/a.png "A title")', hostClass = "markdown") {
  document.body.innerHTML = `<div class="${hostClass}">${markdownHtml(source)}</div>`;
  dispose = installMarkdownImages(document);
  return document.querySelector(`.${hostClass}`);
}

describe("markdown image activation", () => {
  it.each(["markdown", "thread-body", "task-comment-body", "plan", "thread-event-detail", "cc-body"])("activates images in the actual %s renderer host", (hostClass) => {
    const host = mount(undefined, hostClass);
    expect(host.querySelectorAll("img")).toHaveLength(0);
    host.querySelector("button").click();
    expect(host.querySelector("img")?.getAttribute("src")).toBe("https://images.test/a.png");
  });

  it.each([false, true])("cancels a containing link before activation validation (tampered source: %s)", (tampered) => {
    const host = mount();
    const button = host.querySelector("button");
    const link = document.createElement("a");
    link.href = "https://target.test";
    button.replaceWith(link);
    link.append(button);
    const follow = vi.fn((event) => event.preventDefault());
    link.addEventListener("click", follow);
    if (tampered) button.dataset.mdImageSrc = "javascript:alert(1)";
    const activation = new MouseEvent("click", { bubbles: true, cancelable: true });
    button.dispatchEvent(activation);
    expect(activation.defaultPrevented).toBe(true);
    expect(follow).not.toHaveBeenCalled();
    expect(host.querySelectorAll("img")).toHaveLength(tampered ? 0 : 1);
    link.click();
    expect(follow).toHaveBeenCalledTimes(1);
    expect(host.querySelectorAll("img")).toHaveLength(tampered ? 0 : 1);
  });

  it("keeps linked-image activation per instance and resets both links on repaint", () => {
    const source = "[![badge](https://images.test/a.png)](https://target.test)";
    const host = mount(`${source} ${source}`);
    expect(host.querySelectorAll("a.md-link > button")).toHaveLength(2);
    host.querySelector("button").click();
    expect(host.querySelectorAll("a.md-link > img")).toHaveLength(1);
    expect(host.querySelectorAll("a.md-link > button")).toHaveLength(1);
    host.innerHTML = markdownHtml(`${source} ${source}`);
    expect(host.querySelectorAll("img")).toHaveLength(0);
    expect(host.querySelectorAll("a.md-link > button")).toHaveLength(2);
  });

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

  it.each(["javascript:alert(1)", "data:image/svg+xml,x", "blob:https://images.test/id", "mailto:a@b.test", "//evil.test/x", "/x", "https://user:secret@evil.test/x", "https://@evil.test/x", "https://evil.test:bad/x"])("revalidates hostile latent sources: %s", (src) => {
    const host = mount();
    const button = host.querySelector("button");
    button.dataset.mdImageSrc = src;
    button.click();
    expect(host.querySelectorAll("img")).toHaveLength(0);
    expect(button.isConnected).toBe(true);
  });

  it.each(["http://dev.test/image.png", new URL("/app/downloads/SHA256SUMS", location.href).href])("requires activation and permits a validated development HTTP source: %s", (src) => {
    const host = mount(`![Local](${src})`);
    expect(host.querySelectorAll("img, [src]")).toHaveLength(0);
    host.querySelector("button").click();
    expect(host.querySelector("img")?.getAttribute("src")).toBe(src);
    expect(host.querySelector("img")?.getAttribute("referrerpolicy")).toBe("no-referrer");
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


describe("trusted attachment images alongside Markdown consent", () => {
  it("loads attachments through the task byte cache into direct Blob previews without activating Markdown", async () => {
    vi.resetModules();
    vi.stubGlobal("indexedDB", new IDBFactory());
    vi.stubGlobal("IDBKeyRange", IDBKeyRange);
    const { createTaskAttachmentBodies } = await import("../src/core/taskAttachmentBodies.js");
    const call = vi.fn(async () => ({ mime: "image/png", size: 3, content_b64: btoa("png") }));
    const options = { deviceId: "image-consent-device", taskId: "image-consent-task" };
    const descriptor = { path: "/attachments/trusted.png", name: "trusted.png", mime: "image/png", size: 3 };
    const host = mount("![Redirectable](https://build.test/app/downloads/SHA256SUMS)");
    const markdown = host.innerHTML;
    const paint = () => { host.innerHTML = markdown + attachmentListHtml([descriptor]); };
    const attach = async (loader) => {
      const state = createThreadState();
      try {
        wireThreadAttachments(host, (path) => loader.load(path), state);
        await painted(() => host.querySelector("img.thread-attachment-image")?.getAttribute("src")?.startsWith("blob:"), host);
        expect(host.querySelectorAll(".md-image-load")).toHaveLength(1);
        expect(host.querySelectorAll("img")).toHaveLength(1);
      } finally {
        state.dispose();
        loader.dispose();
      }
    };
    try {
      paint();
      await attach(createTaskAttachmentBodies({ ...options, call }));
      expect(call).toHaveBeenCalledTimes(1);
      const offline = vi.fn(async () => { throw new Error("attachment transport must not be needed on revisit"); });
      paint();
      await attach(createTaskAttachmentBodies({ ...options, call: offline }));
      expect(offline).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("keeps a selected local file's production composer Data URL preview direct", async () => {
    const host = mount();
    const composer = document.createElement("div");
    composer.innerHTML = composerHtml({ inputId: "image-input", sendId: "image-send", hintId: "image-hint", attachable: true });
    host.append(composer);
    const upload = vi.fn(async (file) => ({ path: "/attachments/local.png", name: file.name, mime: file.type, size: file.size }));
    const controller = mountComposerAttachments(composer, { ids: { input: "image-input", send: "image-send", hint: "image-hint" }, upload });
    controller.addFiles([new File(["png"], "local.png", { type: "image/png" })]);
    await painted(() => !controller.busy() && composer.querySelector("img.composer-chip-thumb"), composer);
    const thumbnail = composer.querySelector("img.composer-chip-thumb");
    expect(thumbnail?.getAttribute("src")).toBe(`data:image/png;base64,${btoa("png")}`);
    expect(upload).toHaveBeenCalledTimes(1);
    expect(host.querySelectorAll("button.md-image-load")).toHaveLength(1);
    expect(composer.querySelector("button.md-image-load")).toBeNull();
    controller.clear();
  });
});
