/** @vitest-environment jsdom */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { composerHtml, mountComposerAttachments } from "../src/core/composer.js";
import { secretComposerAvailable } from "../src/core/secretComposer.js";

const IDS = { input: "chat-input", send: "chat-send", hint: "chat-hint" };

function mount({ secretAttachable = false, onSecret = vi.fn() } = {}) {
  document.body.innerHTML = composerHtml({
    inputId: IDS.input,
    sendId: IDS.send,
    hintId: IDS.hint,
    placeholder: "Say something",
    attachable: true,
    secretAttachable,
  });
  const picker = document.querySelector("#chat-inputfile");
  vi.spyOn(picker, "click").mockImplementation(() => {});
  const controller = mountComposerAttachments(document.body, {
    ids: IDS,
    upload: vi.fn(),
    secretAttachable,
    onSecret,
  });
  return { controller, picker, onSecret };
}

beforeEach(() => { document.body.innerHTML = ""; });

describe("the composer attachment menu", () => {
  it("is a named ARIA menu with files first and no unsupported Secret action", () => {
    mount();
    const opener = document.querySelector(".composer-attach [aria-haspopup=menu]");
    expect(opener.getAttribute("aria-expanded")).toBe("false");
    expect(document.querySelector(".composer-attach [role=menu]").getAttribute("aria-label")).toBe("Attach");
    expect([...document.querySelectorAll(".composer-attach [role=menuitem]")].map((row) => row.textContent.trim()))
      .toEqual(["Files or imagesChoose a file or image from this device"]);
  });

  it("does not offer Secret until the selected conversation has a thread", () => {
    expect(secretComposerAvailable(true, null)).toBe(false);
    expect(secretComposerAvailable(true, "thread-1")).toBe(true);
    expect(secretComposerAvailable(false, "thread-1")).toBe(false);
  });

  it("shows Secret only when supported and routes both actions without changing file input behavior", () => {
    const { picker, onSecret } = mount({ secretAttachable: true });
    const rows = [...document.querySelectorAll(".composer-attach [role=menuitem]")];
    expect(rows.map((row) => row.querySelector(".mt").textContent)).toEqual(["Files or images", "Secret"]);
    expect(rows[1].querySelector(".md").textContent).toBe("Seal a value for an agent or project");
    rows[0].click();
    expect(picker.click).toHaveBeenCalledOnce();
    rows[1].click();
    expect(onSecret).toHaveBeenCalledOnce();
  });

  it("adds and removes Secret live without replacing the textarea", () => {
    const { controller, onSecret } = mount();
    const input = document.querySelector("#chat-input");
    controller.setSecretAttachable(true);
    expect(document.querySelector('[data-action="secret"]')).not.toBeNull();
    expect(document.querySelector("#chat-input")).toBe(input);
    document.querySelector('[data-action="secret"]').click();
    expect(onSecret).toHaveBeenCalledOnce();
    controller.setSecretAttachable(false);
    expect(document.querySelector('[data-action="secret"]')).toBeNull();
    expect(document.querySelector("#chat-input")).toBe(input);
  });
});
