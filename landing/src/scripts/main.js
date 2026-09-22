// The page's entry point. Bundled by Astro into /landing/generated/_astro/ as
// an external module, because the app's CSP (script-src 'self'
// 'wasm-unsafe-eval') drops inline script.
//
// The document is complete and readable without it. What this decides is
// whether the visitor also gets the film: the pinned, scroll-driven version
// with the three.js device stage. Phones, reduced motion, save-data and
// machines without WebGL keep the document.
import { stageMode } from "../stage/fallback.js";

document.documentElement.dataset.js = "ready";

function hasWebgl() {
  try {
    const canvas = document.createElement("canvas");
    return Boolean(canvas.getContext("webgl2"));
  } catch {
    return false;
  }
}

// ?film=0 keeps the document on any machine; ?film=force skips the frame
// budget, for automated checks on software renderers.
const requested = new URLSearchParams(location.search).get("film");
const signals = {
  reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
  viewportWidth: innerWidth,
  saveData: Boolean(navigator.connection?.saveData),
  webgl: hasWebgl(),
};

if (requested !== "0" && stageMode(signals) === "stage") {
  import("../film/film.js")
    .then(({ startFilm }) => startFilm({ ignoreFrameBudget: requested === "force" }))
    .catch((error) => {
      console.warn("The film could not start; the document stands.", error);
      delete document.documentElement.dataset.mode;
    });
}
