import { MOBILE_BREAKPOINT_QUERY } from "./breakpoint.js";

export const OPEN_MENU_CLASS = "is-open";

export function installMobileMenu({ toggleButton, menuElement }) {
  function closeMenu() {
    menuElement.classList.remove(OPEN_MENU_CLASS);
    toggleButton.setAttribute("aria-expanded", "false");
  }

  toggleButton.addEventListener("click", () => {
    const isOpen = menuElement.classList.toggle(OPEN_MENU_CLASS);
    toggleButton.setAttribute("aria-expanded", String(isOpen));
  });
  menuElement.addEventListener("click", closeMenu);
  matchMedia(MOBILE_BREAKPOINT_QUERY).addEventListener("change", closeMenu);
}
