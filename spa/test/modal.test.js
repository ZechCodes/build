// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from "vitest";
import { modalDialogHtml, openModal } from "../src/core/modal.js";
import { motionBeat, recordAnimations, settleMotion, stopRecordingAnimations } from "./motionRecorder.js";

const DIALOG = modalDialogHtml("<p>the words</p>");

const scrimOnScreen = () => document.querySelector(".modal-scrim");

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

    const scrim = scrimOnScreen();
    expect(scrim).not.toBe(null);
    expect(body).toBe(scrim.querySelector(".modal"));
    expect(body.textContent).toContain("the words");
  });

  it("stands on the body with no host named, and inside the host when one is", () => {
    openModal({ dialogHtml: DIALOG });
    const onBody = scrimOnScreen();
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

  it("takes the scrim id its caller asks for, and none when it does not", async () => {
    const named = openModal({ dialogHtml: DIALOG, scrimId: "surface-scrim" });
    expect(document.getElementById("surface-scrim")).not.toBe(null);
    await named.close();

    openModal({ dialogHtml: DIALOG });
    expect(scrimOnScreen().id).toBe("");
  });

  it("closes on Escape, tells its caller once, and lets no handler under it see the press", async () => {
    const onClose = vi.fn();
    let underlyingSawEscape = false;
    const underlying = (event) => {
      if (event.key === "Escape") underlyingSawEscape = true;
    };
    document.addEventListener("keydown", underlying);
    openModal({ dialogHtml: DIALOG, onClose });

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await vi.waitFor(() => expect(scrimOnScreen()).toBe(null));

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(underlyingSawEscape).toBe(false);
    expect(scrimOnScreen()).toBe(null);
    document.removeEventListener("keydown", underlying);
  });

  it("takes its capture listener with it, so Escape propagates again once it is closed", async () => {
    const onClose = vi.fn();
    let underlyingSawEscape = false;
    const underlying = (event) => {
      if (event.key === "Escape") underlyingSawEscape = true;
    };
    document.addEventListener("keydown", underlying);
    const { close } = openModal({ dialogHtml: DIALOG, onClose });
    await close();

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));

    expect(underlyingSawEscape).toBe(true);
    expect(onClose).toHaveBeenCalledTimes(1);
    document.removeEventListener("keydown", underlying);
  });

  it("leaves a key that is not Escape to whatever is under it", async () => {
    const onClose = vi.fn();
    openModal({ dialogHtml: DIALOG, onClose });

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));

    expect(onClose).not.toHaveBeenCalled();
    expect(scrimOnScreen()).not.toBe(null);
  });

  it("closes on a press on the scrim and stays open on one inside the dialog", async () => {
    const onClose = vi.fn();
    const { body } = openModal({ dialogHtml: DIALOG, onClose });

    body.click();
    expect(onClose).not.toHaveBeenCalled();
    expect(scrimOnScreen()).not.toBe(null);

    scrimOnScreen().click();
    await vi.waitFor(() => expect(scrimOnScreen()).toBe(null));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(scrimOnScreen()).toBe(null);
  });

  it("closes at its caller's word, and a second close changes nothing", async () => {
    const onClose = vi.fn();
    const { close } = openModal({ dialogHtml: DIALOG, onClose });

    const closing = close();
    await close();
    await closing;

    expect(onClose).toHaveBeenCalledTimes(1);
    expect(scrimOnScreen()).toBe(null);
  });

  it("puts focus in the dialog once it is on screen", async () => {
    const { body } = openModal({ dialogHtml: modalDialogHtml("<button>Cancel</button><button>Confirm</button>") });
    await vi.waitFor(() => expect(document.activeElement).toBe(body.querySelector("button")));
  });
});

describe("the modal's motion", () => {
  let started = [];

  beforeEach(() => {
    vi.useFakeTimers();
    started = recordAnimations();
  });
  afterEach(async () => {
    try {
      await settleMotion();
    } finally {
      stopRecordingAnimations();
      vi.useRealTimers();
      document.body.innerHTML = "";
    }
  });

  const keyframedProperties = (run) => Object.keys(run.keyframes[0]);

  it("leaves focus where its caller put it when the opening motion finishes", async () => {
    const { body } = openModal({ dialogHtml: modalDialogHtml("<button>Cancel</button><button>Confirm</button>") });
    const confirm = body.querySelectorAll("button")[1];
    confirm.focus();

    await settleMotion();

    expect(document.activeElement).toBe(confirm);
  });

  it("fades the scrim in and grows the dialog into it", async () => {
    const { body } = openModal({ dialogHtml: DIALOG });
    const scrim = scrimOnScreen();

    await motionBeat();

    expect(started).toHaveLength(2);
    expect(started[0].element).toBe(scrim);
    expect(started[0].keyframes).toEqual([{ opacity: 0 }, { opacity: 1 }]);
    expect(started[1].element).toBe(body);
    expect(keyframedProperties(started[1])).toContain("height");

    await settleMotion();
    expect(scrim.hidden).toBe(false);
    expect(body.hidden).toBe(false);
  });

  it("shrinks the dialog and fades the scrim before the scrim leaves the document", async () => {
    const onClose = vi.fn();
    const { body, close } = openModal({ dialogHtml: DIALOG, onClose });
    await settleMotion();
    const scrim = scrimOnScreen();
    started.length = 0;

    const closing = close();
    await motionBeat();

    expect(started).toHaveLength(2);
    expect(started.map((run) => run.element)).toEqual([body, scrim]);
    expect(started[1].keyframes).toEqual([{ opacity: 1 }, { opacity: 0 }]);
    expect(scrimOnScreen()).toBe(scrim);
    expect(onClose).not.toHaveBeenCalled();

    await settleMotion();
    await closing;

    expect(scrimOnScreen()).toBe(null);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("takes Escape through the same closing move", async () => {
    const onClose = vi.fn();
    openModal({ dialogHtml: DIALOG, onClose });
    await settleMotion();
    started.length = 0;

    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await motionBeat();

    expect(started).toHaveLength(2);
    expect(scrimOnScreen()).not.toBe(null);

    await settleMotion();
    await motionBeat();

    expect(scrimOnScreen()).toBe(null);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("takes a press on the scrim through the same closing move", async () => {
    const onClose = vi.fn();
    openModal({ dialogHtml: DIALOG, onClose });
    await settleMotion();
    started.length = 0;

    scrimOnScreen().click();
    await motionBeat();

    expect(started).toHaveLength(2);
    expect(scrimOnScreen()).not.toBe(null);

    await settleMotion();
    await motionBeat();

    expect(scrimOnScreen()).toBe(null);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
