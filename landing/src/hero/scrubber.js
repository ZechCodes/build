// Development only (main.js imports this behind import.meta.env.DEV): a
// replay button and a scrubber over the entrance's timeline, with the phase
// under the playhead, for tuning the transition. Touching the scrubber holds
// the entrance at its end instead of letting it clean up.
import { phaseAt } from "./timing.js";

const STYLE = [
  "position:fixed", "left:12px", "bottom:12px", "z-index:50", "display:flex", "gap:10px", "align-items:center",
  "padding:8px 12px", "background:#0a0c0b", "border:1px solid #1e2422", "border-radius:6px",
  "font:12px/1.2 ui-monospace,monospace", "color:#9aa3a0",
].join(";");

function replay() {
  const url = new URL(location.href);
  url.searchParams.set("hero", "play");
  location.replace(url);
}

export function mountScrubber(entrance) {
  const { timeline, timing } = entrance;
  const end = timing.settle[1];
  const panel = document.createElement("div");
  panel.setAttribute("style", STYLE);
  panel.setAttribute("aria-label", "Hero entrance scrubber (development only)");
  const button = Object.assign(document.createElement("button"), { type: "button", textContent: "Replay" });
  const range = Object.assign(document.createElement("input"), { type: "range", min: "0", max: String(end), step: "0.01", value: "0" });
  range.style.width = "320px";
  const label = document.createElement("output");
  panel.append(button, range, label);
  document.body.append(panel);

  const show = () => {
    const time = timeline.time();
    range.value = String(time);
    label.textContent = `${time.toFixed(2)}s · ${phaseAt(time, timing)}`;
  };
  button.addEventListener("click", replay);
  range.addEventListener("input", () => {
    if (entrance.done) return;
    entrance.hold();
    timeline.pause();
    timeline.time(Number(range.value), false);
    show();
  });
  timeline.eventCallback("onUpdate", show);
  show();
}
