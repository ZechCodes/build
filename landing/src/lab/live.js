// Variant A, the live hero's flood, on the lab's clock. flood.js moves the
// field exactly as it does on the home page; there is no laptop here, so
// each request flies to where its Needs you row would be (stage.rows).
// liveEndless is its endless form: the lanes drift on and never resolve.
import { ATTENTION } from "../hero/field.js";
import { createFieldMotion, seek } from "../hero/flood.js";
import { power2InOut, requestFlight, rippleReach } from "../hero/timing.js";
import { laneLoop, pillLoop } from "./loop.js";

const clamp = (value) => Math.min(1, Math.max(0, value));

function setStyle(element, { transform, opacity }) {
  element.style.transform = transform;
  element.style.opacity = opacity === "" ? "" : String(opacity);
  element.style.visibility = opacity === 0 ? "hidden" : "";
}

// A request's flight, from its take-off to its row. Before take-off the
// flood has it, so going back there hands it back as the flood left it.
function requestFlightMotion(pill, row, { motion, timing, takeOff }) {
  const base = motion.offsetAt(pill, takeOff);
  const { dx, dy } = motion.pathOf(pill).at(takeOff);
  const flight = { base, from: [pill.x + dx, pill.y + dy], to: row };
  let flying = false;
  return {
    handBack(time) {
      if (!flying || time >= takeOff) return;
      flying = false;
      setStyle(pill.element, { transform: `translate(${base[0]}px, ${base[1]}px)`, opacity: "" });
    },
    fly(time) {
      if (time < takeOff) return;
      flying = true;
      setStyle(pill.element, requestFlight(power2InOut(clamp((time - takeOff) / timing.flight)), flight));
    },
  };
}

export function liveFlood({ field, stage }) {
  const { timing, origin } = stage;
  const ripple = { origin, reach: rippleReach(origin, field), timing, start: 0, nudge: stage.nudge };
  const takeOffs = ATTENTION.map((entry, index) => timing.landings[index] - timing.flight);
  const motion = createFieldMotion({ field, ripple, flights: new Map(ATTENTION.map((entry, index) => [entry.id, takeOffs[index]])) });
  const pills = field.lanes.flatMap((lane) => lane.pills);
  const requests = ATTENTION.flatMap((entry, index) => {
    const pill = pills.find((candidate) => candidate.attention === entry.id && motion.pathOf(candidate));
    return pill ? [requestFlightMotion(pill, stage.rows[index], { motion, timing, takeOff: takeOffs[index] })] : [];
  });
  return {
    end: timing.settle[1],
    // The flood styles a pill when its part changes, the flight every frame
    // after take-off: a request handed back first, flown last.
    update(time, playing, rate) {
      for (const request of requests) request.handBack(time);
      motion.update(time, playing, rate);
      for (const request of requests) request.fly(time);
    },
  };
}

export function liveEndless({ field }) {
  const loops = field.lanes.filter((lane) => lane.shown && lane.speed !== 0).flatMap((lane) => {
    const pills = lane.pills.filter((pill) => pill.shown);
    const loop = laneLoop({ pills }, field.width);
    return pills.map((pill) => {
      const { from, to, duration, iterationStart } = pillLoop(pill, lane.speed, loop);
      return pill.element.animate(
        [{ transform: `translateX(${from}px)` }, { transform: `translateX(${to}px)` }],
        { duration, iterationStart, iterations: Infinity, easing: "linear" },
      );
    });
  });
  return {
    end: Infinity,
    update(time, playing, rate) {
      for (const animation of loops) seek(animation, time * 1000, playing, Infinity, rate);
    },
  };
}
