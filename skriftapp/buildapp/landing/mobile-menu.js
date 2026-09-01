export const OPEN_MENU_CLASS = "is-open";

export function installMobileMenu({ toggleButton, menuElement }) {
  toggleButton.addEventListener("click", () => {
    const isOpen = menuElement.classList.toggle(OPEN_MENU_CLASS);
    toggleButton.setAttribute("aria-expanded", String(isOpen));
  });
  menuElement.addEventListener("click", () => {
    menuElement.classList.remove(OPEN_MENU_CLASS);
    toggleButton.setAttribute("aria-expanded", "false");
  });
}
