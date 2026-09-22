// The page's own entry point, and later the choreography's. Bundled by Astro
// into /landing/generated/_astro/ as an external module, because the app's CSP
// (script-src 'self' 'wasm-unsafe-eval') drops inline script.
//
// Today it makes one statement the stylesheet can act on: JavaScript ran. The
// document is complete and readable without it.
document.documentElement.dataset.js = "ready";
