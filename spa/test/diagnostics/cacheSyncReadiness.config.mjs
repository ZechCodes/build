// Opt-in, paired unit diagnostics. No production module transform or warmup.
import { createHash } from "node:crypto";
import base from "../../vite.config.js";

const marker = '  it("writes nothing back from a push read the sync layer stood down under", async () => {';
const root = process.env.READINESS_ROOT;
const mode = process.env.READINESS_MODE;
if (!root || !["red", "green", "visibility"].includes(mode) || !process.env.READINESS_ARTIFACT_DIR) {
  throw new Error("Explicit READINESS_ROOT, MODE, ARTIFACT_DIR are required");
}
const overlay = {
  name: "909-local-readiness-cases", enforce: "pre",
  transform(code, id) {
    if (!id.split("?")[0].endsWith("/test/cacheSyncDelivery.test.js")) return null;
    if (code.split(marker).length !== 2) throw new Error("Readiness target anchor missing/nonunique");
    if (code.split("afterEach(() => {\n").length !== 2) throw new Error("Readiness cleanup anchor missing/nonunique");
    const hash = createHash("sha256").update(code).digest("hex");
    const registerCases = mode === "visibility" ? "visibilityCases" : "readinessCases";
    const cases = `  ${registerCases}(() => ({ subscribingBridge, beginPushFixture, greet, branchItem, cache, sync, changeEvents,
      flush, settle, calls, called, passesSettled, call: () => bridge.call,
      currentContext: () => contexts.get("dev-1"), setBoard: (value) => { board = value; },
      sourceHash: ${JSON.stringify(hash)} }));\n\n`;
    const firstLine = code.indexOf("\n") + 1;
    const injected = code.replace(marker, cases + marker)
      .replace("afterEach(() => {\n", "afterEach(() => {\n  readinessAfterEach();\n");
    return { code: injected.slice(0, firstLine)
      + 'import { readinessCases, visibilityCases, readinessAfterEach } from "./diagnostics/cacheSyncReadiness.mjs";\n'
      + injected.slice(firstLine), map: null };
  },
};
export default {
  ...base,
  root: `${root}/spa`,
  plugins: [...base.plugins, overlay],
  test: { ...base.test, maxWorkers: 1, silent: false, reporters: ["verbose"],
    // This selected-unit configuration is explicitly not full-suite parity.
    projects: base.test.projects.filter((project) => project.test.name === "unit") },
};
