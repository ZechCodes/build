import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const coreDirectory = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "core");

export const coreSourceOf = (moduleFileName) => readFileSync(join(coreDirectory, moduleFileName), "utf8");
