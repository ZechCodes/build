// The page's entry point. Bundled by Astro into /landing/generated/_astro/ as
// an external module, because the app's CSP (script-src 'self'
// 'wasm-unsafe-eval') drops inline script.
//
// The document is complete and readable without it. This starts the film,
// the pinned, scroll-driven version with the three.js device stage, for the
// visitors film-boot.js chose. Phones, reduced motion, save-data and
// machines without WebGL keep the document.
document.documentElement.dataset.js = "ready";

// film-boot.js, in the head, already chose before the first paint and set
// data-mode="film"; this only starts what it chose. ?film=force skips the
// frame budget, for automated checks on software renderers. If the boot's
// deadline gave the page back to the document first, the film stays out.
const root = document.documentElement;
const params = new URLSearchParams(location.search);
const requested = params.get("film");

// The bar's two variants, for comparing: ?nav=hero, or the persistent default.
const nav = document.querySelector(".site-nav");
if (nav && params.get("nav") === "hero") nav.dataset.nav = "hero";

function backToDocument(error) {
  console.warn("The film could not start; the document stands.", error);
  delete root.dataset.mode;
  delete root.dataset.stage;
  // A call to action pressed while the film was pending still goes where it
  // pointed, now in the document.
  if (/^#act-\d+$/.test(location.hash)) document.getElementById(location.hash.slice(1))?.scrollIntoView();
}

if (root.dataset.mode === "film") {
  import("../film/film.js")
    .then(({ startFilm }) => {
      if (root.dataset.mode !== "film") return;
      root.dataset.stage = "starting";
      startFilm({ ignoreFrameBudget: requested === "force" });
    })
    .catch(backToDocument);
}
