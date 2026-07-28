// Pre-paint theme stamp. Loaded parser-blocking from <head>, ahead of the
// (deferred) module bundle, so the very first frame is already the reader's
// theme instead of a white flash they then watch turn dark.
//
// It duplicates a few lines of src/core/theme.js on purpose: that module is
// inside the bundle, and the bundle is exactly what this has to beat. Keep the
// key, the attribute, and the two chrome colours in step with it — theme.js
// takes ownership of all three the moment it loads.
(function () {
  var LIGHT = "#fafafa";
  var DARK = "#15171c";
  var dark = false;
  try {
    var preference = localStorage.getItem("build.theme");
    dark =
      preference === "dark" ||
      ((preference === "system" || preference === null) &&
        window.matchMedia("(prefers-color-scheme: dark)").matches);
  } catch (e) {
    /* private mode or no matchMedia: fall through to light, which is the default */
  }
  document.documentElement.setAttribute("data-theme", dark ? "dark" : "light");
  var meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", dark ? DARK : LIGHT);
})();
