// @vitest-environment jsdom
import { describe, expect, it, beforeEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  SENDING_LABEL,
  installWaitlist,
} from "../../skriftapp/buildapp/landing/waitlist-form.js";

const WAITLIST_MARKUP = readFileSync(
  resolve("../skriftapp/buildapp/landing/waitlist.html"),
  "utf8",
);
const VALID_EMAIL = "someone@example.com";
const INVALID_EMAIL = "not-an-email";

let wrapperElement;
let form;
let input;
let submitButton;
let errorElement;
let successElement;
let confirmationElement;
let emailSlot;
let idleLabel;

function mountWaitlist(submitWaitlistEmail) {
  document.body.innerHTML = WAITLIST_MARKUP;
  wrapperElement = document.querySelector("[data-waitlist]");
  form = wrapperElement.querySelector(".waitlist-form");
  input = form.querySelector(".waitlist-input");
  submitButton = form.querySelector(".button-primary--form");
  errorElement = wrapperElement.querySelector(".waitlist-error");
  successElement = wrapperElement.querySelector(".waitlist-success");
  confirmationElement = wrapperElement.querySelector(".waitlist-confirmation");
  emailSlot = wrapperElement.querySelector("[data-waitlist-email]");
  idleLabel = submitButton.textContent;
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

  it("the input is disabled and the button reads SENDING… while the request is in flight", async () => {
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
    expect(input.disabled).toBe(true);
    expect(submitButton.disabled).toBe(true);
    expect(submitButton.textContent).toBe(SENDING_LABEL);
    acceptRequest();
    await new Promise((settle) => setTimeout(settle, 0));
    expect(input.disabled).toBe(false);
  });

  it("an accepted submission restores the button label and shows the confirmation note beside the success line", async () => {
    const submitWaitlistEmail = vi.fn(async () => true);
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(submitButton.textContent).toBe(idleLabel);
    expect(successElement.hidden).toBe(false);
    expect(confirmationElement.hidden).toBe(false);
  });

  it("a rejected submission restores the label, re-enables the input and button, and keeps the confirmation note hidden", async () => {
    const submitWaitlistEmail = vi.fn(async () => false);
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(submitButton.textContent).toBe(idleLabel);
    expect(input.disabled).toBe(false);
    expect(submitButton.disabled).toBe(false);
    expect(confirmationElement.hidden).toBe(true);
  });

  it("a thrown request restores the label and re-enables the input", async () => {
    const submitWaitlistEmail = vi.fn(async () => {
      throw new Error("offline");
    });
    mountWaitlist(submitWaitlistEmail);
    input.value = VALID_EMAIL;
    await submitForm();
    expect(submitButton.textContent).toBe(idleLabel);
    expect(input.disabled).toBe(false);
    expect(errorElement.hidden).toBe(false);
  });
});
