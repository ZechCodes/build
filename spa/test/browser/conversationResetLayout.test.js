import { expect, it } from "vitest";
import { captureLayout, withLayoutPage } from "./layoutHarness.mjs";
import { mountChatMenu, settled } from "./chatMenuSeed.mjs";

it("Clear conversation stays last, keyboard reachable, and opens the harness picker on a phone", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChatMenu(page, basePath, "phone", { theme: "dark", bigCounts: false, resetCapable: true });
    const clear = page.locator('.rail-surface-menu .mi[data-action="conversation:clear"]');
    await clear.waitFor({ state: "attached", timeout: 5000 });
    await page.locator(".rail-surface-menu .caret").focus();
    await page.keyboard.press("ArrowDown");
    await settled(page);
    expect(await page.locator(".rail-surface-menu .mi").last().getAttribute("data-action")).toBe("conversation:clear");
    await page.keyboard.press("End");
    await settled(page);
    expect(await clear.evaluate((node) => node === document.activeElement)).toBe(true);
    const rect = await clear.boundingBox();
    const compact = page.locator('.rail-surface-menu [data-group="compact"]');
    expect(await compact.locator('[role="slider"]').count()).toBe(1);
    const compactBounds = await compact.boundingBox();
    expect(rect.y).toBeGreaterThanOrEqual(compactBounds.y + compactBounds.height);
    expect(rect.y + rect.height).toBeLessThanOrEqual(844);
    await captureLayout(page, "clear-conversation-menu-phone.png");
    await page.keyboard.press("Enter");
    await page.locator("[data-confirm-ok]").waitFor({ timeout: 5000 });
    await settled(page);
    expect(await page.getByText("Everything in it is removed and a new one starts.", { exact: true }).isVisible()).toBe(true);
    const confirmBounds = await page.locator("[data-confirm-ok]").boundingBox();
    expect(confirmBounds.y + confirmBounds.height).toBeLessThanOrEqual(844);
    await captureLayout(page, "clear-conversation-confirm-phone.png");
    await page.locator("[data-confirm-ok]").click({ timeout: 5000 });
    await page.locator("[data-clear-start]").waitFor({ timeout: 5000 });
    expect(await page.locator(".rail-harness-choice.chosen").getAttribute("data-provider")).toBe("claude_adk");
    await captureLayout(page, "clear-conversation-picker-phone.png");
    await page.locator("[data-clear-cancel]").click();
    await page.locator("[data-clear-start]").waitFor({ state: "detached" });
    expect(await page.getByText("Separate the settings from what the agent opened.", { exact: true }).isVisible()).toBe(true);
    expect(await page.evaluate(() => window.__conversationResetCalls)).toEqual([]);
  }, { width: 390, height: 844 });
}, 30000);

it("changes harness and clears the old transcript in place from the real menu", async () => {
  await withLayoutPage(async ({ page, basePath }) => {
    await mountChatMenu(page, basePath, "desktop", { theme: "light", bigCounts: false, resetCapable: true });
    const oldWords = "Separate the settings from what the agent opened.";
    await page.getByText(oldWords, { exact: true }).waitFor({ timeout: 5000 });
    await page.locator(".rail-surface-menu .caret").click();
    await page.locator('[data-action="conversation:clear"]').click();
    await page.locator("[data-confirm-ok]").click();
    await page.locator('[data-provider="codex"]').click();
    await page.locator("[data-clear-start]").click();
    await page.locator("[data-clear-start]").waitFor({ state: "detached", timeout: 5000 });
    await page.getByText(oldWords, { exact: true }).waitFor({ state: "detached", timeout: 5000 });
    const calls = await page.evaluate(() => window.__conversationResetCalls);
    expect(calls).toHaveLength(1);
    expect(calls).toMatchObject([{ provider: "codex", expected_thread_id: "thread:conversation-menu-agent" }]);
    expect(await page.locator("#railinput").isVisible()).toBe(true);
    expect(await page.locator('#railinput').inputValue()).toBe("");
    expect(await page.getByText("120k", { exact: true }).count()).toBe(0);
    expect(await page.evaluate(() => location.hash)).toBe("");
    await captureLayout(page, "clear-conversation-fresh-desktop.png");

    // The same slider remains usable after reset, addressed to the replacement
    // generation rather than the conversation that was just removed.
    await page.locator(".rail-surface-menu .caret").click();
    await page.locator('.rail-surface-menu [role="slider"]').focus();
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => window.__menuSettingsAsked.length === 1);
    expect(await page.evaluate(() => window.__menuSettingsAsked)).toEqual([{
      entity_id: "menu-run", agent_id: "menu-agent", max_context_tokens: 150000,
      thread_id: "thread:conversation-menu-agent:browser-reset",
    }]);
  });
}, 30000);
