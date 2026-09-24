// The issue page's comment box, both ways: with the paperclip against a 1.8
// bridge, and the plain box an older one gets.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
const SPA = fileURLToPath(new URL("../../spa", import.meta.url));
const sheet = (n) => readFileSync(`${SPA}/src/${n}`, "utf8");

const page = (theme, width) => `<!doctype html><html lang="en" data-theme="${theme}"><head><meta charset="utf-8">
<style>${sheet("styles.css").replace(/@import[^;]+;/g, "")}</style>
<style>${sheet("styles/issues.css")}</style>
<style>html{background:var(--bg)}body{margin:0;width:${width}px}
.surface{background:var(--panel);border-radius:12px;margin:10px;padding:16px}
h4{color:var(--dim);font:600 11px/1 var(--sans);letter-spacing:.06em;text-transform:uppercase;margin:0 0 10px}</style>
</head><body>
<div class="surface"><h4>1.8 bridge — takes files</h4><div id="with"></div></div>
<div class="surface"><h4>older bridge — the box it always had</h4><div id="without"></div></div>
<script type="module">
import { composerHtml } from "./src/core/trackerIssueRender.js";
import { mountComposerAttachments } from "./src/core/composer.js";
document.querySelector("#with").innerHTML = composerHtml("Reproduced on the tablet.", false, true, true);
document.querySelector("#without").innerHTML = composerHtml("Reproduced on the tablet.", false, false, false);
const tray = mountComposerAttachments(document.querySelector("#with .issue-composer"), {
  ids: { input: "issue-comment" },
  upload: async (file) => ({ name: file.name, path: "/store/abc-" + file.name, mime: file.type, size: file.size }),
});
tray.addFiles([new File([new Uint8Array(4096)], "tablet.png", { type: "image/png" })]);
setTimeout(() => { window.__ready = true; }, 400);
</script></body></html>`;

writeFileSync(`${SPA}/__comment-900.html`, page("dark", 900));
console.log("written");
