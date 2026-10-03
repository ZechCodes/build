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
    <div class="menu-slider-copy"><span class="mt">${esc(option.label)}</span></div>
  </div>`;
}

/** Left/Right preview range stops; Up/Down/Home/End still walk the menu. */
export const adjustsMenuSlider = (event) =>
  event.target.matches(MENU_SLIDER_SELECTOR) && adjustmentKeys.has(event.key);

const controls = new WeakMap();
const sliderKey = (slider) => slider.closest("[data-group]")?.dataset.group || slider.getAttribute("aria-label");

function mountSlider(slider, { choose, onCommit }, held) {
  const host = slider.closest(".menu-slider");
  const options = JSON.parse(host.dataset.options);
  const word = host.querySelector(".mt");
  const cachedAction = options[selectedIndex(options)].id;
  const state = held?.cachedAction === cachedAction ? held
    : { cachedAction, committedAction: cachedAction, pending: false };
  let pointerId = null;
  const paint = () => {
    const option = options[Number(slider.value)];
    slider.dataset.action = option.id;
    slider.setAttribute("aria-valuetext", option.label);
    word.textContent = option.label;
  };
  const reset = () => {
    pointerId = null;
    slider.value = String(Math.max(0, options.findIndex((option) => option.id === state.committedAction)));
    paint();
  };
  const rollback = () => {
    state.committedAction = state.cachedAction;
    state.repaint();
  };
  const commit = async () => {
    paint();
    if (state.pending || slider.dataset.action === state.committedAction) {
      reset();
      return;
    }
    // A committed value belongs to this slider until the cache answers. Later
    // blur/close events cancel only a newer preview and cannot resend it.
    state.committedAction = slider.dataset.action;
    state.pending = true;
    pointerId = null;
    try {
      if (await onCommit(state.committedAction) === false) rollback();
    } catch {
      rollback();
    } finally {
      state.pending = false;
    }
  };
  state.repaint = reset;
  reset();
  controls.set(slider, { reset, commit, state });
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

export function mountMenuSliders(menu, callbacks, states = new Map()) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => mountSlider(slider, callbacks, states.get(sliderKey(slider))));
}

/** Keep a pending choice through a menu repaint with the same cached stop.
 * A genuinely changed cache value creates a fresh control state instead. */
export function menuSliderStates(menu) {
  return new Map([...menu.querySelectorAll(MENU_SLIDER_SELECTOR)]
    .map((slider) => [sliderKey(slider), controls.get(slider)?.state]));
}

export const commitMenuSlider = (slider) => controls.get(slider)?.commit();

/** Outside pointerdown precedes blur, so save its preview before closing. */
export function commitMenuSliders(menu) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach(commitMenuSlider);
}

/** Cancel uncommitted previews, keeping any save already in flight. */
export function resetMenuSliders(menu) {
  menu.querySelectorAll(MENU_SLIDER_SELECTOR).forEach((slider) => controls.get(slider)?.reset());
}
