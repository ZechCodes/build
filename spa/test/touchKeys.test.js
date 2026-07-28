import { describe, it, expect } from "vitest";
import {
  TOUCH_KEYS,
  applyModifiers,
  createStickyModifiers,
  keySequence,
  keyboardInset,
} from "../src/terminal/touchKeys.js";

describe("createStickyModifiers", () => {
  it("cycles a modifier off → armed → locked → off", () => {
    const sticky = createStickyModifiers();
    expect(sticky.state("ctrl")).toBe("off");
    expect(sticky.press("ctrl")).toBe("armed");
    expect(sticky.press("ctrl")).toBe("locked");
    expect(sticky.press("ctrl")).toBe("off");
  });

  it("reports the pressed set as booleans for applyModifiers", () => {
    const sticky = createStickyModifiers();
    expect(sticky.pressed()).toEqual({ ctrl: false, alt: false });
    sticky.press("ctrl");
    sticky.press("alt");
    sticky.press("alt"); // locked
    expect(sticky.pressed()).toEqual({ ctrl: true, alt: true });
  });

  it("consume() releases armed modifiers but keeps locked ones", () => {
    const sticky = createStickyModifiers();
    sticky.press("ctrl"); // armed
    sticky.press("alt");
    sticky.press("alt"); // locked
    sticky.consume();
    expect(sticky.state("ctrl")).toBe("off");
    expect(sticky.state("alt")).toBe("locked");
  });

  it("consume() reports whether anything changed, so the bar only repaints when it must", () => {
    const sticky = createStickyModifiers();
    expect(sticky.consume()).toBe(false);
    sticky.press("ctrl");
    expect(sticky.consume()).toBe(true);
    expect(sticky.consume()).toBe(false);
  });

  it("notifies onChange for a press, for a consume that released, and never otherwise", () => {
    const changes = [];
    const sticky = createStickyModifiers({ onChange: () => changes.push(sticky.pressed()) });
    sticky.press("ctrl");
    expect(changes).toHaveLength(1);
    sticky.consume();
    expect(changes).toHaveLength(2);
    sticky.consume();
    expect(changes).toHaveLength(2);
  });

  it("watch() adds a listener and returns its unsubscribe", () => {
    const sticky = createStickyModifiers();
    let calls = 0;
    const unwatch = sticky.watch(() => calls++);
    sticky.press("ctrl");
    expect(calls).toBe(1);
    unwatch();
    sticky.press("alt");
    expect(calls).toBe(1);
  });

  it("reset() clears even locked modifiers", () => {
    const sticky = createStickyModifiers();
    sticky.press("alt");
    sticky.press("alt");
    sticky.reset();
    expect(sticky.state("alt")).toBe("off");
  });

  it("ignores a modifier it does not carry", () => {
    const sticky = createStickyModifiers();
    expect(sticky.press("hyper")).toBe("off");
    expect(sticky.state("hyper")).toBe("off");
  });
});

describe("applyModifiers", () => {
  it("passes data through untouched when nothing is pressed", () => {
    expect(applyModifiers("a", { ctrl: false, alt: false })).toBe("a");
    expect(applyModifiers("hello", {})).toBe("hello");
  });

  it("folds ctrl into the C0 control code for letters, either case", () => {
    expect(applyModifiers("c", { ctrl: true })).toBe("\x03");
    expect(applyModifiers("C", { ctrl: true })).toBe("\x03");
    expect(applyModifiers("a", { ctrl: true })).toBe("\x01");
    expect(applyModifiers("z", { ctrl: true })).toBe("\x1a");
  });

  it("folds ctrl into the classic punctuation control codes", () => {
    expect(applyModifiers(" ", { ctrl: true })).toBe("\x00");
    expect(applyModifiers("[", { ctrl: true })).toBe("\x1b");
    expect(applyModifiers("\\", { ctrl: true })).toBe("\x1c");
    expect(applyModifiers("_", { ctrl: true })).toBe("\x1f");
    expect(applyModifiers("?", { ctrl: true })).toBe("\x7f");
  });

  it("prefixes ESC for alt (meta-sends-escape)", () => {
    expect(applyModifiers("b", { alt: true })).toBe("\x1bb");
    expect(applyModifiers(".", { alt: true })).toBe("\x1b.");
  });

  it("combines ctrl and alt as ESC + control code", () => {
    expect(applyModifiers("c", { ctrl: true, alt: true })).toBe("\x1b\x03");
  });

  it("leaves a char with no ctrl mapping alone rather than corrupting it", () => {
    expect(applyModifiers("\r", { ctrl: true })).toBe("\r");
    expect(applyModifiers("é", { ctrl: true })).toBe("é");
  });

  it("rewrites cursor-key escapes with a CSI modifier parameter, in either cursor mode", () => {
    expect(applyModifiers("\x1b[C", { ctrl: true })).toBe("\x1b[1;5C");
    expect(applyModifiers("\x1bOC", { ctrl: true })).toBe("\x1b[1;5C");
    expect(applyModifiers("\x1b[A", { alt: true })).toBe("\x1b[1;3A");
    expect(applyModifiers("\x1b[D", { ctrl: true, alt: true })).toBe("\x1b[1;7D");
  });

  it("rewrites tilde-terminated keys (Home/End/PgUp) with the same modifier parameter", () => {
    expect(applyModifiers("\x1b[5~", { ctrl: true })).toBe("\x1b[5;5~");
  });

  it("leaves multi-character input (a paste) alone", () => {
    expect(applyModifiers("git status", { ctrl: true, alt: true })).toBe("git status");
  });
});

describe("keySequence", () => {
  it("emits the plain bytes for the standalone keys", () => {
    expect(keySequence("esc", {})).toBe("\x1b");
    expect(keySequence("tab", {})).toBe("\t");
    expect(keySequence("shifttab", {})).toBe("\x1b[Z");
    expect(keySequence("interrupt", {})).toBe("\x03");
  });

  it("emits normal-mode cursor keys by default and application-mode under DECCKM", () => {
    expect(keySequence("up", {})).toBe("\x1b[A");
    expect(keySequence("down", {})).toBe("\x1b[B");
    expect(keySequence("right", {})).toBe("\x1b[C");
    expect(keySequence("left", {})).toBe("\x1b[D");
    expect(keySequence("up", { applicationCursor: true })).toBe("\x1bOA");
    expect(keySequence("left", { applicationCursor: true })).toBe("\x1bOD");
  });

  it("applies pressed modifiers to the key it emits", () => {
    expect(keySequence("right", { ctrl: true })).toBe("\x1b[1;5C");
    expect(keySequence("right", { ctrl: true, applicationCursor: true })).toBe("\x1b[1;5C");
    expect(keySequence("tab", { alt: true })).toBe("\x1b\t");
  });

  it("sends the interrupt literally — ^C is already the modified key", () => {
    expect(keySequence("interrupt", { ctrl: true, alt: true })).toBe("\x03");
  });

  it("returns null for a key it does not know and for the modifier keys themselves", () => {
    expect(keySequence("meta", {})).toBe(null);
    expect(keySequence("ctrl", {})).toBe(null);
    expect(keySequence("alt", {})).toBe(null);
  });
});

describe("TOUCH_KEYS", () => {
  it("leads with the keys a soft keyboard cannot type and carries both sticky modifiers", () => {
    const ids = TOUCH_KEYS.map((key) => key.id);
    expect(ids.slice(0, 3)).toEqual(["esc", "tab", "shifttab"]);
    expect(TOUCH_KEYS.filter((key) => key.modifier).map((key) => key.modifier)).toEqual(["ctrl", "alt"]);
    expect(ids).toContain("up");
    expect(ids).toContain("interrupt");
  });

  it("labels every key", () => {
    for (const key of TOUCH_KEYS) expect(key.label).toBeTruthy();
  });
});

describe("keyboardInset", () => {
  it("is zero when the layout viewport already shrank for the keyboard", () => {
    expect(keyboardInset({ innerHeight: 500, visualViewport: { height: 500, offsetTop: 0 } })).toBe(0);
  });

  it("is the covered height when the visual viewport shrank but the layout viewport did not", () => {
    expect(keyboardInset({ innerHeight: 800, visualViewport: { height: 460, offsetTop: 0 } })).toBe(340);
  });

  it("discounts a scrolled-out visual viewport rather than reporting phantom keyboard", () => {
    expect(keyboardInset({ innerHeight: 800, visualViewport: { height: 700, offsetTop: 100 } })).toBe(0);
  });

  it("is zero without visualViewport support", () => {
    expect(keyboardInset({ innerHeight: 800 })).toBe(0);
  });
});
