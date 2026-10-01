// Stand-ins for an element and its Web Animations, for driving the field's
// motion as the clock would.
// As a browser's Animation: a running one past its end is finished, and
// play() on one at its end rewinds it to the start.
export class FakeAnimation {
  constructor(keyframes, options) {
    this.keyframes = keyframes;
    this.options = options;
    this.currentTime = 0;
    this.playbackRate = 1;
    this.state = "running";
  }

  // An endless one (iterations: Infinity) never finishes.
  get end() {
    return this.options.iterations === Infinity ? Infinity : this.options.duration;
  }

  get playState() {
    return this.state === "running" && this.currentTime >= this.end ? "finished" : this.state;
  }

  pause() { this.state = "paused"; }

  play() {
    if (this.currentTime >= this.end) this.currentTime = 0;
    this.state = "running";
  }

  cancel() { this.state = "idle"; }

  // A frame of the compositor's own time.
  run(ms) {
    if (this.state === "running") this.currentTime = Math.min(this.currentTime + ms * this.playbackRate, this.end);
  }
}

export function element() {
  return {
    style: {},
    animations: [],
    animate(keyframes, options) {
      const animation = new FakeAnimation(keyframes, options);
      this.animations.push(animation);
      return animation;
    },
    live() { return this.animations.filter((animation) => animation.playState !== "idle"); },
  };
}
