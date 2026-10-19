// Review the production menu and context gauge using only cached fixture data.
// From spa/: node test/browser/captureCompactionObservations.mjs /tmp/661-revision-evidence
import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/661-revision-evidence");
await mkdir(output, { recursive: true });
const inherited = { tokens: 150000, source: { kind: "workspace", id: "menu-workspace" } };
const configured = { tokens: 150000, source: { kind: "conversation" } };
const active = { tokens: 200000, evidence: "applied_at_start" };
const states = {
  "older-bridge": undefined,
  pending: { inherited, configured, active, enforcement: "between_turn", pending_change: true },
  fallback: { inherited, configured: { tokens: 200000 }, active, enforcement: "between_turn", pending_change: false },
  unknown: { inherited, configured, active: null, enforcement: "unsupported", pending_change: null },
  native: { inherited, configured, active: { ...active, translated_trigger: 180000 }, enforcement: "native", pending_change: true },
};
const session = await openLayoutSession();
try {
  for (const width of [1180, 320]) {
    for (const [name, compaction] of Object.entries(states)) {
      const olderBridge = name === "older-bridge";
      await session.withPage(async ({ page, basePath }) => {
        await openMenuOn(page, basePath, width === 320 ? "narrow" : "desktop", { theme: "dark", bigCounts: false, olderBridge });
        await page.evaluate((patch) => window.__repaintMenuAgent(patch), {
          max_context_tokens: olderBridge ? 0 : 150000, ...(olderBridge ? {} : { compaction }),
        });
        await settled(page);
        if (width === 320) {
          // The narrow menu overlays the composer. Capture each real surface
          // in turn rather than forcing a pointer through the open menu.
          await page.screenshot({ path: join(output, `menu-${name}-${width}.png`) });
          await page.keyboard.press("Escape");
          await settled(page);
        }
        await page.locator("#railinputgauge").hover();
        await page.screenshot({ path: join(output, `menu-gauge-${name}-${width}.png`) });
      }, { width, height: 840 });
    }
  }
} finally {
  await session.close();
}
