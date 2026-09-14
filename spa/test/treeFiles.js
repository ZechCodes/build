// The tree, as the guard suites read it. Several of them ask the same two
// questions — which .js files are under a root, and what does one of them say —
// so they ask them here: a scan that missed a directory would otherwise pass
// those guards by checking less than they claim.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const srcDirectory = join(here, "..", "src");
const testDirectory = here;

/** Every .js file under a root at any depth, named relative to it, in a stable
 *  order so a guard's report reads the same on every machine. */
const jsFilesIn = (root) =>
  readdirSync(root, { recursive: true })
    .map(String)
    .filter((file) => file.endsWith(".js"))
    .sort();

export const srcJsFiles = () => jsFilesIn(srcDirectory);

/** One src/ file's text, named as srcJsFiles names it. */
export const srcSourceOf = (file) => readFileSync(join(srcDirectory, file), "utf8");

/** The suites themselves, read the same way: a guard that bans something from
 *  the app has to be able to ask whether a test stood it back up. */
export const testJsFiles = () => jsFilesIn(testDirectory);

/** One test/ file's text, named as testJsFiles names it. */
export const testSourceOf = (file) => readFileSync(join(testDirectory, file), "utf8");
