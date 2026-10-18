import { spawn } from "node:child_process";
import { appendFileSync, constants, closeSync, fstatSync, openSync } from "node:fs";
import { isAbsolute } from "node:path";

const DEFAULT_TIMEOUT_MS = 30000;
const CLOSE_GRACE_MS = 500;
const MAX_FRAME_BYTES = 8 * 1024 * 1024;
const EMPTY_TOOL_ERROR = "Build tool failed without an error message";

/** @typedef {{tokens: number|null, contextWindow: number}|undefined} PiContextUsage */
/** @typedef {{getContextUsage: () => PiContextUsage, isIdle: () => boolean, hasPendingMessages: () => boolean, sessionManager: {getSessionId: () => string}}} PiCompactionContext */
/** @typedef {"session_shutdown"|"session_start"|"agent_start"|"tool_execution_start"|"tool_execution_end"|"turn_end"|"agent_end"|"agent_settled"|"session_before_compact"|"session_compact"|"session_compact_failed"} PiCompactionHook */
/** @typedef {{reason?: string}} PiCompactionEvent */
/** @typedef {(event: PiCompactionEvent, ctx: PiCompactionContext) => void|{cancel: boolean}} PiCompactionHandler */
/** @typedef {{on: (name: PiCompactionHook, handler: PiCompactionHandler) => void}} PiCompactionApi */
/** @typedef {{tokens: number|null, tokens_decimal?: string, enabled: boolean, owner: string, thread_id: string, launch_id: string, log?: string}} PiCompactionPolicy */
/** @typedef {{type: "starting"|"heartbeat"|"shutdown"|"ready"|"working"|"settled"}|{type: "context", tokens: number|null, window: number|null}|{type: "compaction", completed: boolean, reason: string, failed?: boolean}|{type: "unsupported", reason: string}} PiTelemetryEvent */
/** @typedef {PiTelemetryEvent & {owner: string, thread_id: string, launch_id: string, generation: number, sequence: number}} PiTelemetryRecord */

function compactionConfiguration(owner) {
  const encoded = process.env.BUILD_PI_COMPACTION_POLICY;
  const log = process.env.BUILD_PI_TELEMETRY_LOG;
  if (encoded === undefined && log === undefined) return undefined;
  if (encoded === undefined) throw new Error("Build Pi telemetry requires a compaction policy");
  const policy = JSON.parse(encoded);
  return { ...policy, owner, log, ownerMismatch: policy?.owner !== owner };
}

function validateCompactionPolicy(policy) {
  if (!isObject(policy)
      || !validCompactionTokens(policy)
      || typeof policy.enabled !== "boolean"
      || ![policy.owner, policy.thread_id, policy.launch_id].every(isNonEmptyString)) {
    throw new Error("Build Pi extension received invalid compaction policy");
  }
}

function validCompactionTokens(policy) {
  if (Number.isSafeInteger(policy.tokens) && policy.tokens > 0) return true;
  return policy.enabled === false && policy.tokens === null
    && typeof policy.tokens_decimal === "string" && /^[1-9][0-9]{0,19}$/.test(policy.tokens_decimal);
}

function telemetryFileSink(path) {
  if (typeof path !== "string" || !isAbsolute(path)) {
    throw new Error("Build Pi telemetry path must be absolute");
  }
  // Reopen only the private launch file; never follow a replaced symbolic link.
  return (record) => {
    const fd = openSync(path, constants.O_WRONLY | constants.O_APPEND | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
    try {
      const info = fstatSync(fd);
      if (!info.isFile() || (info.mode & 0o077) !== 0) {
        throw new Error("Build Pi telemetry file must be private");
      }
      appendFileSync(fd, `${JSON.stringify(record)}\n`);
    } finally {
      closeSync(fd);
    }
  };
}

function contextValue(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function compactionReason(event) {
  return ["manual", "threshold", "overflow"].includes(event.reason) ? event.reason : "unknown";
}

/** @param {PiCompactionPolicy} configuration @param {(record: PiTelemetryRecord) => void} [sink] @param {(reason: string) => void} [onFailure] */
function compactionTelemetryChannel(configuration, sink, onFailure = () => {}) {
  const { owner, thread_id, launch_id } = configuration;
  const write = sink ?? telemetryFileSink(configuration.log);
  const key = Symbol.for("build.pi.compaction.generations");
  /** @type {Map<string, {generation: number, retire: () => void}>} */
  const registry = globalThis[key] ??= new Map();
  const previous = registry.get(launch_id);
  previous?.retire();
  const generation = (previous?.generation ?? 0) + 1;
  if (!Number.isSafeInteger(generation)) throw new Error("Build Pi telemetry generation exhausted");
  let sequence = 0;
  let active = true;
  /** @type {ReturnType<typeof setInterval>|undefined} */
  let heartbeat;
  /** @param {PiTelemetryEvent} event */
  const emit = (event) => write({ owner, thread_id, launch_id, generation, sequence: ++sequence, ...event });
  const stop = () => { active = false; clearInterval(heartbeat); };
  const unsupported = (reason) => {
    if (!active) return;
    stop();
    const label = ["extension_failed", "session_identity_mismatch"].includes(reason) ? reason : "telemetry_failed";
    try { emit({ type: "unsupported", reason: label }); } catch { /* The independent heartbeat watchdog withdraws support. */ }
    onFailure(label);
  };
  const shutdown = () => {
    if (!active) return;
    try { emit({ type: "shutdown" }); } catch { unsupported("telemetry_failed"); }
    stop();
  };
  registry.set(launch_id, { generation, retire: shutdown });
  try {
    emit({ type: "starting" });
    heartbeat = setInterval(() => {
      try { emit({ type: "heartbeat" }); } catch { unsupported("telemetry_failed"); }
    }, 1000);
    heartbeat.unref();
  } catch { unsupported("telemetry_failed"); }
  return { emit, unsupported, shutdown, active: () => active };
}

// Pi 0.86.1's agent_end and turn_end precede retries/auto-compaction. Agent
// work settles only at agent_settled; manual compaction has separate cleanup.
/**
 * @param {PiCompactionApi} pi
 * @param {PiCompactionPolicy} configuration
 * @param {(record: PiTelemetryRecord) => void} [sink]
 * @param {(reason: string) => void} [onFailure]
 */
export function registerCompactionTelemetry(pi, configuration, sink, onFailure = () => {}) {
  validateCompactionPolicy(configuration);
  const { owner, enabled } = configuration;
  const channel = compactionTelemetryChannel(configuration, sink, onFailure);
  const { emit, unsupported } = channel;
  let workGeneration = 0;
  /** @param {PiCompactionContext} ctx */
  const context = (ctx) => {
    const usage = ctx.getContextUsage();
    emit({ type: "context", tokens: contextValue(usage?.tokens), window: contextValue(usage?.contextWindow) });
  };
  /** @param {PiCompactionHook} name @param {PiCompactionHandler} handler */
  const on = (name, handler) => pi.on(name, (event, ctx) => {
    if (!channel.active()) return undefined;
    try { return handler(event, ctx); } catch { unsupported("telemetry_failed"); }
  });
  /** @type {PiCompactionHandler} */
  const working = (_event, ctx) => {
    workGeneration += 1;
    emit({ type: "working" });
    context(ctx);
  };
  /** @param {PiCompactionContext} ctx */
  const settle = (ctx) => {
    context(ctx);
    if (ctx.isIdle() && !ctx.hasPendingMessages()) emit({ type: "settled" });
  };
  /** @type {PiCompactionHandler} */
  const manualSettled = (event, ctx) => {
    if (event.reason !== "manual") return;
    const generation = workGeneration;
    // Native manual compact clears its compacting flag AFTER session_compact.
    // A macrotask observes that cleanup, without authorizing pending work.
    setTimeout(() => {
      if (!channel.active() || generation !== workGeneration) return;
      try { settle(ctx); } catch { unsupported("telemetry_failed"); }
    }, 0);
  };
  try {
    pi.on("session_shutdown", () => channel.shutdown());
    on("session_start", (_event, ctx) => {
      if (ctx.sessionManager.getSessionId() !== owner) {
        unsupported("session_identity_mismatch");
        return;
      }
      emit({ type: "ready" });
      context(ctx);
    });
    on("agent_start", working);
    on("tool_execution_start", working);
    on("tool_execution_end", (_event, ctx) => context(ctx));
    on("turn_end", (_event, ctx) => context(ctx));
    on("agent_end", (_event, ctx) => context(ctx));
    on("agent_settled", (_event, ctx) => settle(ctx));
    on("session_before_compact", (event, ctx) => {
      context(ctx);
      if (enabled && event.reason === "threshold") return { cancel: true };
      workGeneration += 1;
      emit({ type: "working" });
      emit({ type: "compaction", completed: false, reason: compactionReason(event) });
    });
    on("session_compact", (event, ctx) => {
      emit({ type: "compaction", completed: true, reason: compactionReason(event) });
      context(ctx);
      manualSettled(event, ctx);
    });
    on("session_compact_failed", (event, ctx) => {
      emit({ type: "compaction", completed: false, failed: true, reason: compactionReason(event) });
      context(ctx);
      manualSettled(event, ctx);
    });
  } catch { unsupported("extension_failed"); }
  return channel;
}

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
    this.closePromise = this.stopChild();
    this.onTerminalFailure?.();
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
  let telemetry;
  const configured = process.env.BUILD_PI_COMPACTION_POLICY !== undefined || process.env.BUILD_PI_TELEMETRY_LOG !== undefined;
  try {
    const policy = compactionConfiguration(process.env.BUILD_PI_MCP_OWNER);
    if (policy) {
      telemetry = registerCompactionTelemetry(pi, policy, undefined, () => {
        void client?.close();
      });
      if (policy.ownerMismatch) telemetry.unsupported("extension_failed");
      if (!telemetry.active()) return;
    }
    const { command, owner, timeoutMs } = runtimeConfiguration();
    client = new McpStdioClient(command, owner, timeoutMs);
    client.onTerminalFailure = () => telemetry?.unsupported("extension_failed");
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
    telemetry?.unsupported("extension_failed");
    if (client) await client.close();
    if (configured) return;
    throw new Error(`Build Pi extension initialization failed: ${errorMessage(error)}`, {
      cause: error,
    });
  }
}
