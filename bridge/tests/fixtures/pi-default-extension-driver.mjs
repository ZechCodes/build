import { existsSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const extension = await import(pathToFileURL(process.env.BUILD_PI_EXTENSION_PATH));
const handlers = new Map();
const registered = new Map();
const signals = [];
const kill = process.kill;
process.kill = (pid, signal) => { signals.push({ pid, signal }); return true; };
const pi = {
  registerTool(tool) {
    if (process.env.PI_DEFAULT_SCENARIO === "registration_failure") throw new Error("private registration payload");
    registered.set(tool.name, tool);
  },
  on(name, handler) { handlers.set(name, [...(handlers.get(name) ?? []), handler]); },
};
const ctx = {
  sessionManager: { getSessionId: () => process.env.BUILD_PI_MCP_OWNER },
  getContextUsage: () => ({ tokens: 160000, contextWindow: 200000 }),
  isIdle: () => true,
  hasPendingMessages: () => false,
};
const emit = async (name, event = {}) => {
  const results = [];
  for (const handler of handlers.get(name) ?? []) results.push(await handler(event, ctx));
  return results;
};
let initializationError;
try { await extension.default(pi); } catch (error) { initializationError = error.message; }
await emit("session_start");
if (process.env.PI_DEFAULT_SCENARIO === "mcp_failure") {
  try { await registered.get("done").execute("call", { kind: "child_exit" }); } catch { /* The failure must be telemetry, not a killed Pi. */ }
}
const threshold = await emit("session_before_compact", { reason: "threshold" });
await emit("session_shutdown");
const records = process.env.BUILD_PI_TELEMETRY_LOG && existsSync(process.env.BUILD_PI_TELEMETRY_LOG)
  ? readFileSync(process.env.BUILD_PI_TELEMETRY_LOG, "utf8").trim().split("\n").map(JSON.parse) : [];
let childRunning = false;
if (existsSync(process.env.FAKE_MCP_PID)) {
  const child = Number(readFileSync(process.env.FAKE_MCP_PID, "utf8"));
  try { kill(child, 0); childRunning = true; } catch (error) { if (error.code !== "ESRCH") throw error; }
}
process.stdout.write(JSON.stringify({ initializationError, records, threshold, signals, childRunning }));
