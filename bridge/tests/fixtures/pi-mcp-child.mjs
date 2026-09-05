#!/usr/bin/env node
import { appendFileSync, closeSync, writeFileSync } from "node:fs";

const mode = process.env.FAKE_MCP_MODE ?? "normal";
const logPath = process.env.FAKE_MCP_LOG;
const pidPath = process.env.FAKE_MCP_PID;
if (pidPath) writeFileSync(pidPath, String(process.pid));

let buffer = "";
const concurrentCalls = [];
const invalidErrorCalls = [];

function log(value) {
  if (logPath) appendFileSync(logPath, `${JSON.stringify(value)}\n`);
}

function send(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

function reply(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function rpcError(id, message) {
  send({ jsonrpc: "2.0", id, error: { code: -32000, message } });
}

log({
  startup: {
    argv: process.argv.slice(2),
    socket: process.env.BRIDGE_MCP_SOCKET,
    token: process.env.BRIDGE_MCP_TOKEN,
  },
});

const callResults = new Map([
  ["one", () => ({ isError: false, content: [{ type: "text", text: "one" }] })],
  ["many", () => ({ isError: false, content: [{ type: "text", text: "first" }, { type: "text", text: "second" }] })],
  ["empty", () => ({ isError: false, content: [] })],
  ["error", () => ({ isError: true, content: [{ type: "text", text: "failed clearly" }] })],
  ["empty_error", () => ({ isError: true, content: [] })],
  ["malformed", () => ({ isError: false, content: [{ type: "text", text: 4 }] })],
  ["unsupported", () => ({ isError: false, content: [{ type: "image", data: "x" }] })],
]);

function callResult(request) {
  const arguments_ = request.params.arguments;
  const selected = callResults.get(arguments_.kind);
  return selected
    ? selected()
    : { isError: false, content: [{ type: "text", text: arguments_.value ?? "ok" }] };
}

const initializeFailures = new Map([
  ["malformed_json", () => process.stdout.write("{not json}\n")],
  ["invalid_envelope", (request) => send({ jsonrpc: "1.0", id: request.id, result: {} })],
  ["both_result_error", (request) => send({ jsonrpc: "2.0", id: request.id, result: {}, error: { code: -1, message: "bad" } })],
  ["malformed_frame", (request) => {
    process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {} }));
    process.stdout.end();
  }],
  ["unknown_id", (request) => reply(request.id + 100, { protocolVersion: "2025-06-18", capabilities: {} })],
  ["invalid_initialize_object", (request) => reply(request.id, [])],
  ["invalid_initialize_protocol", (request) => reply(request.id, { protocolVersion: "", capabilities: {} })],
  ["invalid_initialize_capabilities", (request) => reply(request.id, { protocolVersion: "2025-06-18", capabilities: [] })],
  ["invalid_initialize_server", (request) => reply(request.id, { protocolVersion: "2025-06-18", capabilities: {} })],
  ["oversized_frame", () => process.stdout.write(Buffer.alloc(8 * 1024 * 1024 + 1, "x"))],
  ["invalid_utf8", () => process.stdout.write(Buffer.from([0xff]))],
]);

function handleInitialize(request) {
  const failure = initializeFailures.get(mode);
  if (failure) {
    failure(request);
    return;
  }
  const result = {
    protocolVersion: "2025-06-18",
    capabilities: { tools: {} },
    serverInfo: { name: "fake-build", version: "1" },
  };
  reply(request.id, result);
  if (mode === "duplicate_id") reply(request.id, result);
}

const malformedToolStrategies = new Map([
  ["empty_tools", () => []],
  ["duplicate_tools", (tools) => tools.map((tool, index) => index === 1 ? { ...tool, name: "done" } : tool)],
  ["tool_entry_array", (tools) => tools.map((tool, index) => index === 0 ? [] : tool)],
  ["empty_tool_name", (tools) => tools.map((tool, index) => index === 0 ? { ...tool, name: "" } : tool)],
  ["missing_tool_description", (tools) => tools.map((tool, index) => {
    if (index !== 0) return tool;
    const { description: _description, ...withoutDescription } = tool;
    return withoutDescription;
  })],
  ["array_tool_schema", (tools) => tools.map((tool, index) => index === 0 ? { ...tool, inputSchema: [] } : tool)],
]);

const toolListEffects = new Map([
  ["stderr_saturation", () => process.stderr.write("warning\n".repeat(200000))],
  ["write_failure", () => {
    closeSync(0);
    setInterval(() => {}, 1000);
  }],
]);

function handleToolList(request) {
  const validTools = [
    { name: "done", description: "canonical done", inputSchema: { type: "object", required: ["kind"] } },
    { name: "post_thread_message", description: "canonical post", inputSchema: { type: "object", properties: { kind: { type: "string" } } } },
  ];
  const strategy = malformedToolStrategies.get(mode);
  const tools = strategy ? strategy(validTools) : validTools;
  reply(request.id, { tools });
  toolListEffects.get(mode)?.();
}

const modeCallHandlers = new Map([
  ["child_exit", () => { process.exit(17); return true; }],
  ["timeout", () => true],
  ["invalid_error", (request) => {
    invalidErrorCalls.push(request);
    if (invalidErrorCalls.length === 2) {
      send({
        jsonrpc: "2.0",
        id: invalidErrorCalls[0].id,
        error: { code: "not-an-integer", message: 7 },
      });
    }
    return true;
  }],
]);

const kindCallHandlers = new Map([
  ["rpc_error", (request) => rpcError(request.id, "canonical rpc error")],
  ["concurrent", (request) => {
    concurrentCalls.push(request);
    if (concurrentCalls.length === 2) {
      for (const pending of concurrentCalls.reverse()) reply(pending.id, callResult(pending));
    }
  }],
]);

function handleToolCall(request) {
  if (modeCallHandlers.get(mode)?.(request)) return;
  const selected = kindCallHandlers.get(request.params.arguments.kind);
  if (selected) {
    selected(request);
    return;
  }
  reply(request.id, callResult(request));
}

const requestHandlers = new Map([
  ["initialize", handleInitialize],
  ["notifications/initialized", () => {}],
  ["tools/list", handleToolList],
  ["tools/call", handleToolCall],
]);

function handle(request) {
  log(request);
  requestHandlers.get(request.method)?.(request);
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let newline;
  while ((newline = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, newline);
    buffer = buffer.slice(newline + 1);
    handle(JSON.parse(line));
  }
});

process.stdin.on("end", () => {
  log({ lifecycle: "stdin_end" });
  if (mode === "stubborn_close") {
    process.on("SIGTERM", () => {});
    setInterval(() => {}, 1000);
    return;
  }
  process.exit(0);
});
