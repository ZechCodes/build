// Review the production menu and context gauge using only cached fixture data.
// From spa/: node test/browser/captureCompactionObservations.mjs /tmp/661-evidence
import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { openLayoutSession } from "./layoutHarness.mjs";
import { openMenuOn, settled } from "./chatMenuSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/661-evidence");
await mkdir(output, { recursive: true });
const inherited = { tokens: 150000, source: { kind: "workspace", id: "menu-workspace" } };
const configured = { tokens: 150000, source: { kind: "conversation" } };
const active = { tokens: 200000, evidence: "applied_at_start" };
const states = {
  pending: { inherited, configured, active, enforcement: "between_turn", pending_change: true },
  fallback: { inherited, configured: { tokens: 200000 }, active, enforcement: "between_turn", pending_change: false },
  unknown: { inherited, configured, active: null, enforcement: "unsupported", pending_change: null },
  native: { inherited, configured, active: { ...active, translated_trigger: 180000 }, enforcement: "native", pending_change: true },
};
const session = await openLayoutSession();
try {
  for (const [name, compaction] of Object.entries(states)) {
    await session.withPage(async ({ page, basePath }) => {
      await openMenuOn(page, basePath, "desktop", { theme: "dark", bigCounts: false });
      await page.evaluate((compaction) => window.__repaintMenuAgent({ max_context_tokens: 150000, compaction }), compaction);
      await settled(page);
      await page.locator("#railinputgauge").hover();
      await page.screenshot({ path: join(output, `menu-gauge-${name}.png`) });
    });
  }
} finally {
  await session.close();
}
