import { readNumericToken } from "./css-token.js";
import { MOBILE_BREAKPOINT_QUERY } from "./breakpoint.js";

export const SOLID_NAV_CLASS = "is-solid";

export function installNavSolidityObserver({ navElement, heroElement }) {
  let heroObserver = null;

  function observeHero() {
    const navHeight = readNumericToken(navElement, "--nav-height");
    heroObserver = new IntersectionObserver(
      ([heroEntry]) =>
        navElement.classList.toggle(SOLID_NAV_CLASS, !heroEntry.isIntersecting),
      { rootMargin: `-${navHeight}px 0px 0px 0px`, threshold: 0 },
    );
    heroObserver.observe(heroElement);
  }

  observeHero();
  matchMedia(MOBILE_BREAKPOINT_QUERY).addEventListener("change", () => {
    heroObserver.disconnect();
    observeHero();
  });
}
