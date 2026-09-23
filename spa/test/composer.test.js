// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  ATTACHMENT_MAX_BYTES,
  LARGE_PASTE_CHARS,
  composerHtml,
  formatAttachmentSize,
  mountComposerAttachments,
  pasteIntent,
} from "../src/core/composer.js";
import { createChatRepository } from "../src/core/chatRepository.js";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readCached } from "../src/core/localCache.js";

const IDS = { input: "ti", send: "ts", hint: "th" };
const MARKUP_IDS = { inputId: "ti", sendId: "ts", hintId: "th", placeholder: "Say something…" };

const mount = (options = {}) => {
  document.body.innerHTML = `<div id="host">${composerHtml({ ...MARKUP_IDS, attachable: true })}</div>`;
  const host = document.querySelector("#host");
  const uploads = [];
  const controller = mountComposerAttachments(host, {
    ids: IDS,
    upload: (file, contentBase64) => {
      uploads.push({ file, contentBase64 });
      return Promise.resolve({
        name: file.name,
        path: `.build/attachments/abc-${file.name}`,
        mime: file.type || "application/octet-stream",
        size: file.size,
      });
    },
    ...options,
  });
  return { host, controller, uploads };
};

const pasteOf = (data) => {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  event.clipboardData = { files: [], items: [], getData: () => "", ...data };
  return event;
};

const dropOf = (files) => {
  const event = new Event("drop", { bubbles: true, cancelable: true });
  event.dataTransfer = { files, items: [], types: ["Files"] };
  return event;
};

/** Resolve every microtask the FileReader + upload chain queues. */
const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

describe("what a paste means", () => {
  it("hands a short paste back to the browser untouched", () => {
    expect(pasteIntent({ files: [], items: [], getData: () => "one line" })).toEqual({
      files: [],
      asFile: null,
    });
  });

  it("turns a wall of text into a file, because a composer is not a document", () => {
    const wall = "x".repeat(LARGE_PASTE_CHARS + 1);
    expect(pasteIntent({ files: [], items: [], getData: () => wall })).toEqual({
      files: [],
      asFile: wall,
    });
  });

  it("takes the files when the clipboard holds any, whatever text rides along", () => {
    const png = new File(["png"], "screenshot.png", { type: "image/png" });
    const intent = pasteIntent({ files: [png], items: [], getData: () => "screenshot.png" });
    expect(intent.files).toEqual([png]);
    expect(intent.asFile).toBe(null);
  });

  // Chromium on Linux offers a pasted screenshot as a file AND as an item, and
  // `getAsFile()` mints a new File object each call — so the same image came
  // through twice, and the tray showed two chips for one paste.
  it("counts an image the clipboard offers as both a file and an item once", () => {
    const stamp = 1700000000000;
    const asFile = new File(["png"], "image.png", { type: "image/png", lastModified: stamp });
    const asItem = new File(["png"], "image.png", { type: "image/png", lastModified: stamp });
    const intent = pasteIntent({
      files: [asFile],
      items: [{ kind: "file", type: "image/png", getAsFile: () => asItem }],
      getData: () => "",
    });
    expect(intent.files).toEqual([asFile]);
  });

  it("keeps two different files pasted together", () => {
    const one = new File(["a"], "a.png", { type: "image/png" });
    const two = new File(["bb"], "b.png", { type: "image/png" });
    const intent = pasteIntent({
      files: [one],
      items: [{ kind: "file", type: "image/png", getAsFile: () => two }],
      getData: () => "",
    });
    expect(intent.files).toEqual([one, two]);
  });

  it("finds an image pasted as a clipboard item rather than a file", () => {
    const png = new File(["png"], "image.png", { type: "image/png" });
    const intent = pasteIntent({
      files: [],
      items: [{ kind: "file", type: "image/png", getAsFile: () => png }],
      getData: () => "",
    });
    expect(intent.files).toEqual([png]);
  });
});

describe("attaching", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("keeps a huge paste out of the textarea and sends it as a file", async () => {
    const { host, controller, uploads } = mount();
    const input = host.querySelector("#ti");
    const wall = "y".repeat(LARGE_PASTE_CHARS + 40);

    const event = pasteOf({ getData: () => wall });
    input.dispatchEvent(event);
    await settle();

    expect(event.defaultPrevented).toBe(true);
    expect(input.value).toBe("");
    expect(uploads).toHaveLength(1);
    expect(uploads[0].file.name).toBe("pasted-text-1.txt");
    expect(atob(uploads[0].contentBase64)).toBe(wall);
    expect(controller.attachments()).toHaveLength(1);
    expect(host.querySelector(".composer-chip-name").textContent).toBe("pasted-text-1.txt");
  });

  it("attaches a dropped file and clears the drop state", async () => {
    const { host, controller } = mount();
    const png = new File(["binary"], "diagram.png", { type: "image/png" });

    host.dispatchEvent(new Event("dragover", { bubbles: true, cancelable: true }));
    expect(host.querySelector(".composer").classList.contains("is-dropping")).toBe(true);

    host.dispatchEvent(dropOf([png]));
    await settle();

    expect(host.querySelector(".composer").classList.contains("is-dropping")).toBe(false);
    expect(controller.attachments().map((a) => a.name)).toEqual(["diagram.png"]);
  });

  it("refuses a file past the limit before spending time reading it", async () => {
    const errors = [];
    const { host, controller, uploads } = mount({ onError: (message) => errors.push(message) });
    const huge = new File(["x"], "huge.bin");
    Object.defineProperty(huge, "size", { value: ATTACHMENT_MAX_BYTES + 1 });

    host.dispatchEvent(dropOf([huge]));
    await settle();

    expect(uploads).toHaveLength(0);
    expect(controller.attachments()).toEqual([]);
    expect(errors.join(" ")).toContain("huge.bin");
  });

  it("drops an attachment the reviewer removed", async () => {
    const { host, controller } = mount();
    host.dispatchEvent(dropOf([new File(["a"], "a.txt"), new File(["b"], "b.txt")]));
    await settle();

    host.querySelectorAll(".composer-chip-remove")[0].click();
    expect(controller.attachments().map((a) => a.name)).toEqual(["b.txt"]);
    expect(host.querySelectorAll(".composer-chip")).toHaveLength(1);
  });

  it("marks a failed upload on its own chip and keeps the others sendable", async () => {
    const { host, controller } = mount({
      upload: (file) =>
        file.name === "bad.txt"
          ? Promise.reject(new Error("relay down"))
          : Promise.resolve({ name: file.name, path: `.build/attachments/x-${file.name}`, mime: "text/plain", size: 1 }),
    });

    host.dispatchEvent(dropOf([new File(["a"], "bad.txt"), new File(["b"], "good.txt")]));
    await settle();

    expect(controller.attachments().map((a) => a.name)).toEqual(["good.txt"]);
    expect(host.querySelector(".composer-chip.failed .composer-chip-name").textContent).toBe("bad.txt");
    expect(controller.busy()).toBe(false);
  });

  it("is busy until every upload has landed", async () => {
    let release;
    const { host, controller } = mount({
      upload: (file) =>
        new Promise((resolve) => {
          release = () => resolve({ name: file.name, path: "p", mime: "text/plain", size: 1 });
        }),
    });

    host.dispatchEvent(dropOf([new File(["a"], "slow.txt")]));
    await settle();
    expect(controller.busy()).toBe(true);
    expect(host.querySelector(".composer-chip.uploading")).toBeTruthy();

    release();
    await settle();
    expect(controller.busy()).toBe(false);
  });

  it("survives the thread repainting underneath it", async () => {
    // The surfaces rebuild the whole timeline on their poll, so a tray that
    // lived only in the DOM would drop the reviewer's files mid-sentence.
    let pending = [];
    const options = {
      readAttachments: () => pending,
      writeAttachments: (next) => {
        pending = next;
      },
    };
    const first = mount(options);
    first.host.dispatchEvent(dropOf([new File(["a"], "kept.png", { type: "image/png" })]));
    await settle();
    expect(first.controller.attachments()).toHaveLength(1);

    const second = mount(options);
    expect(second.controller.attachments().map((a) => a.name)).toEqual(["kept.png"]);
    expect(second.host.querySelectorAll(".composer-chip")).toHaveLength(1);
  });

  it("persists an upload that completes after the composer remounts", async () => {
    let release;
    const repository = createChatRepository({ scope: { key: "device-a" }, call: vi.fn() });
    const chat = repository.controller({ entityId: "run-1", agentId: "agent-1", conversationId: "thread-1" });
    const upload = (file) => new Promise((resolve) => {
      release = () => resolve({ name: file.name, path: "stored/slow.txt", mime: "text/plain", size: 1 });
    });
    const first = mount({ ...chat.bindDraft(), upload });
    first.host.dispatchEvent(dropOf([new File(["a"], "slow.txt")]));
    await settle();
    expect(chat.readAttachments()[0].status).toBe("uploading");

    mount({ ...chat.bindDraft(), upload });
    release();
    await settle();

    expect(chat.readAttachments()[0].status).toBe("ready");
    expect(chat.readAttachments()[0].descriptor.path).toBe("stored/slow.txt");
  });

  it("keeps an active upload through its own cache readback and accepts completion", async () => {
    globalThis.indexedDB = new IDBFactory();
    globalThis.IDBKeyRange = IDBKeyRange;
    let release;
    const repository = createChatRepository({ scope: { deviceId: "device-held-upload" }, call: vi.fn() });
    const chat = repository.controller({ entityId: "run-1", agentId: "agent-1", conversationId: "thread-1" });
    const announced = vi.fn();
    chat.subscribe(announced);
    const { host } = mount({
      ...chat.bindDraft(),
      upload: (file) => new Promise((resolve) => {
        release = () => resolve({ name: file.name, path: "stored/held.txt", mime: "text/plain", size: 1 });
      }),
    });
    host.dispatchEvent(dropOf([new File(["a"], "held.txt", { type: "text/plain" })]));
    await vi.waitFor(() => expect(release).toBeTypeOf("function"));
    const address = { deviceId: "device-held-upload", entityId: "thread-1", kind: "ui-draft", sub: "chat:agent:run-1:agent-1" };
    await vi.waitFor(async () => expect((await readCached(address))?.value.attachments[0].status).toBe("failed"));
    await vi.waitFor(() => expect(announced).toHaveBeenCalled());
    expect(chat.readAttachments()[0]).toMatchObject({ name: "held.txt", status: "uploading" });
    expect(chat.readAttachments()[0].pending).toBeInstanceOf(Promise);
    release();
    await vi.waitFor(() => expect(chat.readAttachments()[0].status).toBe("ready"));
    await vi.waitFor(async () => expect((await readCached(address))?.value.attachments[0].status).toBe("ready"));
    repository.dispose();
  });

  it("does not let an old upload completion replace a newer attachment draft", async () => {
    let release;
    const repository = createChatRepository({ scope: { key: "device-a" }, call: vi.fn() });
    const chat = repository.controller({ entityId: "run-1", agentId: "agent-1", conversationId: "thread-1" });
    const first = mount({
      ...chat.bindDraft(),
      upload: (file) => new Promise((resolve) => {
        release = () => resolve({ name: file.name, path: "stored/old.txt", mime: "text/plain", size: 1 });
      }),
    });
    first.host.dispatchEvent(dropOf([new File(["a"], "old.txt")]));
    await settle();
    chat.writeAttachments([{ name: "new.txt", path: "stored/new.txt", status: "ready" }]);

    release();
    await settle();

    expect(chat.readAttachments().map((entry) => entry.name)).toEqual(["new.txt"]);
  });
});

describe("size for a human", () => {
  it("reads at a glance", () => {
    expect(formatAttachmentSize(0)).toBe("0 B");
    expect(formatAttachmentSize(999)).toBe("999 B");
    expect(formatAttachmentSize(2048)).toBe("2 KB");
    expect(formatAttachmentSize(1_500_000)).toBe("1.5 MB");
  });
});
