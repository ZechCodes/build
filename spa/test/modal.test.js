// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from "vitest";
import { modalDialogHtml, openModal } from "../src/core/modal.js";

const DIALOG = modalDialogHtml("<p>the words</p>");

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("modalDialogHtml", () => {
  it("writes the dialog every modal wears, and takes a class beside it", () => {
    expect(DIALOG).toContain('class="modal"');
    expect(DIALOG).toContain('role="dialog"');
    expect(DIALOG).toContain('aria-modal="true"');
    expect(DIALOG).toContain("<p>the words</p>");
    expect(modalDialogHtml("", { className: "modal-surface" })).toContain('class="modal modal-surface"');
  });
});

describe("openModal", () => {
  it("puts the dialog on screen and hands back the body to fill", () => {
    const { body } = openModal({ dialogHtml: DIALOG });

    const scrim = document.querySelector(".modal-scrim");
    expect(scrim).not.toBe(null);
    expect(body).toBe(scrim.querySelector(".modal"));
    expect(body.textContent).toContain("the words");
  });

  it("stands on the body with no host named, and inside the host when one is", () => {
    openModal({ dialogHtml: DIALOG });
    const onBody = document.querySelector(".modal-scrim");
    expect(onBody.parentElement).toBe(document.body);
    expect(onBody.classList.contains("modal-scrim-local")).toBe(false);
    onBody.remove();

    const host = document.createElement("section");
    document.body.appendChild(host);
    const { body } = openModal({ dialogHtml: DIALOG, host });
    const local = host.querySelector(".modal-scrim");
    expect(local).not.toBe(null);
    expect(local.classList.contains("modal-scrim-local")).toBe(true);
    expect(body).toBe(local.querySelector(".modal"));
  });

  it("takes the scrim id its caller asks for, and none when it does not", () => {
    const named = openModal({ dialogHtml: DIALOG, scrimId: "surface-scrim" });
    expect(document.getElementById("surface-scrim")).not.toBe(null);
    named.close();

    openModal({ dialogHtml: DIALOG });
    expect(document.querySelector(".modal-scrim").id).toBe("");
  });

  it("closes on Escape, tells its caller once, and lets no handler under it see the press", () => {
    const onClose = vi.fn();
    let underlyingSawEscape = false;
    const underlying = (event) => {
      if (event.key === "Escape") underlyingSawEscape = true;
    };
    document.addEventListener("keydown", underlying);
    openModal({ dialogHtml: DIALOG, onClose });

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(underlyingSawEscape).toBe(false);
    expect(document.querySelector(".modal-scrim")).toBe(null);
    document.removeEventListener("keydown", underlying);
  });

  it("takes its capture listener with it, so Escape propagates again once it is closed", () => {
    const onClose = vi.fn();
    let underlyingSawEscape = false;
    const underlying = (event) => {
      if (event.key === "Escape") underlyingSawEscape = true;
    };
    document.addEventListener("keydown", underlying);
    const { close } = openModal({ dialogHtml: DIALOG, onClose });
    close();

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    expect(underlyingSawEscape).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
    document.removeEventListener("keydown", underlying);
  });

  it("leaves a key that is not Escape to whatever is under it", () => {
    const onClose = vi.fn();
    openModal({ dialogHtml: DIALOG, onClose });

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));

    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector(".modal-scrim")).not.toBe(null);
  });

  it("closes on a press on the scrim and stays open on one inside the dialog", () => {
    const onClose = vi.fn();
    const { body } = openModal({ dialogHtml: DIALOG, onClose });

    body.click();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.querySelector(".modal-scrim")).not.toBe(null);

    document.querySelector(".modal-scrim").click();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".modal-scrim")).toBe(null);
  });

  it("closes at its caller's word, and a second close changes nothing", () => {
    const onClose = vi.fn();
    const { close } = openModal({ dialogHtml: DIALOG, onClose });

    close();
    close();

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".modal-scrim")).toBe(null);
  });
});
