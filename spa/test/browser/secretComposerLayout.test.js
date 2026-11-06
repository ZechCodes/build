import { expect, it } from "vitest";
import { loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";
import { settled } from "./chatMenuSeed.mjs";

async function mountSecretComposer(page, basePath) {
  await mountLayout(page, '<main id="secret-fixture"></main>', {
    basePath,
    styles: "body{display:grid;place-items:center;min-height:100vh}#secret-fixture{width:min(620px,calc(100vw - 24px))}",
  });
  await loadBrowserModules(page, {
    composer: "src/core/composer.js",
    secretUi: "src/core/secretComposer.js",
    thread: "src/core/thread.js",
  }, basePath);
  await page.evaluate(() => {
    const { composer, secretUi, thread } = window.__layoutModules;
    const host = document.querySelector("#secret-fixture");
    host.innerHTML = `${composer.composerHtml({ inputId: "secret-chat", sendId: "secret-send", hintId: "secret-hint",
      placeholder: "Send a message", attachable: true, secretAttachable: false })}<div id="sent-secret"></div>`;
    let body = "";
    let entries = [];
    const cache = [];
    const service = {
      entries: () => entries,
      stage: async ({ name, scope, ttlSeconds }) => {
        const descriptor = { secret_id: `sec_${cache.length + 1}`, name, ttl_seconds: ttlSeconds,
          grant: scope === "project" ? { scope, project_id: "p1" } : { scope, agent_id: "a1", conversation_id: "c1", generation: "g1" } };
        entries = [...entries, descriptor];
        cache.push(descriptor);
        return descriptor;
      },
      remove: async (id) => { entries = entries.filter((entry) => entry.secret_id !== id); },
    };
    let control;
    const secrets = secretUi.createSecretComposer({ service, onChange: () => control?.refreshSecrets() });
    control = thread.wireThreadComposer(host, {
      ids: { input: "secret-chat", send: "secret-send", hint: "secret-hint" },
      readDraft: () => body,
      writeDraft: (next) => { body = next; },
      readAttachments: () => [],
      writeAttachments: () => {},
      upload: async () => ({}),
      secretComposer: secrets,
      secretAttachable: false,
      onSubmit: async (message) => {
        const ids = entries.map((entry) => entry.secret_id);
        entries = [];
        host.querySelector("#sent-secret").innerHTML = thread.threadHtml({ id: "c1", items: [
          { type: "message", data: { id: "m1", role: "user", body: message, secret_ids: ids } },
        ] }, { secretMetadata: cache });
      },
    });
    window.__secretComposer = { control, cache, service };
  });
}

it("opens the capability-gated attach menu and sends cache-rendered secret metadata", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountSecretComposer(page, basePath);
    expect(await page.locator('[data-action="secret"]').count()).toBe(0);
    await page.evaluate(() => window.__secretComposer.control.setSecretAttachable(true));

    const opener = page.locator(".composer-attach .caret");
    await opener.focus();
    await page.keyboard.press("ArrowDown");
    await settled(page);
    expect(await page.evaluate(() => document.activeElement?.dataset.action)).toBe("files");
    await page.keyboard.press("ArrowDown");
    expect(await page.evaluate(() => document.activeElement?.dataset.action)).toBe("secret");
    await page.keyboard.press("Enter");

    const modal = page.locator(".modal-secret");
    await modal.waitFor({ state: "visible" });
    expect(await modal.getAttribute("aria-labelledby")).toBe("secret-modal-title");
    const value = modal.locator("#secret-value");
    expect(await value.getAttribute("type")).toBe("password");
    expect(await value.getAttribute("autocomplete")).toBe("off");
    await modal.locator("#secret-name").fill("DEPLOY_TOKEN");
    await value.fill("synthetic-browser-value");
    await modal.locator("[data-secret-reveal]").click();
    expect(await value.getAttribute("type")).toBe("text");
    await modal.locator('[name="secret-scope"][value="project"]').check();
    await modal.locator("#secret-ttl").selectOption("3600");
    await modal.locator("[data-secret-submit]").click();
    await page.locator(".modal-secret").waitFor({ state: "detached" });

    expect((await page.locator(".secret-chip-label").textContent()).trim())
      .toBe("DEPLOY_TOKEN · Project · 1 hour after sending");
    expect(await page.evaluate(() => JSON.stringify(window.__secretComposer.cache))).not.toContain("synthetic-browser-value");
    await page.locator("#secret-chat").fill("Deploy this build");
    await page.locator("#secret-send").click();
    expect((await page.locator(".thread-secret-chip").textContent()).trim())
      .toBe("DEPLOY_TOKEN · Project · 1 hour after sending");
    await page.waitForFunction(() => !document.querySelector(".secret-chip-label"));
    expect(await page.locator(".secret-chip-label").count()).toBe(0);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
    expect(overflow).toBeLessThanOrEqual(1);
  }, { width: 390, height: 760 });
});
