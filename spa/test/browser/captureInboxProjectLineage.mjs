// node test/browser/captureInboxProjectLineage.mjs /tmp/inbox-project-lineage-before
import { mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";
import { withLayoutPage } from "./layoutHarness.mjs";
import { mountProjectLineageInbox } from "./inboxProjectLineageSeed.mjs";

const output = resolve(process.argv[2] || "/tmp/inbox-project-lineage");
await mkdir(output, { recursive: true });
await withLayoutPage(async ({ page, basePath }) => {
  await mountProjectLineageInbox(page, basePath);
  await page.locator("#inbox-rail").screenshot({ path: join(output, "inbox-row.png"), animations: "disabled" });
  await page.screenshot({ path: join(output, "inbox-and-chat.png"), animations: "disabled" });
  console.log(JSON.stringify({ row: await page.locator(".inbox-project-agent").textContent(),
    chat: await page.locator("#agent-rail .rail-head").textContent(), output }));
}, { width: 1320, height: 720 });
