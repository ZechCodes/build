// The waitlist form's wiring, kept as a separate external module so the form
// works on a page whose only other script is the choreography's.
//
// The form's behaviour and its POST /api/waitlist client are the files the
// rest of the site already ships, served from /landing/ — this module only
// introduces them to the markup, exactly as the old landing main.js did.
import { installWaitlist } from "/landing/waitlist-form.js";
import { submitWaitlistEmail } from "/landing/waitlist-api.js";

for (const wrapperElement of document.querySelectorAll("[data-waitlist]")) {
  installWaitlist({ wrapperElement, submitWaitlistEmail });
}
