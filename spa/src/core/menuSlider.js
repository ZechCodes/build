// Discrete settings in a split menu. The range holds option indexes, never
// token counts: the only values a pointer or keyboard can choose are stops.
import { esc } from "./text.js";

export const MENU_SLIDER_SELECTOR = '.menu-slider input[role="slider"]';
const adjustmentKeys = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"]);
const selectedIndex = (options) => Math.max(0, options.findIndex((option) => option.selected));

export function menuSliderMarkup({ label, options }) {
  const index = selectedIndex(options);
  const option = options[index];
  return `<div class="menu-slider" data-options="${esc(JSON.stringify(options))}">
    <input type="range" role="slider" min="0" max="${options.length - 1}" step="1" value="${index}" tabindex="0"
      aria-label="${esc(label)}" aria-valuetext="${esc(option.label)}" data-action="${esc(option.id)}">
    <div class="menu-slider-stops" aria-hidden="true">${options.map(() => "<span></span>").join("")}</div>
    <div class="menu-slider-copy"><span class="mt">${esc(option.label)}</span><span class="md">${esc(option.description)}</span></div>
  </div>`;
}

/** Menu navigation leaves range adjustment to the browser. Escape and Enter
 * still belong to the menu, and Tab can leave the control normally. */
export const adjustsMenuSlider = (event) =>
  event.target.matches(MENU_SLIDER_SELECTOR) && adjustmentKeys.has(event.key);

const resets = new WeakMap();
let descriptionCount = 0;

function mountSlider(slider, choose) {
  const host = slider.closest(".menu-slider");
  const options = JSON.parse(host.dataset.options);
  const word = host.querySelector(".mt");
  const description = host.querySelector(".md");
  // Allocate when mounting rather than rendering, so unchanged cached markup
  // keeps the open menu and the control under the pointer intact.
  description.id = `menu-slider-description-${(descriptionCount += 1)}`;
  slider.setAttribute("aria-describedby", description.id);
  const paint = () => {
    const option = options[Number(slider.value)];
    slider.dataset.action = option.id;
    slider.setAttribute("aria-valuetext", option.label);
    word.textContent = option.label;
    description.textContent = option.description;
  };
  const reset = () => {
    slider.value = String(selectedIndex(options));
    paint();
  };
  resets.set(slider, reset);
  slider.oninput = paint;
  slider.onchange = () => {
    paint();
    choose(slider);
  };
}

export function mountMenuSliders(menu, choose) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => mountSlider(slider, choose));
}

/** A preview is local to the gesture. After closing, only the cached choice
 * stands, including when the settings write fails or is still in flight. */
export function resetMenuSliders(menu) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => resets.get(slider)?.());
}
