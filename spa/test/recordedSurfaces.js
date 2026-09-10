import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const testDirectory = dirname(fileURLToPath(import.meta.url));

const RECORDED = readFileSync(
  join(testDirectory, "..", "..", "bridge", "tests", "fixtures", "agent_surfaces.json"),
  "utf8",
);

export const recordedSurfaces = () => JSON.parse(RECORDED);
