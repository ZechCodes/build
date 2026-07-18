// @vitest-environment jsdom
// DOM flow of requestSheetDismiss: an empty sheet closes through its own
// Cancel control (so promise-settling sheets are not stranded), a drafted
// sheet demands the discard confirm first, and an open confirm modal owns
// the screen (dismiss is a no-op).

import { describe, it, expect, beforeEach } from "vitest";
import { requestSheetDismiss } from "../src/core/sheetDismiss.js";
import { confirmAction } from "../src/core/confirm.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function mountSheet({ draft = "", withCancel = true } = {}) {
  document.body.innerHTML = `
    <div id="scrim" class="show">
      <div id="sheet">
        <textarea id="goal">${draft}</textarea>
        ${withCancel ? '<button id="ntcancel">Cancel</button>' : ""}
      </div>
    </div>`;
  const cancelClicks = [];
  const cancel = document.getElementById("ntcancel");
  if (cancel)
    cancel.onclick = () => {
      cancelClicks.push(1);
      document.getElementById("scrim").classList.remove("show");
    };
  return { cancelClicks };
}

describe("requestSheetDismiss (DOM)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("closes an empty sheet through its own Cancel control", () => {
    const { cancelClicks } = mountSheet();
    requestSheetDismiss();
    expect(cancelClicks).toHaveLength(1);
    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
  });

  it("falls back to hiding the scrim when the sheet has no cancel button", () => {
    mountSheet({ withCancel: false });
    requestSheetDismiss();
    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
  });

  it("a drafted sheet asks the discard confirm; cancelling keeps the sheet", async () => {
    const { cancelClicks } = mountSheet({ draft: "half-typed goal" });
    requestSheetDismiss();
    expect(document.getElementById("confirm-scrim")).toBeTruthy(); // confirm shown, sheet intact
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    document.querySelector("[data-confirm-cancel]").click();
    await flush();
    expect(cancelClicks).toHaveLength(0);
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
  });

  it("a drafted sheet discards through Cancel once confirmed", async () => {
    const { cancelClicks } = mountSheet({ draft: "half-typed goal" });
    requestSheetDismiss();
    document.querySelector("[data-confirm-ok]").click();
    await flush();
    expect(cancelClicks).toHaveLength(1);
    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
  });

  it("is a no-op while a confirm modal owns the screen", async () => {
    const { cancelClicks } = mountSheet();
    const pending = confirmAction({ title: "t" });
    requestSheetDismiss();
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    expect(cancelClicks).toHaveLength(0);
    document.querySelector("[data-confirm-cancel]").click();
    await pending;
  });
});
