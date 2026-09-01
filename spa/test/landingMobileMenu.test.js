// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from "vitest";
import {
  OPEN_MENU_CLASS,
  installMobileMenu,
} from "../../skriftapp/buildapp/landing/mobile-menu.js";

let toggleButton;
let menuElement;
let menuLink;

beforeEach(() => {
  document.body.innerHTML = `
    <button data-nav-toggle aria-expanded="false"></button>
    <ul class="nav-links" data-nav-menu><li><a href="#features">[features]</a></li></ul>
  `;
  toggleButton = document.querySelector("[data-nav-toggle]");
  menuElement = document.querySelector("[data-nav-menu]");
  menuLink = menuElement.querySelector("a");
  installMobileMenu({ toggleButton, menuElement });
});

describe("mobile menu", () => {
  it("the toggle opens and closes the menu and mirrors aria-expanded", () => {
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(false);
    toggleButton.click();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(true);
    expect(toggleButton.getAttribute("aria-expanded")).toBe("true");
    toggleButton.click();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(false);
    expect(toggleButton.getAttribute("aria-expanded")).toBe("false");
  });

  it("a tap inside the open menu closes it", () => {
    toggleButton.click();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(true);
    menuLink.click();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(false);
    expect(toggleButton.getAttribute("aria-expanded")).toBe("false");
  });
});
