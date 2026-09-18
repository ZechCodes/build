import { MOBILE_BREAKPOINT_QUERY } from "./breakpoint.js";

export const OPEN_MENU_CLASS = "is-open";
export const BODY_OPEN_CLASS = "is-menu-open";

export function installMobileMenu({ toggleButton, menuElement }) {
  function setOpen(isOpen) {
    menuElement.classList.toggle(OPEN_MENU_CLASS, isOpen);
    toggleButton.setAttribute("aria-expanded", String(isOpen));
    document.body.classList.toggle(BODY_OPEN_CLASS, isOpen);
  }

  function closeMenu() {
    setOpen(false);
  }

  toggleButton.addEventListener("click", () => {
    setOpen(!menuElement.classList.contains(OPEN_MENU_CLASS));
  });
  menuElement.addEventListener("click", closeMenu);
  document.addEventListener("click", (event) => {
    if (!menuElement.classList.contains(OPEN_MENU_CLASS)) return;
    if (menuElement.contains(event.target) || toggleButton.contains(event.target)) return;
    closeMenu();
  });
  matchMedia(MOBILE_BREAKPOINT_QUERY).addEventListener("change", closeMenu);
}
