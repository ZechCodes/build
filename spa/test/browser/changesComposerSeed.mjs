import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";

export const COMPOSER_VARIANTS = [
  { id: "comment", label: "Comment", offer: { commentable: true } },
  { id: "commit", label: "Commit", offer: { uncommitted: true } },
  { id: "split", label: "Comment or commit", offer: { commentable: true, uncommitted: true } },
];
export const THREE_LINES = "Keep the action quiet.\nThe field carries the review.\nReady for another look.";

export async function mountChangesComposers(page, basePath) {
  const rows = COMPOSER_VARIANTS.map(({ id, label }) =>
    `<section><p class="hint">${label}</p><div id="${id}"></div></section>`).join("");
  await mountLayout(page, `<main id="changes-evidence"><h1>Changes</h1>${rows}</main>
    <div id="references"><div id="chat-send"></div>
      <div id="verbar"><button class="btn">Comment</button></div>
      <div id="shared"></div></div>`, {
    basePath,
    styles: "#changes-evidence { width:100%; max-width:900px; padding:24px; } #changes-evidence h1 { font-size:18px; } #changes-evidence section { margin:28px 0; } #references { margin:24px; display:flex; gap:16px; }",
  });
  await loadBrowserModules(page, { changes: "src/core/changesComposer.js", split: "src/core/splitButton.js", composer: "src/core/composer.js" }, basePath);
  await page.evaluate((variants) => {
    window.__runs = [];
    document.getElementById("chat-send").innerHTML = window.__layoutModules.composer.sendControlHtml({ sendId: "reference-send" });
    for (const { id, offer } of variants) {
      let draft = "";
      window.__layoutModules.changes.mountChangesComposer(document.getElementById(id), {
        offers: () => offer,
        run: async (verb, text) => { window.__runs.push({ variant: id, verb, text }); },
        readDraft: () => draft,
        writeDraft: (text) => { draft = text; },
      });
    }
    window.__layoutModules.split.mountSplitButton(document.getElementById("shared"), {
      options: [{ id: "other", label: "Other action" }, { id: "alternate", label: "Alternate" }],
      run: async () => {},
    });
  }, COMPOSER_VARIANTS);
}

export async function settleChangesTheme(page, theme) {
  await page.evaluate((theme) => {
    document.documentElement.dataset.theme = theme;
    for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
  }, theme);
}
