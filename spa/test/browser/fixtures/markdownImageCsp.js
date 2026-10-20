// Served as a bundled same-origin external script under the real production
// policy: the fixture does not rely on an inline-script exception or nonce.
import { markdownHtml } from "../../../src/core/markdown.js";
import { installMarkdownImages } from "../../../src/core/markdownImages.js";

window.__cspViolations = [];
document.addEventListener("securitypolicyviolation", (event) => {
  window.__cspViolations.push({ directive: event.effectiveDirective, blocked: event.blockedURI });
});
const query = new URLSearchParams(location.search);
const source = query.has("redirect")
  ? new URL("/app/downloads/SHA256SUMS", location.href).href
  : "https://images.fixture.test/diagram.svg";
const image = `![Production CSP diagram](${source})`;
document.querySelector(".markdown").innerHTML = markdownHtml(query.has("linked") ? `[${image}](https://example.com/docs)` : image);
window.__disposeImages = installMarkdownImages(document);
window.__cspFixtureReady = true;
