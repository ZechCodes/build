#!/usr/bin/env node
// Generate portable route placement and truthful OpenAPI from one explicit map.
// Domain parameters remain validated by the existing Rust typed registry.
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const read = (path) => readFileSync(resolve(root, path), "utf8");
const json = (path) => JSON.parse(read(path));
const bytes = (n) => 4 * Math.ceil(n / 3) + 65536;
const overrides = new Map([
  ["tasks.attach", bytes(5 * 1048576)], ["thread.attach", bytes(5 * 1048576)],
  ["fs.uploadChunk", bytes(4 * 1048576)], ["fs.write", bytes(1048576)],
]);
const eventMethods = new Set([
  "session.hello", "changes.subscribe", "changes.unsubscribe", "writes.subscribe",
  "fs.uploadBegin", "fs.uploadChunk", "fs.uploadFinish", "fs.uploadAbort",
]);
const declarations = new Map();
for (const file of readdirSync(resolve(root, "bridge/src/api/v1")).filter((name) => name.endsWith(".rs"))) {
  const source = read(`bridge/src/api/v1/${file}`);
  // Context-aware methods have the same public params/result contract; the
  // extra caller context is transport metadata, not a domain parameter.
  const expression = /v1_(?:context_)?method!\(\s*"([^"]+)"\s*,\s*\w+\s*,\s*([\w:<>]+)\s*,\s*([\w:<>]+)\s*\)/g;
  for (const match of source.matchAll(expression)) {
    declarations.set(match[1], { source: `bridge/src/api/v1/${file}`, params: match[2], result: match[3] });
  }
}
const legacySources = {
  "agent.start": "bridge/src/app/runtime/agents/endpoints.rs",
  "agent.interrupt": "bridge/src/app/runtime/agents/endpoints.rs",
  "bridge.stats": "bridge/src/app/rpc.rs",
  "ping": "bridge/src/carrier.rs",
  "session.hello": "bridge/src/app/runtime/terminals.rs",
  "term.list": "bridge/src/app/runtime/terminals.rs",
};
const operations = read("bridge/src/http/routes/resources.tsv").split("\n")
  .filter((line) => line && !line.startsWith("#"))
  .map((line) => {
    const [method, verb, path, extra] = line.split("\t");
    if (!path || extra || !["GET", "POST", "PUT", "PATCH", "DELETE"].includes(verb)) throw new Error(`Invalid route: ${line}`);
    const contract = declarations.get(method) || { source: legacySources[method], params: "legacy normalized by execution::Invocation", result: "legacy wire result" };
    if (!contract.source) throw new Error(`No registry declaration for ${method}`);
    const pathParams = [...path.matchAll(/\{(\w+)\}/g)].map((match) => match[1]);
    return {
      method, operationId: method.replaceAll(".", "_"), verb, path, pathParams,
      queryParams: [], body: "remaining", scope: "existing typed method authorization and source selectors",
      requestMaxBytes: overrides.get(method) || 1048576, responseMaxBytes: 8388608,
      requires_events: eventMethods.has(method), contract,
    };
  }).sort((a, b) => a.method.localeCompare(b.method, "en"));
for (const key of ["method", "operationId"]) {
  if (new Set(operations.map((op) => op[key])).size !== operations.length) throw new Error(`Duplicate ${key}`);
}
const keys = operations.map((op) => `${op.verb} ${op.path.replace(/\{\w+\}/g, "{}")}`);
if (new Set(keys).size !== keys.length) throw new Error("Ambiguous resource routes");
const exclusions = [
  ...["agent.attach", "term.create", "term.close", "term.attach", "term.input", "term.resize", "term.ack"].map((method) => ({ method, reason: "terminal control/output uses a separately authenticated WebRTC path" })),
  ...["rtc.offer", "rtc.ice", "rtc.close"].map((method) => ({ method, reason: "authenticated WebRTC signaling only" })),
  ...["stream.start", "stream.events", "stream.state"].map((method) => ({ method, reason: "QA-only; never exported by HTTP" })),
];
const headers = {
  session: "X-Build-Session", subscription: "X-Build-Subscription", requestId: "X-Build-Request-Id",
  priority: "X-Build-Priority", operationId: "Idempotency-Key", after: "X-Build-After",
};
const limitation = "Domain parameter/result JSON Schemas are incomplete: the typed registry has no schema reflection. Rust declaration links are lexically inventoried from v1_method! and v1_context_method!, not derived by Rust semantic analysis; verified wire fixtures and the cargo registry witness remain authoritative. Registered v1 methods use existing strict typed normalization. Six legacy methods preserve unknown/default optional fields and retain object-shape plus existing domain checks; their shared execution extraction is owned by #908. No blanket strict legacy parameter validation is claimed.";
const manifest = {
  version: 1, generatedBy: "scripts/http-resource-manifest.mjs", schemaLimitation: limitation,
  headers, terminal: "webrtc", durable_events: false,
  transport: { resource_manifest_version: 1, operation_ids: operations.map((op) => op.operationId), terminal: "webrtc", durable_events: false },
  operations, exclusions,
};
const replySchema = {
  type: "object", required: ["id", "ok"], properties: {
    id: {type: "string"}, ok: {type: "boolean"}, result: {},
    error: {type: "string"}, error_code: {type: "string"}, retryable: {type: "boolean"}, details: {},
    bridge_epoch: {type: "string"}, write: {},
  }, additionalProperties: true,
};
function paramsExample(op, fixture) {
  const params = { ...fixture.params };
  for (const key of op.pathParams) delete params[key];
  return params;
}
function routeSpec(op) {
  const fixturePath = `fixtures/api/v1/${op.method}.json`;
  let fixture;
  try { fixture = json(fixturePath); } catch (error) {
    if (declarations.has(op.method)) throw error;
  }
  const parameters = op.pathParams.map((name) => ({ name, in: "path", required: true, schema: {type: "string", minLength: 1}, description: "Existing opaque selector; encoded as exactly one path segment." }));
  for (const key of Object.keys(headers)) {
    parameters.push({$ref:`#/components/parameters/${key}${key === "subscription" && op.requires_events ? "Required" : ""}`});
  }
  const examples = fixture ? { wire: {value: {id: "example", ok: true, result: fixture.result}} } : undefined;
  const responses = { "200": {description: "Final existing wire reply, including typed success/error outcome. No blanket durable acknowledgement or retry promise.", content:{"application/json":{schema:{$ref:"#/components/schemas/Reply"}, ...(examples ? {examples} : {})}}} };
  for (const [status, description] of Object.entries({400:"Malformed request or invalid params",401:"Authentication failed",403:"Scope forbidden",404:"Resource not found",405:"Method not allowed",409:"Conflict, stale scope or stream_required/stream_replaced",413:"Request body exceeds this operation's byte cap",426:"Unsupported API version",429:"Adapter quota exceeded",500:"Internal failure",503:"Authority or execution unavailable"})) {
    responses[status] = {$ref:`#/components/responses/Http${status}`};
  }
  return {
    operationId: op.operationId, security: [{deviceBearer:[]}], parameters,
    description: `Maps only to ${op.method}. Remaining JSON fields retain their existing semantics. ${declarations.has(op.method) ? "Registered v1 parameters, including unknown fields, are validated by the existing typed registry." : "Legacy normalization preserves unknown fields and existing default/optional semantics; #908 owns the shared execution extraction and object-shape checks. Domain checks remain with the existing handler; full typed normalization is not claimed."} ${limitation}`,
    "x-build-method": op.method, "x-build-contract": {...op.contract, ...(fixture ? {fixture: fixturePath} : {})},
    "x-build-request-max-bytes":op.requestMaxBytes, "x-build-response-max-bytes":op.responseMaxBytes,
    "x-build-requires-events":op.requires_events,
    ...(op.verb !== "GET" ? {requestBody:{required:true, content:{"application/json":{schema:{type:"object",additionalProperties:true,"x-build-schema-complete":false}, ...(fixture ? {example:paramsExample(op,fixture)} : {})}}}} : {}),
    responses,
  };
}
const headerParameters = {};
for (const [key, name] of Object.entries(headers)) {
  headerParameters[key] = {name,in:"header",required:key === "session",schema:key === "priority" ? {type:"string",enum:["user","background"]} : {type:"string"},description:key === "subscription" ? "Exact active application stream handle. A supplied handle is always validated; stale handles cannot target replacements." : "Existing transport metadata; never inserted into domain params."};
}
headerParameters.subscriptionRequired = {...headerParameters.subscription,required:true};
const errorResponses = {};
for (const [status, description] of Object.entries({400:"Malformed request or invalid params",401:"Authentication failed",403:"Scope forbidden",404:"Resource not found",405:"Method not allowed",409:"Conflict, stale scope or stream_required/stream_replaced",413:"Request body exceeds this operation's byte cap",426:"Unsupported API version",429:"Adapter quota exceeded",500:"Internal failure",503:"Authority or execution unavailable"})) {
  errorResponses[`Http${status}`] = {description, content:{"application/json":{schema:{$ref:"#/components/schemas/Reply"}}}};
}
const paths = {};
for (const op of operations) (paths[op.path] ||= {})[op.verb.toLowerCase()] = routeSpec(op);
const openapi = {openapi:"3.1.0",info:{title:"Build explicit HTTP resources",version:"1",description:limitation},servers:[{url:"https://verified-bridge.invalid",description:"Use only the authenticated configured canonical origin."}],paths,components:{securitySchemes:{deviceBearer:{type:"http",scheme:"bearer",description:"Device-scoped account credential verified by the hosted authority; never a hosted OAuth token."}},schemas:{Reply:replySchema},parameters:headerParameters,responses:errorResponses},"x-build-resource-manifest-version":1,"x-build-lifecycle-owner":"#911 (separate lifecycle/SSE contract)","x-build-no-query-strings":true};
const artifacts = new Map([["fixtures/api/http/resource-manifest.json", manifest], ["fixtures/api/http/openapi.json",openapi]]);
let stale = false;
for (const [path, data] of artifacts) {
  const content = JSON.stringify(data, null, 2) + "\n";
  if (process.argv.includes("--check")) {
    if (read(path) !== content) { console.error(`stale: ${path}`); stale = true; }
  } else writeFileSync(resolve(root,path),content);
}
if (stale) process.exitCode = 1;
else console.log(`${operations.length} explicit resource routes; generated artifacts ${process.argv.includes("--check") ? "current" : "written"}`);
