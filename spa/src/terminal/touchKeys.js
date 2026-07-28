// The key vocabulary a soft keyboard cannot reach. On a phone there is no Esc,
// no Tab, no Ctrl and no Alt — which is most of how anyone drives a shell or a
// coding agent — so the terminal pane grows a bar of them (keyBar.js). This
// module is the rules alone: what each key emits, how the sticky modifiers
// latch, and how a modifier folds into a keystroke. DOM-free, so the encodings
// are unit-testable in vitest's node environment.

/** Off → armed (applies to the next key) → locked (applies until pressed off). */
const NEXT_STATE = { off: "armed", armed: "locked", locked: "off" };

const MODIFIER_NAMES = ["ctrl", "alt"];

/**
 * createStickyModifiers({ onChange }) → the latch shared by the bar (which
 * paints it) and the pane (which folds it into every keystroke the soft
 * keyboard produces). Listeners — the constructor's onChange, plus any added
 * with watch() — fire only on a real transition, so a bar that repaints on
 * them never repaints for nothing.
 */
export function createStickyModifiers({ onChange } = {}) {
  const states = { ctrl: "off", alt: "off" };
  const listeners = new Set();
  if (onChange) listeners.add(onChange);
  const announce = () => {
    for (const listener of [...listeners]) listener();
  };
  return {
    state: (name) => states[name] || "off",
    /** Subscribe to transitions; returns the unsubscribe. */
    watch(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    /** The set as booleans, shaped for applyModifiers. */
    pressed: () => ({ ctrl: states.ctrl !== "off", alt: states.alt !== "off" }),
    /** Advance one modifier through the cycle; returns its new state. */
    press(name) {
      if (!(name in states)) return "off";
      states[name] = NEXT_STATE[states[name]];
      announce();
      return states[name];
    },
    /** A keystroke used the modifiers: armed ones release, locked ones stay.
     *  Returns whether anything actually released. */
    consume() {
      const released = MODIFIER_NAMES.filter((name) => states[name] === "armed");
      if (!released.length) return false;
      for (const name of released) states[name] = "off";
      announce();
      return true;
    },
    /** Drop everything, locks included (the bar is going away). */
    reset() {
      const changed = MODIFIER_NAMES.some((name) => states[name] !== "off");
      for (const name of MODIFIER_NAMES) states[name] = "off";
      if (changed) announce();
      return changed;
    },
  };
}

/** Ctrl over the punctuation that has a C0 code but no letter to derive it from. */
const CTRL_PUNCTUATION = {
  " ": 0x00, "@": 0x00, "[": 0x1b, "\\": 0x1c, "]": 0x1d, "^": 0x1e, "_": 0x1f, "?": 0x7f,
};

/** The C0 code for ctrl+char, or null when the pairing has no encoding. */
function controlCode(char) {
  if (char >= "a" && char <= "z") return String.fromCharCode(char.charCodeAt(0) - 0x60);
  if (char >= "A" && char <= "Z") return String.fromCharCode(char.charCodeAt(0) - 0x40);
  const code = CTRL_PUNCTUATION[char];
  return code === undefined ? null : String.fromCharCode(code);
}

/** The xterm modifier parameter: 1 + a bit per held modifier. */
function modifierParameter({ ctrl, alt }) {
  return 1 + (alt ? 2 : 0) + (ctrl ? 4 : 0);
}

// A cursor/editing key in either normal (CSI) or application (SS3) form, and the
// CSI n~ family (Home/End/PgUp/PgDn/Ins/Del). Only unmodified forms match —
// a sequence that already carries a parameter is left as its sender wrote it.
const BARE_FINAL_KEY = /^\x1b(?:\[|O)([A-DHFPQRS])$/;
const BARE_TILDE_KEY = /^\x1b\[(\d+)~$/;

/** Rewrite an escape sequence to carry the modifiers, or null if it is not one. */
function modifiedEscapeSequence(data, modifiers) {
  const parameter = modifierParameter(modifiers);
  const final = BARE_FINAL_KEY.exec(data);
  if (final) return `\x1b[1;${parameter}${final[1]}`;
  const tilde = BARE_TILDE_KEY.exec(data);
  if (tilde) return `\x1b[${tilde[1]};${parameter}~`;
  return null;
}

/**
 * applyModifiers(data, { ctrl, alt }) → the bytes that keystroke becomes with
 * the sticky modifiers folded in. Ctrl collapses a character to its C0 code,
 * alt prefixes ESC (meta-sends-escape), and an escape sequence instead grows an
 * xterm modifier parameter. Anything with no encoding — a paste, a character
 * with no control code — passes through untouched rather than being corrupted.
 */
export function applyModifiers(data, { ctrl = false, alt = false } = {}) {
  if (!ctrl && !alt) return data;
  const sequence = modifiedEscapeSequence(data, { ctrl, alt });
  if (sequence) return sequence;
  if (data.length !== 1) return data;
  let char = data;
  if (ctrl) {
    const code = controlCode(char);
    if (code === null) return alt ? `\x1b${char}` : char;
    char = code;
  }
  return alt ? `\x1b${char}` : char;
}

/** The final byte of each cursor key, shared by the CSI and SS3 forms. */
const CURSOR_FINALS = { up: "A", down: "B", right: "C", left: "D" };

/** Keys whose bytes never vary — no cursor mode, no modifier folding for ^C. */
const LITERAL_KEYS = { esc: "\x1b", tab: "\t", shifttab: "\x1b[Z" };

/**
 * The bar, in row order: first the keys a phone keyboard simply does not have,
 * then the two sticky modifiers that make the rest of the keyboard usable, then
 * navigation, then the one chord worth its own button — interrupting a runaway
 * process is the thing you most need when the keyboard is not even up.
 */
export const TOUCH_KEYS = [
  { id: "esc", label: "Esc", title: "Escape" },
  { id: "tab", label: "Tab", title: "Tab" },
  { id: "shifttab", label: "⇧Tab", title: "Shift-Tab" },
  { id: "ctrl", label: "Ctrl", title: "Control — tap to arm, tap again to lock", modifier: "ctrl" },
  { id: "alt", label: "Alt", title: "Alt/Option — tap to arm, tap again to lock", modifier: "alt" },
  { id: "left", label: "←", title: "Left" },
  { id: "up", label: "↑", title: "Up" },
  { id: "down", label: "↓", title: "Down" },
  { id: "right", label: "→", title: "Right" },
  { id: "interrupt", label: "^C", title: "Interrupt (Ctrl-C)" },
];

/**
 * keySequence(id, { ctrl, alt, applicationCursor }) → the bytes to send for one
 * bar key, or null when the id names no sendable key (an unknown id, or a
 * modifier button — those latch rather than send). `applicationCursor` is the
 * terminal's DECCKM state: an app that set it expects SS3 cursor keys, and gets
 * arrows that do nothing if it is ignored.
 */
export function keySequence(id, { ctrl = false, alt = false, applicationCursor = false } = {}) {
  if (id === "interrupt") return "\x03"; // already a chord; modifiers would only garble it
  const literal = LITERAL_KEYS[id];
  if (literal) return applyModifiers(literal, { ctrl, alt });
  const final = CURSOR_FINALS[id];
  if (!final) return null;
  // Modified cursor keys are CSI-with-parameter in both modes — SS3 has nowhere
  // to put the parameter — so the bare form only picks up DECCKM.
  if (ctrl || alt) return applyModifiers(`\x1b[${final}`, { ctrl, alt });
  return `\x1b${applicationCursor ? "O" : "["}${final}`;
}

/**
 * How much of the layout viewport an on-screen keyboard is covering. Where the
 * browser shrinks the layout viewport for the keyboard this is 0 and the bar
 * needs no help; where it does not (iOS Safari), the pane's bottom — and so the
 * bar — sits underneath the keyboard, and this is how far to lift it.
 */
export function keyboardInset({ innerHeight, visualViewport } = {}) {
  if (!visualViewport || !(innerHeight > 0)) return 0;
  return Math.max(0, Math.round(innerHeight - visualViewport.height - visualViewport.offsetTop));
}
