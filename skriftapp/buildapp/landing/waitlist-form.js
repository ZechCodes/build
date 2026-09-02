export const SENDING_LABEL = "SENDING…";

export function installWaitlist({ wrapperElement, submitWaitlistEmail }) {
  const form = wrapperElement.querySelector(".waitlist-form");
  const input = form.querySelector(".waitlist-input");
  const submitButton = form.querySelector(".button-primary--form");
  const errorElement = wrapperElement.querySelector(".waitlist-error");
  const successElement = wrapperElement.querySelector(".waitlist-success");
  const confirmationElement = wrapperElement.querySelector(".waitlist-confirmation");
  const emailSlot = wrapperElement.querySelector("[data-waitlist-email]");
  const idleLabel = submitButton.textContent;

  form.addEventListener("submit", async (submitEvent) => {
    submitEvent.preventDefault();
    if (!input.checkValidity()) return;
    input.disabled = true;
    submitButton.disabled = true;
    submitButton.textContent = SENDING_LABEL;
    errorElement.hidden = true;
    const accepted = await submitWaitlistEmail(input.value).catch(() => false);
    submitButton.textContent = idleLabel;
    input.disabled = false;
    submitButton.disabled = false;
    if (accepted) {
      emailSlot.textContent = input.value;
      form.hidden = true;
      successElement.hidden = false;
      confirmationElement.hidden = false;
    } else {
      errorElement.hidden = false;
    }
  });
}
