// Synthetic provider and event recorder for the opt-in, real Pi TUI probe.
// The actual Build default export owns its hooks, MCP client and file sink.
import { appendFileSync, existsSync } from "node:fs";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";
import buildTools from "./build-tools.ts";

const record = (event) => appendFileSync(process.env.QUAL_EVENTS, `${JSON.stringify(event)}\n`);
const phase = (name) => existsSync(`${process.env.QUAL_HOME}/${name}`);
const scenario = process.env.QUAL_SCENARIO;
let requests = 0;
let toolCalled = false;

function stream(model) {
  const output = createAssistantMessageEventStream();
  const overflow = scenario === "overflow" && requests === 1;
  const callTool = scenario === "mcp_failure" && phase("fail-mcp") && !toolCalled;
  requests += 1;
  toolCalled ||= callTool;
  const message = {
    role: "assistant", api: model.api, provider: model.provider, model: model.id,
    content: callTool
      ? [{type: "toolCall", id: "qualification-tool", name: "done", arguments: {kind: "child_exit"}}]
      : [{type: "text", text: "QUALIFICATION_OK"}],
    usage: {input: 50000, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 50001,
      cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0}},
    stopReason: overflow ? "error" : callTool ? "toolUse" : "stop",
    timestamp: Date.now(),
  };
  if (overflow) message.errorMessage = "prompt is too long: 70000 tokens > 64000 maximum";
  queueMicrotask(() => {
    output.push(overflow ? {type: "error", reason: "error", error: message}
      : {type: "done", reason: message.stopReason, message});
    output.end();
  });
  record({type: "provider_response", request: requests, stopReason: message.stopReason});
  return output;
}

export default async function qualification(pi) {
  pi.registerProvider("qualification", {
    baseUrl: "http://invalid.invalid", apiKey: "synthetic-placeholder", api: "qualification",
    models: [{id: "local", name: "Local qualification", reasoning: false, input: ["text"],
      cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0}, contextWindow: 64000, maxTokens: 1024}],
    streamSimple: stream,
  });
  for (const type of ["session_start", "session_shutdown", "agent_start", "agent_end", "agent_settled",
    "session_before_compact", "session_compact", "session_compact_failed"]) {
    pi.on(type, (event, ctx) => record({type, reason: event.reason, aborted: event.aborted,
      willRetry: event.willRetry, idle: ctx.isIdle(), pending: ctx.hasPendingMessages(),
      usage: ctx.getContextUsage()}));
  }
  await buildTools(pi);
  pi.on("session_before_compact", (event) => ({compaction: {
    summary: "Synthetic qualification summary.",
    firstKeptEntryId: event.preparation.firstKeptEntryId,
    tokensBefore: event.preparation.tokensBefore,
  }}));
}
