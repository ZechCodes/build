import { spawn } from "node:child_process";
import { isAbsolute } from "node:path";

const DEFAULT_TIMEOUT_MS = 30000;
const CLOSE_GRACE_MS = 500;
const MAX_FRAME_BYTES = 8 * 1024 * 1024;
const EMPTY_TOOL_ERROR = "Build tool failed without an error message";

function requiredEnvironment(name) {
  const value = process.env[name];
  if (!value) throw new Error(`Build Pi extension requires ${name}`);
  return value;
}

function runtimeConfiguration() {
  const command = requiredEnvironment("BUILD_PI_MCP_COMMAND");
  if (!isAbsolute(command)) throw new Error("BUILD_PI_MCP_COMMAND must be absolute");
  const owner = requiredEnvironment("BUILD_PI_MCP_OWNER");
  requiredEnvironment("BRIDGE_MCP_SOCKET");
  requiredEnvironment("BRIDGE_MCP_TOKEN");
  const configuredTimeout = Number(process.env.BUILD_PI_MCP_TIMEOUT_MS ?? DEFAULT_TIMEOUT_MS);
  if (!Number.isInteger(configuredTimeout) || configuredTimeout < 1) {
    throw new Error("BUILD_PI_MCP_TIMEOUT_MS must be a positive integer");
  }
  return { command, owner, timeoutMs: configuredTimeout };
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isNonEmptyString(value) {
  return typeof value === "string" && value.length > 0;
}

function validateTool(tool) {
  if (!isObject(tool)) {
    throw new Error("tools/list entries must be objects");
  }
  if (!isNonEmptyString(tool.name)) {
    throw new Error("tools/list names must be unique non-empty strings");
  }
  if (typeof tool.description !== "string") {
    throw new Error(`tool ${tool.name} has no string description`);
  }
  if (!isObject(tool.inputSchema)) {
    throw new Error(`tool ${tool.name} has no object inputSchema`);
  }
  return tool;
}

function validateToolList(result) {
  if (!isObject(result) || !Array.isArray(result.tools)) {
    throw new Error("tools/list result must contain a tools array");
  }
  if (result.tools.length === 0) {
    throw new Error("tools/list returned no tools");
  }
  const names = new Set();
  return result.tools.map((entry) => {
    const tool = validateTool(entry);
    if (names.has(tool.name)) {
      throw new Error("tools/list names must be unique non-empty strings");
    }
    names.add(tool.name);
    return tool;
  });
}

function validateInitialize(result) {
  if (!isObject(result)) {
    throw new Error("initialize result must be an object");
  }
  if (!isNonEmptyString(result.protocolVersion)) {
    throw new Error("initialize result has no protocolVersion");
  }
  if (!isObject(result.capabilities)) {
    throw new Error("initialize result has no capabilities object");
  }
  if (!isObject(result.serverInfo)
      || !isNonEmptyString(result.serverInfo.name)
      || !isNonEmptyString(result.serverInfo.version)) {
    throw new Error("initialize result has no valid serverInfo");
  }
}

function textFromContentBlock(block) {
  if (!isObject(block) || block.type !== "text" || typeof block.text !== "string") {
    throw new Error("Build tool returned an unsupported content block");
  }
  return block.text;
}

function convertToolResult(result) {
  if (!isObject(result) || typeof result.isError !== "boolean" || !Array.isArray(result.content)) {
    throw new Error("tools/call result must contain boolean isError and a content array");
  }
  const text = result.content.map(textFromContentBlock).join("\n");
  if (result.isError) throw new Error(text || EMPTY_TOOL_ERROR);
  return { content: [{ type: "text", text }], details: {} };
}

function validateResponseError(error) {
  if (!isObject(error)) {
    throw new Error("Build MCP response has an invalid JSON-RPC error");
  }
  if (!Number.isInteger(error.code) || typeof error.message !== "string") {
    throw new Error("Build MCP response has an invalid JSON-RPC error");
  }
}

function parseResponseEnvelope(line, pendingIds) {
  let response;
  try {
    response = JSON.parse(line);
  } catch (error) {
    throw new Error(`Build MCP emitted malformed JSON: ${errorMessage(error)}`);
  }
  if (!isObject(response) || response.jsonrpc !== "2.0") {
    throw new Error("Build MCP emitted an invalid JSON-RPC envelope");
  }
  if (!Number.isInteger(response.id) || !pendingIds.has(response.id)) {
    throw new Error(`Build MCP emitted an unknown response id: ${String(response.id)}`);
  }
  const hasResult = Object.prototype.hasOwnProperty.call(response, "result");
  const hasError = Object.prototype.hasOwnProperty.call(response, "error");
  if (hasResult === hasError) {
    throw new Error("Build MCP response must contain exactly one of result or error");
  }
  if (hasError) validateResponseError(response.error);
  return response;
}

class McpStdioClient {
  constructor(command, owner, timeoutMs) {
    this.command = command;
    this.owner = owner;
    this.timeoutMs = timeoutMs;
    this.nextId = 1;
    this.pending = new Map();
    this.failure = undefined;
    this.closing = false;
    this.closePromise = undefined;
    this.frame = "";
    this.writeChain = Promise.resolve();
    this.decoder = new TextDecoder("utf-8", { fatal: true });
  }

  async start() {
    this.child = spawn(this.command, ["mcp", "--task", this.owner], {
      env: process.env,
      shell: false,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.exitPromise = new Promise((resolve) => this.child.once("close", resolve));
    this.child.stderr.on("data", () => {});
    this.child.stdin.on("error", (error) => {
      if (!this.closing) this.failTerminal(`Build MCP write failed: ${errorMessage(error)}`);
    });
    this.child.stdout.on("data", (chunk) => this.readChunk(chunk));
    this.child.stdout.on("end", () => this.readEnd());
    this.child.once("exit", (code, signal) => {
      if (!this.closing) this.failTerminal(`Build MCP child exited (${signal ?? code ?? "unknown"})`);
    });
    await new Promise((resolve, reject) => {
      this.child.once("spawn", resolve);
      this.child.once("error", reject);
    });
  }

  async initialize() {
    const initialized = await this.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "build-pi-extension", version: "1" },
    });
    validateInitialize(initialized);
    await this.notify("notifications/initialized", {});
    return validateToolList(await this.request("tools/list", {}));
  }

  request(method, params) {
    if (this.failure) return Promise.reject(this.failure);
    if (this.closing) return Promise.reject(new Error("Build MCP session is shutting down"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.failTerminal(`Build MCP request ${id} timed out`), this.timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.write({ jsonrpc: "2.0", id, method, params }).catch((error) => {
        this.failTerminal(`Build MCP write failed: ${errorMessage(error)}`);
      });
    });
  }

  notify(method, params) {
    if (this.failure) return Promise.reject(this.failure);
    if (this.closing) return Promise.reject(new Error("Build MCP session is shutting down"));
    return this.write({ jsonrpc: "2.0", method, params });
  }

  write(message) {
    const line = `${JSON.stringify(message)}\n`;
    this.writeChain = this.writeChain.then(() => new Promise((resolve, reject) => {
      if (!this.child?.stdin?.writable) {
        reject(new Error("Build MCP stdin is closed"));
        return;
      }
      this.child.stdin.write(line, (error) => error ? reject(error) : resolve());
    }));
    return this.writeChain;
  }

  readChunk(chunk) {
    if (this.failure || this.closing) return;
    try {
      this.frame += this.decoder.decode(chunk, { stream: true });
    } catch (error) {
      this.failTerminal(`Build MCP stdout is not UTF-8: ${errorMessage(error)}`);
      return;
    }
    if (Buffer.byteLength(this.frame) > MAX_FRAME_BYTES) {
      this.failTerminal("Build MCP response exceeded the framing limit");
      return;
    }
    let newline;
    while ((newline = this.frame.indexOf("\n")) !== -1) {
      const line = this.frame.slice(0, newline);
      this.frame = this.frame.slice(newline + 1);
      if (line.length === 0) {
        this.failTerminal("Build MCP emitted an empty frame");
        return;
      }
      this.readLine(line);
      if (this.failure) return;
    }
  }

  readEnd() {
    if (this.closing || this.failure) return;
    try {
      this.frame += this.decoder.decode();
    } catch (error) {
      this.failTerminal(`Build MCP stdout is not UTF-8: ${errorMessage(error)}`);
      return;
    }
    if (this.frame.length !== 0) {
      this.failTerminal("Build MCP stdout ended with a malformed frame");
      return;
    }
    this.failTerminal("Build MCP stdout ended unexpectedly");
  }

  readLine(line) {
    let response;
    try {
      response = parseResponseEnvelope(line, new Set(this.pending.keys()));
    } catch (error) {
      this.failTerminal(errorMessage(error));
      return;
    }
    const pending = this.pending.get(response.id);
    this.pending.delete(response.id);
    clearTimeout(pending.timer);
    if (Object.prototype.hasOwnProperty.call(response, "error")) {
      pending.reject(new Error(response.error.message));
      return;
    }
    pending.resolve(response.result);
  }

  failTerminal(message) {
    if (this.failure || this.closing) return;
    this.failure = new Error(message);
    this.closing = true;
    this.#rejectPending(this.failure);
    void this.stopChild().finally(() => process.kill(process.pid, "SIGTERM"));
  }

  close() {
    if (this.closePromise) return this.closePromise;
    this.closing = true;
    this.#rejectPending(new Error("Build MCP session is shutting down"));
    this.closePromise = this.stopChild();
    return this.closePromise;
  }

  #rejectPending(error) {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
  }

  async stopChild() {
    if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) {
      if (this.exitPromise) await this.exitPromise;
      return;
    }
    this.child.stdin?.end();
    if (await this.exitsWithin(CLOSE_GRACE_MS)) return;
    this.child.kill("SIGTERM");
    if (await this.exitsWithin(CLOSE_GRACE_MS)) return;
    this.child.kill("SIGKILL");
    await this.exitPromise;
  }

  async exitsWithin(milliseconds) {
    return Promise.race([
      this.exitPromise.then(() => true),
      wait(milliseconds).then(() => false),
    ]);
  }
}

export default async function buildTools(pi) {
  let client;
  try {
    const { command, owner, timeoutMs } = runtimeConfiguration();
    client = new McpStdioClient(command, owner, timeoutMs);
    await client.start();
    const tools = await client.initialize();
    for (const tool of tools) {
      pi.registerTool({
        name: tool.name,
        label: tool.name,
        description: tool.description,
        parameters: tool.inputSchema,
        async execute(_toolCallId, parameters) {
          return convertToolResult(await client.request("tools/call", {
            name: tool.name,
            arguments: parameters,
          }));
        },
      });
    }
    pi.on("session_before_switch", () => ({ cancel: true }));
    pi.on("session_before_fork", () => ({ cancel: true }));
    pi.on("session_shutdown", () => client.close());
  } catch (error) {
    if (client) await client.close();
    throw new Error(`Build Pi extension initialization failed: ${errorMessage(error)}`, {
      cause: error,
    });
  }
}
