// The source tree, as the guard suites read it. Three of them ask the same two
// questions — which files are under src/, and what does one of them say — so
// they ask them here: a scan that missed a directory would otherwise pass those
// guards by checking less than they claim.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const srcDirectory = join(dirname(fileURLToPath(import.meta.url)), "..", "src");

/** Every .js file under src/ at any depth, named relative to src/, in a stable
 *  order so a guard's report reads the same on every machine. */
export const srcJsFiles = () =>
  readdirSync(srcDirectory, { recursive: true })
    .map(String)
    .filter((file) => file.endsWith(".js"))
    .sort();

/** One src/ file's text, named as srcJsFiles names it. */
export const srcSourceOf = (file) => readFileSync(join(srcDirectory, file), "utf8");
