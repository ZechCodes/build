import { installNavSolidityObserver } from "./nav-solidity.js";
import { installMobileMenu } from "./mobile-menu.js";
import { installDotField } from "./dot-field.js";
import { installFeatureRail } from "./feature-rail.js";
import { installWaitlist } from "./waitlist-form.js";
import { submitWaitlistEmail } from "./waitlist-api.js";

const navElement = document.querySelector("[data-nav]");
const heroElement = document.querySelector("[data-hero]");
const canvasElement = document.querySelector("[data-dot-field]");
const toggleButton = document.querySelector("[data-nav-toggle]");
const menuElement = document.querySelector("[data-nav-menu]");
const railElement = document.querySelector("[data-feature-rail]");
const cardElements = Array.from(document.querySelectorAll("[data-feature-card]"));
const dashElements = Array.from(document.querySelectorAll("[data-rail-dash]"));
const waitlistWrappers = Array.from(document.querySelectorAll("[data-waitlist]"));

if (navElement && heroElement) installNavSolidityObserver({ navElement, heroElement });
if (toggleButton && menuElement) installMobileMenu({ toggleButton, menuElement });
if (canvasElement) installDotField(canvasElement);
if (railElement && cardElements.length && dashElements.length) {
  installFeatureRail({ railElement, cardElements, dashElements });
}
for (const wrapperElement of waitlistWrappers) {
  installWaitlist({ wrapperElement, submitWaitlistEmail });
}
