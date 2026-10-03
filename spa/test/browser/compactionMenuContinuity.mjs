// Install only after opening motion settles and the slider has focus. Keep
// original references: locators alone would silently follow replacement nodes.
export async function observeCompactionMenu(page) {
  await page.locator('.rail-surface-menu [role="slider"]').focus();
  await page.evaluate(() => {
    const region = document.querySelector(".rail-surface-menu");
    const menu = region.querySelector(".splitmenu");
    const slider = menu.querySelector('[role="slider"]');
    const caret = region.querySelector(".caret");
    const violations = new Set();
    const events = [];
    const originalAnimate = Element.prototype.animate;
    const observeAnimate = function (...args) {
      if (this === menu || menu.contains(this)) events.push("Element.animate");
      return originalAnimate.apply(this, args);
    };
    Element.prototype.animate = observeAnimate;
    const note = (reason) => violations.add(reason);
    const checkStanding = () => {
      if (document.querySelector(".rail-surface-menu .splitmenu") !== menu) note("menu replaced");
      if (document.querySelector('.rail-surface-menu [role="slider"]') !== slider) note("slider replaced");
      if (!menu.isConnected || !slider.isConnected) note("original nodes detached");
      if (document.activeElement !== slider) note("slider lost focus");
      if (menu.hidden || getComputedStyle(menu).display === "none") note("menu hidden");
      if (caret.getAttribute("aria-expanded") !== "true") note("caret collapsed");
    };
    const consume = (records) => {
      for (const record of records) {
        if (record.type === "childList") {
          for (const removed of record.removedNodes) {
            if (removed === menu || removed.contains?.(menu)) note("menu removed");
            if (removed === slider || removed.contains?.(slider)) note("slider removed");
          }
        }
        if (record.target === menu && record.attributeName === "hidden") note("hidden attribute mutated");
        // oldValue catches true -> false -> true in a single observer batch.
        if (record.target === caret && record.attributeName === "aria-expanded"
          && (record.oldValue === "false" || caret.getAttribute("aria-expanded") === "false")) note("aria-expanded became false");
      }
      checkStanding();
    };
    const observer = new MutationObserver(consume);
    observer.observe(document.body, { subtree: true, childList: true, attributes: true,
      attributeOldValue: true, attributeFilter: ["hidden", "aria-expanded", "class", "style"] });
    const focusChanged = (event) => {
      if (event.type === "focusout" && event.target === slider) note("slider focusout");
      if (event.type === "focusin" && event.target !== slider) note("focus moved away from slider");
    };
    const motion = (event) => {
      if (event.target !== menu && !menu.contains(event.target)) return;
      events.push(`${event.type}:${event.animationName || event.propertyName}`);
    };
    const focusEvents = ["focusin", "focusout"];
    const motionEvents = ["animationstart", "animationend", "animationcancel", "transitionrun", "transitionstart", "transitionend", "transitioncancel"];
    for (const name of focusEvents) document.addEventListener(name, focusChanged, true);
    for (const name of motionEvents) document.addEventListener(name, motion, true);
    let frame;
    const sample = () => { checkStanding(); frame = requestAnimationFrame(sample); };
    sample();
    window.__compactionContinuity = {
      read() {
        consume(observer.takeRecords());
        return { violations: [...violations], motionEvents: [...events],
          sameMenu: document.querySelector(".rail-surface-menu .splitmenu") === menu,
          sameSlider: document.querySelector('.rail-surface-menu [role="slider"]') === slider,
          sliderFocused: document.activeElement === slider };
      },
      stop() {
        const result = this.read();
        observer.disconnect();
        cancelAnimationFrame(frame);
        Element.prototype.animate = originalAnimate;
        for (const name of focusEvents) document.removeEventListener(name, focusChanged, true);
        for (const name of motionEvents) document.removeEventListener(name, motion, true);
        return result;
      },
    };
  });
}

export const compactionContinuity = (page, { stop = false } = {}) => page.evaluate((stop) =>
  window.__compactionContinuity[stop ? "stop" : "read"](), stop);

export const uninterruptedMenu = {
  violations: [], motionEvents: [], sameMenu: true, sameSlider: true, sliderFocused: true,
};
