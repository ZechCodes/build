// @vitest-environment jsdom
// bindAction's failure path follows the G2 grammar: a persistent, expandable
// error notification carrying the FULL message — not a truncated hint.

import { describe, it, expect, beforeEach } from "vitest";
import { bindAction } from "../src/views/stages.js";
import { dismissAllNotices } from "../src/core/notify.js";

const LONG_ERROR = "stage dispatch rejected: " + "x".repeat(200);

describe("bindAction failure notifications (DOM)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    dismissAllNotices();
  });

  it("shows busy label while pending, then restores and raises a full-text error notice on failure", async () => {
    const button = document.createElement("button");
    button.textContent = "Start stage";
    document.body.appendChild(button);
    let reject;
    bindAction(button, "starting…", () => new Promise((_, rej) => (reject = rej)));

    button.click();
    expect(button.disabled).toBe(true);
    expect(button.textContent).toBe("starting…");

    reject(new Error(LONG_ERROR));
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(button.disabled).toBe(false);
    expect(button.textContent).toBe("Start stage");
    const notice = document.querySelector("#notices .notice.error");
    expect(notice).toBeTruthy();
    expect(notice.querySelector(".notice-summary").textContent).toContain("Start stage failed");
    expect(notice.querySelector(".notice-detail").textContent).toBe(LONG_ERROR); // full text, not sliced
    expect(notice.querySelector(".notice-x")).toBeTruthy(); // dismissable
  });

  it("leaves no notice on success", async () => {
    const button = document.createElement("button");
    button.textContent = "Approve";
    document.body.appendChild(button);
    bindAction(button, "approving…", () => Promise.resolve());
    button.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.querySelector("#notices .notice")).toBeNull();
  });
});
