import { expect, it } from "vitest";
import opening from "../../../fixtures/api/v1/tasks.review.open.json";
import pushing from "../../../fixtures/api/v1/tasks.review.push.json";
import { captureLayout, loadBrowserModules, mountLayout, withLayoutPage } from "./layoutHarness.mjs";

const sizes = [{ label: "desktop", width: 1280, hasTouch: false }, { label: "mobile", width: 390, hasTouch: true }];

async function mountWorkspaceReview(page, basePath, { published = false, rejectedOpen = false } = {}) {
  await mountLayout(page, '<div id="shell"><div id="view"><header id="toolbar">Workspace</header><div id="view-body"><main id="root" class="surface"><div class="workspace-changes"><div class="workspace-changes-header"><div id="review-entry"></div></div><nav class="workspace-dirtabs" aria-label="Directories"><button class="workspace-dirtab">API</button><button class="workspace-dirtab">UI</button><button class="workspace-dirtab">Notes</button></nav><div class="workspace-changes-body"><p>Committed workspace changes</p></div></div></main></div></div></div>', {
    basePath, styles: '#shell{height:100vh;box-sizing:border-box} #view-body,#root{min-width:0} .workspace-changes-body{padding:1rem}',
  });
  await page.evaluate(() => {
    document.head.prepend(Object.assign(document.createElement("meta"), { name: "viewport", content: "width=device-width, initial-scale=1" }));
  });
  await loadBrowserModules(page, {
    entry: "src/core/workspaceReviewEntry.js", support: "src/core/taskReviewSupport.js",
    review: "src/core/taskReviewCache.js", local: "src/core/localCache.js",
    directory: "src/core/directoryScope.js", model: "src/core/workspaceModel.js",
    motion: "src/core/motion.js",
    drafts: "src/core/taskReviewDrafts.js", ui: "src/core/localUiStore.js",
  }, basePath);
  await page.evaluate(async ({ opened, pushed, published, rejectedOpen }) => {
    const { entry, support, review, local, directory, model } = window.__layoutModules;
    const scope = { deviceId: "workspace-review-layout", projectId: "layout-project", workspaceId: "workspace-1" };
    const directories = opened.review.snapshots[0].directories.map((row) => ({ ...row, base_branch: "main", branch: "build/work" }));
    const workspace = { id: scope.workspaceId, workspace_id: scope.workspaceId, project_id: scope.projectId, name: "API and UI changes", directories };
    const initial = { ...opened, sync: pushed.result.sync.map((fact) => ({ ...fact, revision: 1 })) };
    if (published) {
      workspace.active_review = { task_id: initial.review.task_id, workspace_id: scope.workspaceId, version: 1, status: "open" };
      for (const row of directories) row.branch = initial.review.bindings.find((binding) => binding.directory_id === row.id)?.dedicated_branch_ref.replace("refs/heads/", "") || row.branch;
      await review.writeReviewReply({ ...scope, taskId: initial.review.task_id }, initial, 1);
    }
    await local.writeCached({ deviceId: scope.deviceId, entityId: "", kind: "workspaces" }, [workspace]);
    for (const row of directories.filter((row) => row.is_git && row.status === "git")) {
      const entityId = directory.directoryCacheId(model.workspaceScope(scope.workspaceId, row.source_id, workspace));
      await local.writeCached({ deviceId: scope.deviceId, entityId, kind: "refs" }, {
        current: { kind: "branch", full_ref: `refs/heads/${row.branch}`, name: row.branch },
        refs: ["main", "release", row.branch].map((name) => ({ kind: "branch", full_ref: `refs/heads/${name}`, name })),
      });
    }
    await support.rememberReviewSupport(scope.deviceId, { reviews: { get: true, snapshot: true, pullRequests: true, open: true, push: true } });
    window.__reviewCalls = [];
    window.__reviewNavigations = [];
    window.__reviewScope = scope;
    window.__reviewAnswer = initial;
    const callRpc = async (method, params) => {
      window.__reviewCalls.push({ method, params });
      if (method === "tasks.review.open") {
        if (rejectedOpen) throw Object.assign(new Error("The selected review base was rejected. Choose another base branch."), { code: "invalid_params" });
        return initial;
      }
      if (method === "tasks.review.get") return window.__reviewAnswer;
      if (method === "tasks.review.push") {
        const answer = structuredClone(pushed.examples[1].result);
        answer.review.version = 2;
        answer.sync = answer.sync.map((fact) => ({ ...fact, revision: 3 }));
        window.__reviewAnswer = answer;
        return answer;
      }
      throw new Error(`Unexpected review request: ${method}`);
    };
    window.__workspaceReview = entry.mountWorkspaceReviewEntry(document.querySelector("#review-entry"), {
      ...scope, callRpc, navigate: (route) => window.__reviewNavigations.push(route),
    });
  }, { opened: opening.result, pushed: pushing, published, rejectedOpen });
  await page.locator(published ? "[data-review-link]" : "[data-workspace-review]").waitFor({ timeout: 5000 });
}

async function expectFitsViewport(page) {
  await page.evaluate(() => window.__layoutModules.motion.motionSettled());
  const bounds = await page.evaluate(() => ({ viewport: innerWidth, page: document.documentElement.scrollWidth,
    dialogs: [...document.querySelectorAll('[role="dialog"]')].map((node) => ({ left: node.getBoundingClientRect().left, right: node.getBoundingClientRect().right })) }));
  expect(bounds.page, JSON.stringify(bounds)).toBeLessThanOrEqual(bounds.viewport + 1);
  for (const dialog of bounds.dialogs) {
    expect(dialog.left).toBeGreaterThanOrEqual(0);
    expect(dialog.right).toBeLessThanOrEqual(bounds.viewport + 1);
  }
}

const readCreateDraft = (page) => page.evaluate(async () => {
  const { drafts, ui } = window.__layoutModules;
  return (await ui.readUiRecord(drafts.reviewCreateDraftAddress(window.__reviewScope)))?.value;
});

for (const { label, width, hasTouch } of sizes) {
  it(`opens a multi-directory review with usable controls on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWorkspaceReview(page, basePath);
      const open = page.locator("[data-workspace-review]");
      expect(await open.textContent()).toContain("Open review");
      await expectFitsViewport(page);
      await captureLayout(page, `workspace-review-entry-${label}.png`);
      if (hasTouch) await open.tap();
      else {
        await page.keyboard.press("Tab");
        expect(await open.evaluate((node) => node === document.activeElement)).toBe(true);
        await page.keyboard.press("Enter");
      }
      await page.locator("[data-review-title]").waitFor({ timeout: 5000 });
      expect(await page.evaluate(() => window.__reviewCalls)).toEqual([]);
      await page.locator("[data-review-title]").fill("Review API and UI changes");
      await page.locator("[data-review-description]").fill("Publish the committed API and UI changes.");
      await page.locator("#review-create-assignee").selectOption("project_agent");
      await page.locator('[data-review-base="dir-api"]').fill("main");
      await page.locator('[data-review-base="dir-ui"]').fill("release");
      await page.locator('[data-review-exclude="dir-missing"]').check();
      const form = page.locator('[role="dialog"]');
      expect(await form.textContent()).toContain("review/<number>-review-api-and-ui-changes");
      expect(await form.textContent()).toContain("Notes");
      expect(await page.locator('[data-review-base="dir-notes"]').count()).toBe(0);
      await expectFitsViewport(page);
      await form.evaluate((node) => { node.scrollTop = 0; });
      await captureLayout(page, `workspace-review-open-${label}.png`);
      const submit = page.locator("[data-open-review-submit]");
      await submit.scrollIntoViewIfNeeded();
      await captureLayout(page, `workspace-review-open-sources-${label}.png`);
      if (hasTouch) await submit.tap();
      else {
        await submit.focus();
        await page.keyboard.press("Enter");
      }
      await page.locator("[data-review-link]").waitFor({ timeout: 5000 });
      const calls = await page.evaluate(() => window.__reviewCalls.filter((call) => call.method === "tasks.review.open"));
      expect(calls).toHaveLength(1);
      expect(calls[0].params).toMatchObject({ workspace_id: "workspace-1", title: "Review API and UI changes",
        description: "Publish the committed API and UI changes.", reviewer: { kind: "project_agent" },
        bases: [{ directory_id: "dir-api", branch: "main" }, { directory_id: "dir-ui", branch: "release" }],
        excluded_git_directory_ids: ["dir-missing"] });
      expect(calls[0].params.request_id).toEqual(expect.any(String));
      expect(await page.locator("#review-entry").textContent()).toContain("review/1-api");
      await expectFitsViewport(page);
      await captureLayout(page, `workspace-review-created-${label}.png`);
      await page.locator("[data-review-link]").click();
      expect(await page.evaluate(() => window.__reviewNavigations)).toEqual([{ name: "trackerTask", deviceId: "workspace-review-layout", projectId: "layout-project", taskId: "task-1" }]);
      await page.evaluate(() => window.__workspaceReview.dispose());
    }, { width, height: 844, hasTouch });
  }, 60_000);

  it(`keeps a rejected Open review draft editable and durable on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWorkspaceReview(page, basePath, { rejectedOpen: true });
      await page.locator("[data-workspace-review]").click();
      await page.locator("[data-review-title]").fill("Review API and UI changes");
      await page.locator("[data-review-description]").fill("Publish the committed API and UI changes.");
      await page.locator("#review-create-assignee").selectOption("project_agent");
      await page.locator('[data-review-base="dir-ui"]').fill("release");
      const submit = page.locator("[data-open-review-submit]");
      if (hasTouch) await submit.tap();
      else { await submit.focus(); await page.keyboard.press("Enter"); }
      const error = page.locator("[data-review-form-error]");
      await error.waitFor({ state: "visible", timeout: 5000 });
      expect(await error.textContent()).toContain("The selected review base was rejected.");
      for (const selector of ["[data-review-title]", "[data-review-description]", "#review-create-assignee", '[data-review-base="dir-api"]', '[data-review-base="dir-ui"]']) {
        expect(await page.locator(selector).isEnabled(), selector).toBe(true);
      }
      expect(await page.locator('[data-review-base="dir-missing"]').isEnabled()).toBe(false);
      const rejected = await readCreateDraft(page);
      expect(rejected).toMatchObject({ title: "Review API and UI changes", description: "Publish the committed API and UI changes.",
        reviewerDraft: { optionId: "project_agent" }, excluded_git_directory_ids: ["dir-missing"] });
      expect(rejected.submitted).toBeFalsy();
      await expectFitsViewport(page);
      await error.scrollIntoViewIfNeeded();
      await captureLayout(page, `workspace-review-rejected-${label}.png`);
      await page.locator("[data-review-title]").fill("Review revised API and UI changes");
      await page.locator("[data-review-description]").fill("Use the revised publication description.");
      await page.locator("#review-create-assignee").selectOption("none");
      await page.locator('[data-review-base="dir-api"]').fill("release");
      await page.waitForFunction(async () => {
        const { drafts, ui } = window.__layoutModules;
        const held = (await ui.readUiRecord(drafts.reviewCreateDraftAddress(window.__reviewScope)))?.value;
        return held?.title === "Review revised API and UI changes" && held.bases.find((base) => base.directory_id === "dir-api")?.branch === "release";
      });
      await page.locator("[data-review-form-cancel]").click();
      await page.locator("[data-review-title]").waitFor({ state: "detached" });
      await page.locator("[data-workspace-review]").click();
      await page.locator("[data-review-title]").waitFor({ state: "visible" });
      expect(await page.locator("[data-review-title]").inputValue()).toBe("Review revised API and UI changes");
      expect(await page.locator("[data-review-description]").inputValue()).toBe("Use the revised publication description.");
      expect(await page.locator("#review-create-assignee").inputValue()).toBe("none");
      expect(await page.locator('[data-review-base="dir-api"]').inputValue()).toBe("release");
      expect(await page.locator('[data-review-base="dir-api"]').isEnabled()).toBe(true);
      expect(await page.locator('[data-review-base="dir-missing"]').isEnabled()).toBe(false);
      expect((await readCreateDraft(page)).submitted).toBeFalsy();
      expect(await page.evaluate(() => window.__reviewCalls.filter((call) => call.method === "tasks.review.open"))).toHaveLength(1);
      await expectFitsViewport(page);
      await page.locator('[role="dialog"]').evaluate((node) => { node.scrollTop = 0; });
      await captureLayout(page, `workspace-review-rejected-editable-${label}.png`);
      await page.locator("[data-review-form-cancel]").click();
      await page.evaluate(() => window.__workspaceReview.dispose());
    }, { width, height: 844, hasTouch });
  }, 60_000);

  it(`updates cached pending commits and publishes a new snapshot on ${label}`, async () => {
    await withLayoutPage(async ({ page, basePath }) => {
      await mountWorkspaceReview(page, basePath, { published: true });
      const push = page.locator('[data-review-push="dir-api"]');
      await push.waitFor({ timeout: 5000 });
      expect(await push.textContent()).toContain("0");
      await page.evaluate(async () => {
        const { review } = window.__layoutModules;
        const scope = { ...window.__reviewScope, taskId: "task-1" };
        const answer = structuredClone(window.__reviewAnswer);
        answer.sync = answer.sync.map((fact) => fact.directory_id === "dir-api"
          ? { ...fact, revision: 2, health: "pending", pending_commits: 1, working_head: "4".repeat(40) } : fact);
        window.__reviewAnswer = answer;
        await review.writeReviewReply(scope, answer, 2);
      });
      await page.waitForFunction(() => document.querySelector('[data-review-push="dir-api"]')?.textContent.includes("1"));
      expect(await page.evaluate(() => window.__reviewCalls.filter((call) => call.method === "tasks.review.push"))).toEqual([]);
      await expectFitsViewport(page);
      await captureLayout(page, `workspace-review-pending-${label}.png`);
      if (hasTouch) await push.tap();
      else {
        await push.focus();
        await page.keyboard.press("Enter");
      }
      await page.locator("[data-push-review-submit]").waitFor({ timeout: 5000 });
      await expectFitsViewport(page);
      await captureLayout(page, `workspace-review-push-confirm-${label}.png`);
      if (hasTouch) await page.locator("[data-push-review-submit]").tap();
      else {
        await page.locator("[data-push-review-submit]").focus();
        await page.keyboard.press("Enter");
      }
      await page.waitForFunction(() => document.querySelector('[data-review-push="dir-api"]')?.textContent.includes("0") && !document.querySelector('[data-push-review-submit]'));
      const calls = await page.evaluate(() => window.__reviewCalls.filter((call) => call.method === "tasks.review.push"));
      expect(calls).toHaveLength(1);
      expect(calls[0].params).toMatchObject({ task_id: "task-1", expected_version: 1,
        sources: [{ directory_id: "dir-api", expected_head: "4".repeat(40), expected_received_head: "2".repeat(40) }] });
      const saved = await page.evaluate(async () => {
        const { review, local } = window.__layoutModules;
        return (await local.readCached(review.reviewAddress({ ...window.__reviewScope, taskId: "task-1" }))).value.review;
      });
      expect(saved.snapshots).toHaveLength(2);
      expect(saved.pull_request.latest_published_snapshot_id).toBe("snapshot-2");
      expect(await page.locator("#review-entry").textContent()).toContain("Snapshot 2");
      await expectFitsViewport(page);
      await captureLayout(page, `workspace-review-published-${label}.png`);
      await page.evaluate(() => window.__workspaceReview.dispose());
    }, { width, height: 844, hasTouch });
  }, 60_000);
}
