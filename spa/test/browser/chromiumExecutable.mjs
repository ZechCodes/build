import { constants } from "node:fs";
import { access } from "node:fs/promises";
import { delimiter, join } from "node:path";

const chromiumNames = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];

export async function chromiumExecutable() {
  const configured = process.env.CHROMIUM_PATH;
  if (configured) {
    await access(configured, constants.X_OK);
    return configured;
  }
  for (const directory of (process.env.PATH || "").split(delimiter)) {
    for (const name of chromiumNames) {
      const candidate = join(directory, name);
      try {
        await access(candidate, constants.X_OK);
        return candidate;
      } catch { /* Try the next browser location. */ }
    }
  }
  throw new Error("Browser tests require Chromium or Chrome on PATH (or CHROMIUM_PATH)");
}
