import { existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
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
const log = process.env.BUILD_PI_TELEMETRY_LOG;
const readRecords = () => {
  try { return readFileSync(log, "utf8").trim().split("\n").map(JSON.parse); } catch { return []; }
};
function observe() {
  const records = readRecords();
  const last = records.at(-1);
  if (!last) return;
  const { owner, thread_id, launch_id, generation, sequence } = last;
  const status = records.some((r) => r.type === "unsupported") ? "unsupported" : records.some((r) => r.type === "ready") ? "ready" : "starting";
  const temp = `${log}.ack-next`;
  writeFileSync(temp, JSON.stringify({ owner, thread_id, launch_id, generation, sequence, status }), { mode: 0o600 });
  renameSync(temp, `${log}.ack`);
}
const emit = async (name, event = {}) => {
  const results = [];
  for (const handler of handlers.get(name) ?? []) results.push(await handler(event, ctx));
  observe();
  return results;
};
let initializationError;
try { await extension.default(pi); } catch (error) { initializationError = error.message; }
observe();
await emit("session_start");
const beforeFailureRecords = readRecords();
const scenario = process.env.PI_DEFAULT_SCENARIO;
if (scenario === "sink_failure") { unlinkSync(log); mkdirSync(log); await emit("agent_start"); }
if (scenario === "hook_failure") { ctx.getContextUsage = () => { throw new Error("private hook payload"); }; await emit("agent_start"); }
if (scenario === "lease_failure") {
  const original = performance.now.bind(performance);
  Object.defineProperty(performance, "now", { configurable: true, value: () => original() + 9000 });
  await emit("agent_start");
  delete performance.now;
}
let toolResult;
if (["sink_failure", "hook_failure", "lease_failure", "missing_log"].includes(scenario)) {
  try { toolResult = await registered.get("done").execute("call", { kind: "one" }); } catch (error) { toolResult = { error: error.message }; }
}
if (process.env.PI_DEFAULT_SCENARIO === "mcp_failure") {
  try { await registered.get("done").execute("call", { kind: "child_exit" }); } catch { /* Transport failure must signal Pi after its MCP child is reaped. */ }
}
const threshold = await emit("session_before_compact", { reason: "threshold" });
await emit("session_shutdown");
await new Promise((resolve) => setImmediate(resolve));
const records = scenario === "sink_failure" ? beforeFailureRecords : readRecords();
let childRunning = false;
if (existsSync(process.env.FAKE_MCP_PID)) {
  const child = Number(readFileSync(process.env.FAKE_MCP_PID, "utf8"));
  try { kill(child, 0); childRunning = true; } catch (error) { if (error.code !== "ESRCH") throw error; }
}
process.stdout.write(JSON.stringify({ initializationError, records, threshold, signals, childRunning, toolResult }));
