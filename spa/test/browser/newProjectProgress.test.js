import { expect, it } from "vitest";
import { withLayoutPage } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { failure, holdProjectCache, mountNewProject, remote, resolveProject } from "./newProjectProgressSeed.mjs";

async function assertProgress(page, count) {
  expect(await page.locator("#sheet input, #sheet select, #nrdo, #nrcancel, #nrform").count()).toBe(0);
  expect(await page.locator("[data-source-state='in_progress']").count()).toBe(count);
  expect(await page.locator("#nrprogress h3").textContent()).toBe("Zimmerman Web Works");
  expect(await page.locator("#nrprogress-status").getAttribute("role")).toBe("status");
  expect(await page.locator("#nrprogress-status").getAttribute("aria-live")).toBe("polite");
  expect(await page.locator("#nrprogress").evaluate((node) => node.contains(document.activeElement))).toBe(true);
  expect(await page.locator("#nrprogress").textContent()).toContain(`Cloning ${remote} as zimmerman-web-works`);
}

const dismissalRoutes = {
  Escape: (page) => page.keyboard.press("Escape"),
  scrim: (page) => page.locator("#scrim").click({ position: { x: 1, y: 1 } }),
  Cancel: (page) => page.locator("#nrcancel").click(),
};
it("announces pending and settled states without a busy live region", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountNewProject(page, basePath);
    await page.locator("#nrdo").click();
    expect(await page.locator('#nrprogress-status').evaluate((node) => node.closest('[aria-busy="true"]') !== null)).toBe(false);
    expect(await page.locator('#sheet [role="status"][aria-live="polite"]').count()).toBe(1);
    await page.evaluate((message) => window.__rejectProject(message), failure);
    await page.waitForSelector("[data-source-state='failed']");
    expect(await page.locator('#nrprogress-status').evaluate((node) => node.closest('[aria-busy="true"]') !== null)).toBe(false);
    expect(await page.locator('#sheet [role="status"][aria-live="polite"]').count()).toBe(1);
  }, { plugins: [deviceShim, holdProjectCache] });
});

for (const { label, width, height } of [
  { label: "phone", width: 390, height: 844 },
  { label: "desktop", width: 1280, height: 800 },
]) {
  for (const { name, message, fields } of [
    { name: "delivery timeout", message: "project.create timed out", fields: { error_code: undefined, timedOut: true, uncertain: true, deadline: "path" } },
    { name: "answer timeout", message: "project.create timed out", fields: { error_code: undefined, timedOut: true, uncertain: true, deadline: "answer" } },
    { name: "session loss after send", message: "your device went offline", fields: { error_code: undefined, uncertain: true } },
    { name: "carrier loss", message: "the relay socket closed", fields: { error_code: undefined } },
  ]) {
    it(`shows an unknown creation outcome after ${name} on ${label}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountNewProject(page, basePath);
        await page.locator("#nrdo").click();
        await page.evaluate(({ message, fields }) => window.__rejectProject(message, fields), { message, fields });
        // Cancel appears after any settlement in both the old and new views.
        await page.waitForSelector("#nrcancel");
        expect(await page.locator("[data-source-state='failed'], #nrback, #nrdo").count()).toBe(0);
        expect(await page.locator("[data-source-state='unknown']").count()).toBe(1);
        expect(await page.locator("#nrprogress").textContent()).toContain("request was lost");
        expect(await page.locator("#nrprogress").textContent()).toContain("may still be being created");
        expect(await page.locator("#nrprogress").textContent()).toContain("Projects");
        expect(await page.locator("#nrprogress").textContent()).toContain("The Beast");
        expect(await page.locator("#nrprogress").textContent()).not.toContain("try again");
        expect(await page.locator("#sheet").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
        await page.locator("#nrcancel").click();
        expect(await page.locator("#scrim").getAttribute("class")).not.toContain("show");
      }, { width, height, plugins: [deviceShim, holdProjectCache] });
    });
  }
  it(`keeps coded and uncoded bridge refusals definite with Back on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountNewProject(page, basePath);
      for (const { message, fields } of [
        { message: failure, fields: { error_code: "internal" } },
        { message: failure, fields: { error_code: undefined } },
        { message: "Device offline", fields: { error_code: "unavailable" } },
      ]) {
        await page.locator("#nrdo").click();
        await page.evaluate(({ message, fields }) => window.__rejectProject(message, fields), { message, fields });
        await page.waitForSelector("#nrcancel");
        expect(await page.locator("[data-source-state='failed']").count()).toBe(1);
        expect(await page.locator("[data-source-state='unknown']").count()).toBe(0);
        expect(await page.locator("[data-source-state='failed']").textContent()).toContain(message);
        expect(await page.locator("#nrback").count()).toBe(1);
        await page.locator("#nrback").click();
        expect(await page.locator("[data-source-value]").inputValue()).toBe(remote);
      }
    }, { width, height, plugins: [deviceShim, holdProjectCache] });
  });
}
it("keeps successful source states when the created project cannot be cached", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountNewProject(page, basePath);
    await page.evaluate(() => { window.__projectCacheError = "Local cache unavailable"; });
    await page.locator("#nrdo").click();
    await resolveProject(page);
    await page.waitForSelector("[data-source-state='done']");
    const sources = await page.locator(".nr-progress-sources").elementHandle();
    await page.evaluate(() => window.__releaseProjectCache());
    await page.waitForSelector("#nropenerr");
    expect(await sources.evaluate((node) => node.isConnected && node === document.querySelector(".nr-progress-sources"))).toBe(true);
    expect(await page.locator("#nrcancel").textContent()).toBe("Close");
    expect(await page.locator("#nropenerr").textContent()).toBe("Local cache unavailable");
    expect(await page.locator("[data-source-state='done']").count()).toBe(1);
    expect(await page.locator("[data-source-state='failed'], #nrback, #nrdo").count()).toBe(0);
    expect(await page.locator("#nrprogress").textContent()).toContain("Project created. Could not open it.");
    await page.waitForFunction(async () =>
      (await window.__layoutModules.store.readUiRecord(window.__projectDraftAddress)).value.sources.length === 0);
    await page.locator("#nrcancel").click();
    expect(await page.locator("#scrim").getAttribute("class")).not.toContain("show");
  }, { plugins: [deviceShim, holdProjectCache] });
});
it("does not navigate or hide a replacement sheet when dismissed during the draft-clear write", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountNewProject(page, basePath);
    await page.evaluate(() => {
      window.__draftClearReady = new Promise((resolve) => { window.__releaseDraftClear = resolve; });
      window.__releaseProjectCache();
    });
    await page.locator("#nrdo").click();
    await resolveProject(page);
    await page.waitForSelector("[data-source-state='done']");
    await page.locator("#nrcancel").click();
    await page.evaluate(() => {
      document.querySelector("#sheet").innerHTML = '<h3 id="replacement">Replacement sheet</h3>';
      document.querySelector("#scrim").classList.add("show");
      window.__releaseDraftClear();
    });
    await page.waitForFunction(async () =>
      (await window.__layoutModules.store.readUiRecord(window.__projectDraftAddress)).value.sources.length === 0);
    // The cache read is the last await before navigation; flush that same store.
    await page.evaluate(() => window.__layoutModules.cache.readCached(window.__layoutModules.settings.projectSettingsAddress("beast", "zimmerman")));
    expect(await page.evaluate(() => window.__openedProject)).toBeUndefined();
    expect(await page.locator("#scrim").getAttribute("class")).toContain("show");
    expect(await page.locator("#replacement").count()).toBe(1);
  }, { plugins: [deviceShim, holdProjectCache] });
});
for (const [route, dismiss] of Object.entries(dismissalRoutes)) {
  for (const success of [false, true]) {
    it(`releases ${route} dismissal when creation ${success ? "succeeds" : "times out"}`, async () => {
      await withLayoutPage(async ({ page, basePath }) => {
        await mountNewProject(page, basePath);
        await page.locator("#nrdo").click();
        if (success) await resolveProject(page);
        else await page.evaluate(() => window.__rejectProject("Request timed out", { error_code: undefined, timedOut: true, uncertain: true }));
        await page.waitForSelector(`[data-source-state='${success ? "done" : "unknown"}']`);
        expect(await page.locator("#nrprogress-actions").getAttribute("aria-busy")).toBe("false");
        await dismiss(page);
        expect(await page.locator("#scrim").getAttribute("class")).not.toContain("show");
        await page.evaluate(() => window.__releaseProjectCache());
        // Await the real cache write; a dismissed success must not navigate later.
        if (success) await page.waitForFunction(async () =>
          Boolean(await window.__layoutModules.cache.readCached(window.__layoutModules.settings.projectSettingsAddress("beast", "zimmerman"))));
        expect(await page.evaluate(() => window.__openedProject)).toBeUndefined();
        if (success) expect(await page.evaluate(async () =>
          (await window.__layoutModules.store.readUiRecord(window.__projectDraftAddress)).value.sources)).toEqual([]);
      }, { plugins: [deviceShim, holdProjectCache] });
    });
  }
}

for (const { label, width, height } of [
  { label: "phone", width: 390, height: 844 },
  { label: "desktop", width: 1280, height: 800 },
]) {
  it(`replaces the form, reports failure, preserves the draft and retries on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountNewProject(page, basePath);
      await page.locator("#nrdo").click();
      await assertProgress(page, 1);
      await page.keyboard.press("Escape");
      await page.locator("#scrim").click({ position: { x: 1, y: 1 } });
      expect(await page.locator("#scrim").getAttribute("class")).toContain("show");
      await page.evaluate((message) => window.__rejectProject(message), failure);
      await page.waitForSelector("[data-source-state='failed']");
      expect(await page.locator("[data-source-state='failed']").textContent()).toContain(failure);
      expect(await page.locator("#nrform").count()).toBe(0);
      expect(await page.locator("#sheet").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
      const saved = await page.evaluate(async () => (await window.__layoutModules.store.readUiRecord(window.__projectDraftAddress))?.value);
      expect(saved.name).toBe("Zimmerman Web Works");
      expect(saved.sources[0]).toMatchObject({ remote, name: "zimmerman-web-works", base_branch: "main" });
      await page.locator("#nrback").click();
      expect(await page.locator("[data-source-value]").inputValue()).toBe(remote);
      expect(await page.locator("[data-source-branch]").inputValue()).toBe("main");
      expect(await page.locator("#nrproject").inputValue()).toBe("Zimmerman Web Works");
      expect(await page.locator("#nrdevice").inputValue()).toBe("beast");
      expect(await page.locator("#nrdo").evaluate((node) => node === document.activeElement)).toBe(true);
      await page.locator("#nrdo").click();
      await assertProgress(page, 1);
      await resolveProject(page);
      await page.waitForSelector("[data-source-state='done']");
      expect(await page.locator("#nrprogress").textContent()).toContain("Project created");
      expect(await page.evaluate(() => window.__openedProject)).toBeUndefined();
      await page.evaluate(() => window.__releaseProjectCache());
      await page.waitForFunction(() => window.__openedProject);
      expect(await page.locator("#scrim").getAttribute("class")).not.toContain("show");
      const result = await page.evaluate(async () => ({
        opened: window.__openedProject,
        cached: (await window.__layoutModules.cache.readCached(window.__layoutModules.settings.projectSettingsAddress("beast", "zimmerman"))).value,
        draft: (await window.__layoutModules.store.readUiRecord(window.__projectDraftAddress)).value,
        requests: window.__projectRequests,
      }));
      expect(result.opened.project).toEqual(result.cached);
      expect(result.opened.target.id).toBe("beast");
      expect(result.draft.sources).toEqual([]);
      expect(result.requests).toHaveLength(2);
    }, { width, height, plugins: [deviceShim, holdProjectCache] });
  });

  it(`shows mixed local and remote folders settling together on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountNewProject(page, basePath, { local: true });
      await page.locator("#nrdo").click();
      await assertProgress(page, 2);
      expect(await page.locator("#nrprogress").textContent()).toContain("Adding /home/you/code/docs as docs");
      await resolveProject(page);
      await page.waitForSelector("[data-source-state='done']");
      expect(await page.locator("[data-source-state='done']").count()).toBe(2);
      expect(await page.locator("#nrcancel").textContent()).toBe("Close");
      expect(await page.locator("#sheet").evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
      await page.evaluate(() => window.__releaseProjectCache());
      await page.waitForFunction(() => window.__openedProject);
    }, { width, height, plugins: [deviceShim, holdProjectCache] });
  });
}
