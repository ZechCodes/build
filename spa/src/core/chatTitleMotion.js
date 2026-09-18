import { prefersReducedMotion } from "./motion.js";

const ERASE_STEP_MS = 18;
const TYPE_STEP_MS = 24;
const MAX_ERASE_STEPS = 12;
const MAX_TYPE_STEPS = 15;

/** Split only at places where a reader sees a character boundary. An engine
 * without Intl.Segmenter gets the final title immediately rather than risking
 * a frame that cuts a joined character in half. */
export function titleGraphemes(text) {
  if (typeof Intl.Segmenter !== "function") return null;
  const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
  return [...segmenter.segment(text)].map(({ segment }) => segment);
}

const hiddenFromView = (element) =>
  !element.isConnected || !!element.closest('[hidden], [aria-hidden="true"], .rail-panel-concealed');

const normalizedTitle = (title) => ({
  text: String(title?.text || ""),
  title: String(title?.title || ""),
  starting: title?.starting === true,
});

function applyMetadata(element, title) {
  element.title = title.title;
  element.classList.toggle("rail-who-starting", title.starting);
}

function applyTitle(element, title) {
  applyMetadata(element, title);
  element.textContent = title.text;
}

const textAtStep = (graphemes, step, steps, revealing) => {
  const progress = revealing ? Math.floor((graphemes.length * step) / steps) : Math.ceil((graphemes.length * (steps - step)) / steps);
  return graphemes.slice(0, progress).join("");
};

function widestFittingPrefix(element, graphemes, available) {
  let lower = 0;
  let upper = graphemes.length;
  while (lower < upper) {
    const candidate = Math.ceil((lower + upper) / 2);
    element.textContent = graphemes.slice(0, candidate).join("");
    if (element.scrollWidth <= available) lower = candidate;
    else upper = candidate - 1;
  }
  return graphemes.slice(0, Math.max(1, lower));
}

function visibleGraphemes(element, graphemes) {
  const available = element.clientWidth;
  const natural = element.scrollWidth;
  if (!available || !natural || natural <= available) return graphemes;
  const fullText = element.textContent;
  const visible = widestFittingPrefix(element, graphemes, available);
  element.textContent = fullText;
  return visible;
}

/**
 * Move one already-mounted title from its old topic to its next one. Its flex
 * item never leaves the header: only textContent changes, so neighbouring
 * controls keep their position and focus while the old words backspace away
 * and the new words type in.
 */
export function createChatTitleMotion(element, {
  schedule = (callback, delay) => setTimeout(callback, delay),
  cancel = (timer) => clearTimeout(timer),
  reducedMotion = prefersReducedMotion,
} = {}) {
  let disposed = false;
  let phase = "idle";
  let timer = null;
  let target = normalizedTitle({ text: element.textContent, title: element.title, starting: element.classList.contains("rail-who-starting") });

  const clearStep = () => {
    if (timer !== null) cancel(timer);
    timer = null;
  };

  const settle = () => {
    clearStep();
    phase = "idle";
    delete element.dataset.titleMotion;
    applyTitle(element, target);
  };

  const runSteps = (graphemes, revealing, done) => {
    const steps = Math.min(graphemes.length, revealing ? MAX_TYPE_STEPS : MAX_ERASE_STEPS);
    const delay = revealing ? TYPE_STEP_MS : ERASE_STEP_MS;
    let step = 0;
    const tick = () => {
      if (disposed) return;
      if (reducedMotion() || hiddenFromView(element)) {
        settle();
        return;
      }
      step += 1;
      element.textContent = textAtStep(graphemes, step, steps, revealing);
      if (step === steps) done();
      else timer = schedule(tick, delay);
    };
    timer = schedule(tick, delay);
  };

  const typeTarget = () => {
    phase = "typing";
    element.dataset.titleMotion = phase;
    applyMetadata(element, target);
    const graphemes = titleGraphemes(target.text);
    if (!graphemes) {
      settle();
      return;
    }
    if (!graphemes.length) {
      settle();
      return;
    }
    // Measure the full text synchronously, then put the empty frame back before
    // the browser paints it. Only its visible prefix types; settle restores the
    // untouched full string so native ellipsis and the hover title remain true.
    element.textContent = target.text;
    const shown = visibleGraphemes(element, graphemes);
    element.textContent = "";
    runSteps(shown, true, settle);
  };

  const eraseShownTitle = () => {
    phase = "erasing";
    element.dataset.titleMotion = phase;
    const graphemes = titleGraphemes(element.textContent);
    if (!graphemes) {
      settle();
      return;
    }
    if (!graphemes.length) {
      typeTarget();
      return;
    }
    runSteps(visibleGraphemes(element, graphemes), false, typeTarget);
  };

  const show = (nextTitle) => {
    if (disposed) return;
    const next = normalizedTitle(nextTitle);
    const sameTarget = next.text === target.text;
    target = next;
    if (reducedMotion() || hiddenFromView(element)) {
      settle();
      return;
    }
    if (phase === "erasing" || (phase === "typing" && sameTarget)) return;
    if (phase === "typing") clearStep();
    if (phase === "idle" && element.textContent === target.text) {
      applyMetadata(element, target);
      return;
    }
    eraseShownTitle();
  };

  return {
    show,
    dispose() {
      disposed = true;
      clearStep();
      phase = "idle";
      delete element.dataset.titleMotion;
    },
  };
}
