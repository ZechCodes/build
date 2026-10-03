import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createWall, notePose, WALL_TIMING, REQUEST_SELECT } from '../../src/lab/hero336/wall.js';

const sizes = [[390, 603], [390, 780], [768, 960], [1280, 836], [1920, 1016], [2560, 1376]];
for (const [width, height] of sizes) {
  test(`wall stays populated and locally still at ${width}×${height}`, () => {
    const wall = createWall({ width, height });
    assert.deepEqual(wall, createWall({ width, height }), 'deterministic placement and timing');
    assert.ok(wall.slots.some(slot => slot.x < slot.width / 2), 'cards reach the left fade');
    assert.ok(wall.slots.some(slot => slot.x > width - slot.width / 2), 'cards reach the right fade');
    for (const time of [0, 0.3, 0.7, 1.2, 1.7, 2.4, 3.1, 3.8, 4.4]) {
      const inside = wall.slots.filter(slot => slot.x > width * .16 && slot.x < width * .84 && slot.y > height * .16 && slot.y < height * .84);
      const readable = inside.filter(slot => slot.turns.some(turn => notePose(turn, time).opacity > .35));
      assert.ok(readable.length / inside.length >= .62, `${time}s: interior stays at least 62% occupied`);
      for (const slot of wall.slots) {
        assert.ok(slot.turns.filter(turn => notePose(turn, time).opacity > .05).length <= 1, 'one notification per position');
      }
    }
    for (const slot of wall.slots) {
      for (const turn of slot.turns.filter(turn => turn.start > 0 && turn.hold > .3)) {
        const first = notePose(turn, turn.start + turn.enter + .05);
        const later = notePose(turn, turn.start + turn.enter + .25);
        assert.deepEqual(first, later, 'notification actually holds instead of scrolling');
        assert.ok(turn.from[0] > 0 && turn.from[0] <= 64, 'every arrival comes from the right');
        assert.equal(turn.from[1], 0, 'entries are horizontal');
      }
      assert.ok(slot.turns.filter(turn => !turn.attention).every(turn => notePose(turn, WALL_TIMING.ripple[1]).opacity === 0), 'routine field clears for the hand-off');
    }
  });
  test(`three requests fit in the solid middle at ${width}×${height}`, () => {
    const wall = createWall({ width, height });
    const positions = wall.requests;
    assert.equal(positions.length, 3);
    for (const position of positions) {
      assert.ok(position.x - position.width / 2 >= 16);
      assert.ok(position.x + position.width / 2 <= width - 16);
      assert.ok(position.y > height * .15 && position.y < height * .8);
    }
    assert.ok(new Set(positions.map(position => position.y)).size === 3);
    positions.forEach((request, index) => {
      const slot = wall.slots[request.slotIndex];
      assert.ok(slot.turns.includes(request.turn), 'selection is an existing wall notification');
      assert.equal(request.x, slot.x);
      assert.equal(request.width, slot.width, 'selected card keeps its wall size');
      assert.equal(notePose(request.turn, REQUEST_SELECT[index] - .6).opacity, 1, 'already present before selection');
      assert.equal(notePose(request.turn, REQUEST_SELECT[index]).opacity, 1, 'selection has no fresh arrival');
    });
  });
}

test('each notification slides in, holds, then fades without drifting across the wall', () => {
  const turn = { start: 1, enter: .32, hold: 1.3, fade: .45, from: [24, 8] };
  assert.equal(notePose(turn, .9).opacity, 0);
  assert.ok(notePose(turn, 1.1).opacity > 0);
  assert.deepEqual(notePose(turn, 1.5), { x: 0, y: 0, opacity: 1 });
  assert.ok(notePose(turn, 2.8).opacity < 1);
  assert.equal(notePose(turn, 3.1).opacity, 0);
});


test('the longer wall turns over at least twice as quickly', () => {
  assert.equal(WALL_TIMING.field[1], 4.8);
  const wall = createWall({ width: 1280, height: 836 });
  const routine = wall.slots.flatMap(slot => slot.turns).filter(turn => !turn.attention);
  const arrivals = routine.filter(turn => turn.start > 0 && turn.start < 4.4);
  assert.ok(arrivals.length / wall.slots.length >= 2.5, 'at least 2.5 fresh arrivals per wall position');
  assert.ok(routine.every(turn => turn.hold <= 1), 'shorter still holds keep the wall busy');
});
