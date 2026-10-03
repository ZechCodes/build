import assert from "node:assert/strict";
import { test } from "node:test";
import { createWallMotion } from "../../src/hero/wall-motion.js";
import { WALL_TIMING } from "../../src/hero/wall.js";

function fieldFixture() {
  const animations = [];
  const document = {
    createElement(tag) {
      const node = {
        tag, className: "", dataset: {}, style: {}, children: [], parentElement: null,
        append(...children) {
          for (const child of children) {
            child.remove();
            child.parentElement = this;
            this.children.push(child);
          }
        },
        remove() {
          if (!this.parentElement) return;
          const siblings = this.parentElement.children;
          siblings.splice(siblings.indexOf(this), 1);
          this.parentElement = null;
        },
        replaceChildren(...children) {
          for (const child of this.children) child.parentElement = null;
          this.children = [];
          this.append(...children);
        },
        querySelector(selector) {
          const className = selector.slice(1);
          return this.children.find(child => child.className.split(" ").includes(className))
            ?? this.children.map(child => child.querySelector(selector)).find(Boolean);
        },
        animate() {
          const animation = {
            effect: { target: this }, playbackRate: 1, currentTime: 0,
            playState: "running", cancelCount: 0, seekCount: 0,
            pause() { this.playState = "paused"; this.seekCount += 1; },
            play() { this.playState = "running"; this.seekCount += 1; },
            cancel() { this.playState = "idle"; this.cancelCount += 1; },
          };
          animations.push(animation);
          return animation;
        },
      };
      node.classList = { add(name) { node.className += ` ${name}`; } };
      return node;
    },
  };
  const field = document.createElement("div");
  const routine = document.createElement("div");
  routine.dataset.wallRoutine = "";
  field.append(routine);
  field.ownerDocument = document;
  field.getBoundingClientRect = () => ({ width: 1280, height: 836 });
  field.querySelector = (selector) => selector === "[data-wall-routine]" ? routine : null;
  return { field, routine, animations };
}

test("the wall releases compositor effects, cards and request handles at disposal", () => {
  const { field, routine, animations } = fieldFixture();
  const motion = createWallMotion(field);
  const savedUpdate = motion.update;
  const savedRequests = motion.requests;
  assert.ok(animations.length > 100, "the wall owns many compositor animations");
  assert.equal(savedRequests.length, 3);
  savedRequests[0].layer(true);
  motion.update(WALL_TIMING.ripple[1], false);

  motion.dispose();
  assert.equal(savedRequests.length, 0, "the retained motion handle no longer holds request nodes");
  assert.equal(routine.children.length, 0, "no routine nodes remain under a saved field handle");
  assert.equal(field.children.length, 0, "flying request nodes are detached too");
  assert.ok(animations.every(animation => animation.playState === "idle" && animation.effect === null));

  const before = animations.map(animation => [animation.cancelCount, animation.seekCount]);
  savedUpdate(0, true);
  motion.dispose();
  assert.deepEqual(animations.map(animation => [animation.cancelCount, animation.seekCount]), before,
    "seeking or disposing again cannot revive a released animation");
});
