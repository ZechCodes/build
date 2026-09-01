// @vitest-environment jsdom
import { describe, expect, it, beforeEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { installWaitlist } from "../../skriftapp/buildapp/landing/waitlist-form.js";

const LANDING_HTML = readFileSync(
  resolve("../skriftapp/buildapp/landing/index.html"),
  "utf8",
);
const WAITLIST_MARKUP = LANDING_HTML.match(
  /<div class="waitlist" data-waitlist>[\s\S]*?<\/div>/,
)[0];
const VALID_EMAIL = "someone@example.com";
const INVALID_EMAIL = "not-an-email";

let wrapperElement;
let form;
let input;
let submitButton;
let errorElement;
let successElement;
let emailSlot;

function mountWaitlist(submitWaitlistEmail) {
  document.body.innerHTML = WAITLIST_MARKUP;
  wrapperElement = document.querySelector("[data-waitlist]");
  form = wrapperElement.querySelector("form");
  input = form.querySelector("input");
  submitButton = form.querySelector("button");
  errorElement = wrapperElement.querySelector(".waitlist-error");
  successElement = wrapperElement.querySelector(".waitlist-success");
  emailSlot = wrapperElement.querySelector("[data-waitlist-email]");
  installWaitlist({ wrapperElement, submitWaitlistEmail });
}

function submitForm() {
  form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  return new Promise((settle) => setTimeout(settle, 0));
}

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("waitlist behaviour", () => {
  it("an invalid address sends no request and shows nothing", async () => {
    const submitWaitlistEmail = vi.fn(async () => true);
    mountWaitlist(submitWaitlistEmail);
    input.value = INVALID_EMAIL;
    await submitForm();
    expect(submitWaitlistEmail).not.toHaveBeenCalled();
    expect(errorElement.hidden).toBe(true);
    expect(successElement.hidden).toBe(true);
    expect(form.hidden).toBe(false);
  });

  it("an accepted submission hides the form and shows the success line with the address", async () => {
    const submitWaitlistEmail = vi.fn(async () => true);
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(submitWaitlistEmail).toHaveBeenCalledWith(VALID_EMAIL);
    expect(form.hidden).toBe(true);
    expect(successElement.hidden).toBe(false);
    expect(emailSlot.textContent).toBe(VALID_EMAIL);
    expect(errorElement.hidden).toBe(true);
  });

  it("a rejected submission shows the error line and keeps the form usable", async () => {
    const submitWaitlistEmail = vi.fn(async () => false);
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(errorElement.hidden).toBe(false);
    expect(successElement.hidden).toBe(true);
    expect(form.hidden).toBe(false);
    expect(submitButton.disabled).toBe(false);
  });

  it("a thrown request shows the error line and keeps the form usable", async () => {
    const submitWaitlistEmail = vi.fn(async () => {
      throw new Error("offline");
    });
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(errorElement.hidden).toBe(false);
    expect(form.hidden).toBe(false);
    expect(submitButton.disabled).toBe(false);
  });

  it("the submit button is disabled while the request is in flight", async () => {
    let acceptRequest;
    const submitWaitlistEmail = vi.fn(
      () =>
        new Promise((settle) => {
          acceptRequest = () => settle(true);
        }),
    );
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await new Promise((settle) => setTimeout(settle, 0));
    expect(submitButton.disabled).toBe(true);
    acceptRequest();
    await new Promise((settle) => setTimeout(settle, 0));
    expect(submitButton.disabled).toBe(false);
    expect(successElement.hidden).toBe(false);
  });
});
