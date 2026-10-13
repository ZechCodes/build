import { afterAll, beforeAll, expect, it } from "vitest";
import { captureLayout, openLayoutSession } from "./layoutHarness.mjs";
import { deviceShim } from "./taskIdentityHarness.mjs";
import { mountWaitingOnFixture } from "./taskWaitingOnHarness.mjs";

let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [deviceShim] }); }, 30_000);
afterAll(async () => { await session?.close(); });

async function addPrerequisite(page, title, id) {
  await page.locator("[data-waiting-search]").fill(title);
  await page.locator("[data-waiting-choice]").selectOption(id);
  await page.locator("[data-waiting-add]").focus();
  await page.keyboard.press("Enter");
}

async function removePrerequisite(page, id) {
  await page.locator(`[data-waiting-remove="${id}"]`).focus();
  await page.keyboard.press("Enter");
  await page.locator("[data-waiting-confirm-remove]").focus();
  await page.keyboard.press("Enter");
}

for (const width of [390, 1280]) {
  for (const theme of ["light", "dark"]) {
    it(`edits cached edges with the keyboard and distinguishes Done from closed at ${width}px ${theme}`, async () => {
      await session.withPage(async ({ page, basePath }) => {
        await mountWaitingOnFixture(page, basePath, { theme, cachedOnly: true });
        expect(await page.locator("[data-task-waiting-on]").isVisible()).toBe(true);
        const forward = page.locator("[data-waiting-list]");
        expect(await forward.textContent()).toContain("Finish shared cache");
        expect(await forward.textContent()).toMatch(/Closed|closed/);
        expect(await forward.textContent()).toContain("Done");
        expect(await forward.locator('a[href*="closed-prerequisite"]').count()).toBe(1);
        expect(await forward.locator('a[href*="done-prerequisite"]').count()).toBe(1);
        await addPrerequisite(page, "integration", "candidate");
        await page.waitForFunction(() => window.__waiting.calls.some((call) => call.method === "tasks.waiting_on.add"));
        await page.locator('[data-waiting-remove="candidate"]').waitFor();
        expect(await page.evaluate(async () => (await window.__layoutModules.cache.readTaskRecord("device-waiting", "proj-1", "task-waiting")).task.dependencies.waiting_on.map((entry) => entry.task_id)))
          .toContain("candidate");
        await removePrerequisite(page, "candidate");
        await page.locator('[data-waiting-remove="candidate"]').waitFor({ state: "detached" });
        await page.locator("[data-blocks-more]").focus();
        await page.keyboard.press("Enter");
        await page.locator('[data-blocks-list] a[href*="dependent-page-two"]').waitFor();
        const reverse = page.locator('[data-blocks-list] a[href*="dependent-page-two"]');
        expect(await reverse.getAttribute("href")).toContain("/tasks/dependent-page-two");
        expect(await page.locator('[data-waiting-list] a[href*="prerequisite"]').first().getAttribute("href")).toContain("/tasks/prerequisite");
        expect(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)).toBe(false);
        await captureLayout(page, `task-waiting-on-${width}-${theme}.png`);
        await reverse.focus();
        await page.keyboard.press("Enter");
        expect(await page.evaluate(() => location.hash)).toContain("/tasks/dependent-page-two");
        await forward.locator('a[href*="closed-prerequisite"]').focus();
        await page.keyboard.press("Enter");
        expect(await page.evaluate(() => location.hash)).toContain("/tasks/closed-prerequisite");
      }, { width, height: width === 390 ? 844 : 900 });
    }, 30_000);
  }
}

it.each(["Would create a dependency cycle", "This task changed elsewhere; reload and try again"])("retains edge input and assignment notes after refusal: %s", async (refusal) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath);
    await page.locator("[data-task-assign]").click();
    await page.locator("#task-assign-note").fill("Keep the exact private instruction");
    await page.locator("[data-assign-cancel]").click();
    await page.evaluate((refusal) => { window.__waiting.refusal = refusal; }, refusal);
    await addPrerequisite(page, "integration", "candidate");
    await page.waitForFunction(() => document.querySelector("[data-waiting-error]")?.textContent.includes(window.__waiting.refusal));
    expect(await page.locator("[data-waiting-search]").inputValue()).toBe("integration");
    expect(await page.locator("[data-waiting-choice]").inputValue()).toBe("candidate");
    expect(await page.locator('[data-waiting-remove="candidate"]').count()).toBe(0);
    await page.locator("[data-task-assign]").click();
    await page.locator("#task-assign-assignee").selectOption("project_agent");
    await page.locator("#task-assign-note").fill("Keep the exact private instruction");
    await page.locator("[data-assign-go]").click();
    await page.locator("#task-assign-scrim .create-error").waitFor({ state: "visible" });
    expect(await page.locator("#task-assign-note").inputValue()).toBe("Keep the exact private instruction");
  });
}, 30_000);

it("escapes hostile picker titles and confines candidates to the same project", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath);
    await page.locator("[data-waiting-search]").fill("<img");
    expect(await page.locator('[data-waiting-choice] option[value="hostile-title"]').textContent()).toContain('<img src=x');
    expect(await page.locator('[data-waiting-choice] option[value="foreign"]').count()).toBe(0);
    expect(await page.locator('[data-waiting-choice] option[value="task-waiting"]').count()).toBe(0);
    expect(await page.locator("[data-waiting-picker] img").count()).toBe(0);
    expect(await page.evaluate(() => window.__titleExecuted)).toBeUndefined();
  });
}, 30_000);

it.each([false, true])("last unresolved edge removal explains only the saved assignment (held=%s)", async (assigned) => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { assigned });
    await page.locator('[data-waiting-remove="prerequisite"]').click();
    const consequence = await page.locator("[data-waiting-confirm]").textContent();
    expect(consequence).toMatch(assigned ? /starts the saved assignment/ : /[Nn]o.*worker|[Nn]obody|[Nn]o.*assignment/);
    await page.locator("[data-waiting-confirm-remove]").click();
    await page.waitForFunction(() => window.__waiting.calls.some((call) => call.method === "tasks.waiting_on.remove"));
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => ["tasks.assign", "workspace.create", "agent.start"].includes(call.method)))).toEqual([]);
    if (assigned) expect(await page.evaluate(() => window.__waiting.current.deferred_assignment.state)).toBe("queued");
  });
}, 30_000);

it("holds new workspace creation until readiness and describes agent assessment of closures", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath);
    await page.locator("[data-task-assign]").click();
    await page.locator("#task-assign-assignee").selectOption("new_workspace");
    const dialog = page.locator("#task-assign-scrim [role='dialog']");
    expect(await dialog.textContent()).toContain("Done or closed");
    expect(await dialog.textContent()).toMatch(/create.*workspace.*then|workspace.*when/i);
    expect(await dialog.textContent()).toMatch(/agent.*(review|assess).*clos/i);
    await page.locator("#task-assign-note").fill("Assess the closed approach before proceeding");
    await page.locator("[data-assign-go]").click();
    await dialog.waitFor({ state: "detached" });
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => ["workspace.create", "agent.start"].includes(call.method)))).toEqual([]);
    expect(await page.evaluate(() => window.__waiting.calls.find((call) => call.method === "tasks.assign").params)).toMatchObject({
      task_id: "task-waiting", assignee: { kind: "new_workspace" }, note: "Assess the closed approach before proceeding",
    });
  });
}, 30_000);

it("retries the exact failed intent without replacing its target or note", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { assigned: true, failed: true });
    await page.locator("[data-waiting-retry]").focus();
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__waiting.calls.some((call) => call.method === "tasks.assign"));
    expect(await page.evaluate(() => window.__waiting.calls.find((call) => call.method === "tasks.assign").params)).toEqual({
      task_id: "task-waiting", retry_deferred_id: "saved-assignment-exact",
    });
  });
}, 30_000);

it("refuses a captured waiting form under an older greeting despite previously cached support", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { assigned: true, failed: true });
    await page.locator("[data-waiting-choice]").selectOption("candidate");
    await page.evaluate(async () => {
      const fixture = window.__waiting;
      fixture.capturedSubmit = document.querySelector("[data-waiting-picker]").onsubmit;
      fixture.capturedRemove = document.querySelector('[data-waiting-remove="prerequisite"]').onclick;
      fixture.capturedRetry = document.querySelector("[data-waiting-retry]").onclick;
      await fixture.greet(true);
      fixture.calls.length = 0;
      await fixture.capturedSubmit?.({ preventDefault() {} });
      await fixture.capturedRetry?.();
    });
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => call.method.startsWith("tasks.waiting_on.") || call.params.retry_deferred_id || call.params.waiting_on))).toEqual([]);
  });
}, 30_000);


it("keeps the picker draft when cached closure resolution changes and explains reopening after delivery", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath);
    await page.locator("[data-waiting-search]").fill("integration");
    await page.locator("[data-waiting-choice]").selectOption("candidate");
    await page.evaluate(async () => {
      const fixture = window.__waiting;
      fixture.searchNode = document.querySelector("[data-waiting-search]");
      fixture.choiceNode = document.querySelector("[data-waiting-choice]");
      fixture.current.status = "in_progress";
      fixture.current.assignee = { kind: "project_agent" };
      const closed = fixture.current.dependencies.waiting_on.find((edge) => edge.task_id === "closed-prerequisite");
      Object.assign(closed, { state: "open", status: "ready", resolved: false, resolved_by: null });
      await fixture.writeCurrent();
    });
    await page.waitForFunction(() => document.querySelector("[data-waiting-notice]")?.textContent.includes("does not stop a running turn"));
    expect(await page.locator('[data-waiting-list] a[href*="closed-prerequisite"]').locator("..").textContent()).toContain("Waiting");
    expect(await page.locator("[data-waiting-search]").inputValue()).toBe("integration");
    expect(await page.locator("[data-waiting-choice]").inputValue()).toBe("candidate");
    expect(await page.evaluate(() => window.__waiting.searchNode === document.querySelector("[data-waiting-search]")
      && window.__waiting.choiceNode === document.querySelector("[data-waiting-choice]"))).toBe(true);
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => call.method === "tasks.assign"))).toEqual([]);
  });
}, 30_000);

it("keeps legacy assignment usable on an older greeting while hiding edge controls", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { old: true, legacy: true });
    expect(await page.locator("[data-task-waiting-on]").isVisible()).toBe(false);
    await page.locator("[data-task-assign]").click();
    await page.locator("#task-assign-assignee").selectOption("user");
    expect(await page.locator("#task-assign-scrim").textContent()).not.toContain("Done or closed");
    await page.locator("[data-assign-go]").click();
    await page.locator("#task-assign-scrim").waitFor({ state: "detached" });
    expect(await page.evaluate(() => window.__waiting.calls.find((call) => call.method === "tasks.assign").params)).toEqual({
      task_id: "task-waiting", assignee: { kind: "user" },
    });
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => call.method.startsWith("tasks.waiting_on.") || call.params.retry_deferred_id || call.params.waiting_on))).toEqual([]);
  });
}, 30_000);


it("retains cached waiting copy and refuses an assignment under an older greeting", async () => {
  await session.withPage(async ({ page, basePath }) => {
    await mountWaitingOnFixture(page, basePath, { old: true });
    expect(await page.locator("[data-task-waiting-on]").isVisible()).toBe(false);
    await page.locator("[data-task-assign]").click();
    await page.locator("#task-assign-assignee").selectOption("user");
    await page.locator("#task-assign-note").fill("Keep the waiting assignment");
    expect(await page.locator("#task-assign-scrim").textContent()).toContain("Done or closed");
    await page.locator("[data-assign-go]").click();
    await page.locator("#task-assign-scrim .create-error").waitFor({ state: "visible" });
    expect(await page.locator("#task-assign-scrim .create-error").textContent()).toMatch(/update.*bridge/i);
    expect(await page.locator("#task-assign-note").inputValue()).toBe("Keep the waiting assignment");
    expect(await page.locator("#task-assign-assignee").inputValue()).toBe("user");
    expect(await page.evaluate(() => window.__waiting.calls.filter((call) => call.method === "tasks.assign" || call.method.startsWith("tasks.waiting_on.")))).toEqual([]);
  });
}, 30_000);
