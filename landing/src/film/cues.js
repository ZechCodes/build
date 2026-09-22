// Discrete state from a continuous clock. A scrubbed timeline runs both ways,
// so a display swap is never an event: it is the last cue at or before now.
export function cueAt(cues, time) {
  let current = null;
  for (const [cueTime, value] of cues) {
    if (cueTime <= time) current = value;
    else break;
  }
  return current;
}

// The cues that will fire within `lookahead` units, for preloading.
export function upcomingCues(cues, time, lookahead) {
  return cues
    .filter(([cueTime]) => cueTime > time && cueTime <= time + lookahead)
    .map(([, value]) => value);
}

// A per-device screen resolver that reports only changes, so the stage is
// asked for a display once per swap and not once per frame. A scene may set
// a display on the clock (`overrides`: device → { name, until }); it holds
// until the timeline leaves the scene's act, where the scroll cues resume.
export function createScreenResolver(cuesByDevice, apply, overrides = new Map()) {
  const current = new Map();
  return (time) => {
    for (const [device, cues] of Object.entries(cuesByDevice)) {
      const override = overrides.get(device);
      const wanted = override && time < override.until ? override.name : cueAt(cues, time);
      if (!wanted || current.get(device) === wanted) continue;
      current.set(device, wanted);
      apply(device, wanted);
    }
  };
}
