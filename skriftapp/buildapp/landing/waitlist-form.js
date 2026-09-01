export function installWaitlist({ wrapperElement, submitWaitlistEmail }) {
  const form = wrapperElement.querySelector(".waitlist-form");
  const input = form.querySelector(".waitlist-input");
  const submitButton = form.querySelector(".button-primary--form");
  const errorElement = wrapperElement.querySelector(".waitlist-error");
  const successElement = wrapperElement.querySelector(".waitlist-success");
  const emailSlot = wrapperElement.querySelector("[data-waitlist-email]");

  form.addEventListener("submit", async (submitEvent) => {
    submitEvent.preventDefault();
    if (!input.checkValidity()) return;
    submitButton.disabled = true;
    errorElement.hidden = true;
    const accepted = await submitWaitlistEmail(input.value).catch(() => false);
    submitButton.disabled = false;
    if (accepted) {
      emailSlot.textContent = input.value;
      form.hidden = true;
      successElement.hidden = false;
    } else {
      errorElement.hidden = false;
    }
  });
}
