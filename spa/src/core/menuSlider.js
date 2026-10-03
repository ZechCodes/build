// Discrete settings in a split menu. The range holds option indexes, never
// token counts: the only values a pointer or keyboard can choose are stops.
import { esc } from "./text.js";

export const MENU_SLIDER_SELECTOR = '.menu-slider input[role="slider"]';
const adjustmentKeys = new Set(["ArrowLeft", "ArrowRight"]);
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

/** Left/Right preview range stops; Up/Down/Home/End still walk the menu. */
export const adjustsMenuSlider = (event) =>
  event.target.matches(MENU_SLIDER_SELECTOR) && adjustmentKeys.has(event.key);

const controls = new WeakMap();
let descriptionCount = 0;

function mountSlider(slider, { choose, onCommit }) {
  const host = slider.closest(".menu-slider");
  const options = JSON.parse(host.dataset.options);
  const word = host.querySelector(".mt");
  const description = host.querySelector(".md");
  // Allocate when mounting rather than rendering, so unchanged cached markup
  // keeps the open menu and the control under the pointer intact.
  description.id = `menu-slider-description-${(descriptionCount += 1)}`;
  slider.setAttribute("aria-describedby", description.id);
  let pointerId = null;
  const paint = () => {
    const option = options[Number(slider.value)];
    slider.dataset.action = option.id;
    slider.setAttribute("aria-valuetext", option.label);
    word.textContent = option.label;
    description.textContent = option.description;
  };
  const reset = () => {
    pointerId = null;
    slider.value = String(selectedIndex(options));
    paint();
  };
  const commit = () => {
    if (slider.value === String(selectedIndex(options))) return;
    paint();
    const optionId = slider.dataset.action;
    // Clear the preview before saving: focus changes and cache remounts can
    // cause another blur, and Escape resets before moving focus as well.
    reset();
    onCommit(optionId);
  };
  controls.set(slider, { reset, commit });
  slider.oninput = paint;
  // Native keyboard and assistive-tech changes are previews. Only an actual
  // pointer release chooses immediately; Enter is handled by the menu.
  slider.onchange = paint;
  slider.onblur = commit;
  slider.onpointerdown = (event) => {
    pointerId = event.pointerId;
    slider.setPointerCapture?.(pointerId);
  };
  slider.onpointerup = (event) => {
    if (event.pointerId !== pointerId) return;
    pointerId = null;
    paint();
    choose(slider);
  };
  slider.onpointercancel = reset;
}

export function mountMenuSliders(menu, callbacks) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => mountSlider(slider, callbacks));
}

/** Outside pointerdown precedes blur, so save its preview before the menu
 * closes and restores the cached stop. */
export function commitMenuSliders(menu) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => controls.get(slider)?.commit());
}

/** A preview is local to the gesture. After closing, only the cached choice
 * stands, including when the settings write fails or is still in flight. */
export function resetMenuSliders(menu) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => controls.get(slider)?.reset());
}
