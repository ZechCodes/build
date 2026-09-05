import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const extension = await import(pathToFileURL(process.env.BUILD_PI_EXTENSION_PATH));
const tools = new Map();
const handlers = new Map();
const scenario = process.env.PI_DRIVER_SCENARIO ?? "happy";
const terminationSignals = [];
let toolRegistrations = 0;
let hookRegistrations = 0;

if (new Set(["latched", "latched_multiple", "latched_write"]).has(scenario)) {
  process.kill = (pid, signal) => {
    terminationSignals.push({ pid, signal });
    return true;
  };
}

const pi = {
  registerTool(tool) {
    toolRegistrations += 1;
    if (scenario === "register_tool_failure" && toolRegistrations === 2) {
      throw new Error("fixture registerTool failure");
    }
    tools.set(tool.name, tool);
  },
  on(name, handler) {
    hookRegistrations += 1;
    if (scenario === "register_hook_failure" && hookRegistrations === 2) {
      throw new Error("fixture hook registration failure");
    }
    handlers.set(name, handler);
  },
};

const execute = (parameters) => tools.get("done").execute("call", parameters);
const executeTool = (name, parameters) => tools.get(name).execute("call", parameters);

async function caught(parameters) {
  try {
    await execute(parameters);
    return "unexpected success";
  } catch (error) {
    return error.message;
  }
}

function writeOutput(value) {
  return new Promise((resolve) => process.stdout.write(JSON.stringify(value), resolve));
}

async function waitForTermination() {
  const deadline = Date.now() + 2000;
  while (terminationSignals.length === 0 && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function mcpChildIsRunning() {
  try {
    const childPid = Number(readFileSync(process.env.FAKE_MCP_PID, "utf8"));
    process.kill(childPid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

async function failIfMcpChildLeaks() {
  const deadline = Date.now() + 2500;
  while (mcpChildIsRunning() && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  if (!mcpChildIsRunning()) return;
  const childPid = Number(readFileSync(process.env.FAKE_MCP_PID, "utf8"));
  process.kill(childPid, "SIGKILL");
  process.exit(3);
}

async function happyScenario() {
  const concurrent = await Promise.all([
    execute({ kind: "concurrent", value: "first-call" }),
    execute({ kind: "concurrent", value: "second-call" }),
  ]);
  const malformed = await caught({ kind: "malformed" });
  const afterMalformed = await execute({ kind: "one" });
  const unsupported = await caught({ kind: "unsupported" });
  const rpcError = await caught({ kind: "rpc_error" });
  const afterRpcError = await execute({ kind: "one" });
  const output = {
    tools: [...tools.values()].map((tool) => ({
      name: tool.name,
      label: tool.label,
      description: tool.description,
      parameters: tool.parameters,
    })),
    one: await execute({ kind: "one" }),
    many: await execute({ kind: "many" }),
    empty: await execute({ kind: "empty" }),
    error: await caught({ kind: "error" }),
    emptyError: await caught({ kind: "empty_error" }),
    malformed,
    unsupported,
    rpcError,
    afterMalformed,
    afterRpcError,
    concurrent,
    switchRefused: handlers.get("session_before_switch")().cancel,
    forkRefused: handlers.get("session_before_fork")().cancel,
  };
  await handlers.get("session_shutdown")();
  await writeOutput(output);
}

async function timeoutScenario() {
  await execute({ kind: "timeout" });
  throw new Error("timeout unexpectedly succeeded");
}

async function terminalScenario() {
  await execute({ kind: "terminal" });
  throw new Error("terminal failure unexpectedly succeeded");
}

async function latchedScenario() {
  const pendingFailure = await caught({ kind: "child_exit" });
  const futureFailure = await caught({ kind: "after_exit" });
  await waitForTermination();
  await writeOutput({ pendingFailure, futureFailure, terminationSignals });
  process.exit(0);
}

async function latchedMultipleScenario() {
  const pending = Promise.all([
    caught({ kind: "invalid_error", value: "first" }),
    caught({ kind: "invalid_error", value: "second" }),
  ]);
  const pendingFailures = await Promise.race([
    pending,
    new Promise((resolve) => setTimeout(() => resolve(["unsettled", "unsettled"]), 500)),
  ]);
  const futureFailure = await caught({ kind: "after_invalid_error" });
  await waitForTermination();
  await writeOutput({ pendingFailures, futureFailure, terminationSignals });
  process.exit(0);
}

async function realMcpScenario() {
  const invalidError = await caught({ phase: "build" });
  let invalidPostError;
  try {
    await executeTool("post_thread_message", {});
    invalidPostError = "unexpected success";
  } catch (error) {
    invalidPostError = error.message;
  }
  const valid = await execute({
    phase: "build",
    status: "blocked",
    summary: "waiting for deterministic input",
  });
  await handlers.get("session_shutdown")();
  await writeOutput({ invalidError, invalidPostError, valid });
}

async function latchedWriteScenario() {
  const failure = await caught({ kind: "write_failure" });
  await waitForTermination();
  await writeOutput({ failure, terminationSignals });
  process.exit(0);
}

const scenarioHandlers = new Map([
  ["happy", happyScenario],
  ["timeout", timeoutScenario],
  ["terminal", terminalScenario],
  ["latched", latchedScenario],
  ["latched_multiple", latchedMultipleScenario],
  ["real_mcp", realMcpScenario],
  ["latched_write", latchedWriteScenario],
]);

try {
  await extension.default(pi);
  if (process.env.PI_DRIVER_SESSION_STARTED) {
    writeFileSync(process.env.PI_DRIVER_SESSION_STARTED, "started");
  }
  const selected = scenarioHandlers.get(scenario);
  if (!selected) throw new Error(`unknown Pi extension driver scenario: ${scenario}`);
  await selected();
} catch (error) {
  process.stderr.write(error.message);
  if (scenario === "empty_tools"
      || scenario === "register_tool_failure"
      || scenario === "register_hook_failure") {
    await failIfMcpChildLeaks();
  }
  process.exitCode = 2;
}
