// @vitest-environment jsdom
import { describe, expect, it, beforeEach, vi } from "vitest";
import { MOBILE_BREAKPOINT_QUERY } from "../../skriftapp/buildapp/landing/breakpoint.js";
import {
  OPEN_MENU_CLASS,
  installMobileMenu,
} from "../../skriftapp/buildapp/landing/mobile-menu.js";

let toggleButton;
let menuElement;
let menuLink;
let breakpointListeners;
let observedQuery;

beforeEach(() => {
  breakpointListeners = [];
  observedQuery = "";
  globalThis.matchMedia = vi.fn((query) => {
    observedQuery = query;
    return {
      matches: false,
      addEventListener: (eventName, listener) => breakpointListeners.push(listener),
    };
  });
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

  it("crossing the mobile breakpoint closes the open menu so the desktop nav is not left solid", () => {
    expect(observedQuery).toBe(MOBILE_BREAKPOINT_QUERY);
    toggleButton.click();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(true);
    for (const listener of breakpointListeners) listener();
    expect(menuElement.classList.contains(OPEN_MENU_CLASS)).toBe(false);
    expect(toggleButton.getAttribute("aria-expanded")).toBe("false");
  });
});
