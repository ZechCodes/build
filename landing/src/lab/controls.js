// The lab's controls: small, in a corner, and big enough for a thumb. The
// markup is the page's (lab/notifications-*.astro); this binds it to the
// lab and shows the state it is given.
import { phaseAt } from "../hero/timing.js";
import { ENDLESS_LAST, SCRUB_STEP, scrubberAt } from "./clock.js";

function part(panel, name) {
  const element = panel.querySelector(`[data-lab="${name}"]`);
  if (!element) throw new Error(`The lab's controls need [data-lab="${name}"].`);
  return element;
}

const press = (button, pressed) => button.setAttribute("aria-pressed", String(pressed));

/** `act` is what the lab does: play(), restart(), scrub(time), set(patch). */
export function mountControls(panel, act) {
  const [play, restart, scrub, time, loop, endless, hide] = ["play", "restart", "scrub", "time", "loop", "endless", "hide"].map((name) => part(panel, name));
  // Each a value of one setting: data-lab-key names it, data-lab-value is
  // the value (a number for the speed).
  const choices = [...panel.querySelectorAll("[data-lab-key]")];
  const valueOf = ({ dataset }) => (dataset.labKey === "speed" ? Number(dataset.labValue) : dataset.labValue);
  play.addEventListener("click", act.play);
  restart.addEventListener("click", act.restart);
  scrub.addEventListener("input", () => act.scrub(Number(scrub.value)));
  loop.addEventListener("click", () => act.set({ loop: loop.getAttribute("aria-pressed") !== "true" }));
  endless.addEventListener("click", () => act.set({ endless: endless.getAttribute("aria-pressed") !== "true" }));
  hide.addEventListener("click", () => {
    const hidden = panel.toggleAttribute("data-collapsed");
    hide.setAttribute("aria-expanded", String(!hidden));
    hide.textContent = hidden ? "Controls" : "Hide";
  });
  for (const choice of choices) {
    choice.addEventListener("click", () => act.set({ [choice.dataset.labKey]: valueOf(choice) }));
  }

  return function show(state, { end, timing }) {
    const span = state.endless ? ENDLESS_LAST : end;
    scrub.step = String(SCRUB_STEP);
    scrub.max = String(span);
    scrub.value = String(scrubberAt(state));
    time.textContent = `${state.time.toFixed(2)}s · ${state.endless ? "endless" : phaseAt(state.time, timing)}`;
    play.textContent = state.playing ? "Pause" : "Play";
    press(loop, state.loop);
    press(endless, state.endless);
    loop.disabled = state.endless;
    for (const choice of choices) {
      press(choice, valueOf(choice) === state[choice.dataset.labKey]);
    }
  };
}
