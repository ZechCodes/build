// The carrier has checked the whole-message limit. Keep one staged reset
// separately from the ordinary byte/frame allowance, before and after attach.
const ORDINARY_LIMIT = 1024 * 1024;
const FRAME_LIMIT = 64;

export function createWebSocketFrameQueue() {
  let reset;
  let output = [];
  let bytes = 0;
  return {
    push(frame) {
      if (frame.reset) {
        reset = frame;
        output = [];
        bytes = 0;
        return true;
      }
      bytes += frame.data.length;
      if (output.length >= FRAME_LIMIT || bytes > ORDINARY_LIMIT) return false;
      output.push(frame);
      return true;
    },
    shift() {
      if (reset) {
        const frame = reset;
        reset = undefined;
        return frame;
      }
      const frame = output.shift();
      if (frame) bytes -= frame.data.length;
      return frame;
    },
    clear() { reset = undefined; output = []; bytes = 0; },
  };
}
