// Served as a bundled same-origin external script under the real production
// policy: the fixture does not rely on an inline-script exception or nonce.
import { markdownHtml } from "../../../src/core/markdown.js";
import { installMarkdownImages } from "../../../src/core/markdownImages.js";

window.__cspViolations = [];
document.addEventListener("securitypolicyviolation", (event) => {
  window.__cspViolations.push({ directive: event.effectiveDirective, blocked: event.blockedURI });
});
document.querySelector(".markdown").innerHTML = markdownHtml("![Production CSP diagram](https://images.fixture.test/diagram.svg)");
window.__disposeImages = installMarkdownImages(document);
window.__cspFixtureReady = true;
