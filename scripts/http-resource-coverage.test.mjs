// Run from any directory: node --test scripts/http-resource-coverage.test.mjs
// This scans consumers independently of the Rust registry: intersecting the
// two would silently discard legacy calls and newly introduced methods.
import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = new URL("../", import.meta.url);
const methodShape = /^(?:ping|[a-z][\w]*(?:\.[A-Za-z][\w]*)+)$/;
const resourceNamespace = /^(?:agent|archived|board|branch|bridge|capture|changes|conversation|conversations|entity|fs|git|github|harnesses|messages|models|project|push|rtc|run|session|settings|sources|stream|task|tasks|term|thread|threads|user|workspace|workspaces|writes)\./;

// These are feature flags, push types, or diagnostic labels, not dispatch
// methods. Exceptions are scoped to the source that uses them as metadata.
// A same-named literal passed directly to an RPC helper is still a consumer.
const metadata = {
  "connection.js": ["term.liveness"],
  "core/changeEvents.js": ["harnesses.changed", "models.changed", "tasks.waitingOn", "writes.confirmed"],
  "core/peerLink.js": ["rtc.diagnostics"],
  "core/sessionRpc.js": ["writes.confirmed"],
  "core/writeOperationEffects.js": ["writes.confirmed"],
  "core/writeOperationIntent.js": ["writes.confirmed"],
  "core/writeOperations.js": ["bridge.epoch", "writes.confirmed"],
  "core/bridgeEpoch.js": ["bridge.epoch"],
  "core/syncDiagnostics.js": ["conversation.get", "conversation.list", "messages.get", "messages.list", "project.agents", "tasks.review.list", "thread.get", "thread.list", "thread.read"],
  "terminal/session.js": ["term.closed", "term.output", "term.reset"],
  "core/bridgeApi/v1/writeMethods.js": ["stream.start"],
  "core/bridgeApi/v1/index.js": [
    "bridge.epoch", "bridge.restarting", "changes.projectSources", "changes.subscriptions",
    "conversations.settings", "fs.mediaRawPages", "fs.projectSourcePath", "fs.projectSources",
    "git.projectSources", "harnesses.changed", "messages.context", "models.changed",
    "rtc.clientLanCache", "rtc.diagnostics", "sources.syncBase", "sources.syncBasePath",
    "tasks.assignProjectAgent", "tasks.attachments", "tasks.bodyPrecondition", "tasks.commentUserNotifies",
    "tasks.context", "tasks.doneSinceLeft", "tasks.listPaged", "tasks.review.pullRequests",
    "tasks.reviewComments", "tasks.waitingOn", "tasks.watching", "term.closed", "term.output",
    "term.projectSources", "term.reset", "thread.attachmentChunks", "threads.leanActivity",
    "threads.postOperations", "writes.confirmed",
  ],
};

// Pin method-producing expressions and dynamic dispatch arguments separately
// from literal collection. New/changed dynamic sites must be reviewed rather
// than disappearing because their first argument is not a quoted string.
const dynamicSites = {
  "connection.js": [
    "context . rpc ( ... asked )",
    "session . call ( method )"
  ],
  "core/adoption.js": [
    "call ( method )",
    "runCall ( method )"
  ],
  "core/agentRail.js": [
    ". currentCall ( ) ( method )",
    ". currentCall ( ) ( method )",
    ". currentCall ( ) ( method )",
    "context . call ( ... args )"
  ],
  "core/bridgeApi/v1/index.js": [
    "call ( ... args )"
  ],
  "core/bridgeUpdatePanel.js": [
    "callRpc ( method )"
  ],
  "core/cacheSync.js": [
    "ask ( request . method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "callWhileActive ( method )",
    "context . call ( method )",
    "context . call ( verb )",
    "projectTerminalRpc ( method )"
  ],
  "core/changeEvents.js": [
    "call ( method )"
  ],
  "core/changesReview.js": [
    ") . call ( view )"
  ],
  "core/chatRepository.js": [
    ". currentCall ( ) ( submission . threadPostOperations . statusMethod )",
    "mutate ( fresh )"
  ],
  "core/compactionCache.js": [
    "context . call ( job . method )"
  ],
  "core/composeView.js": [
    "homeCall ( method )"
  ],
  "core/conversationCompaction.js": [
    "ask ( entityId )"
  ],
  "core/deviceContexts.js": [
    "? transport ( ) ( ... asked )",
    "call ( ... asked )",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )",
    "return transport ( ) ( ... asked )"
  ],
  "core/filesUploadActions.js": [
    "callRpc ( entry . method )"
  ],
  "core/filesUploads.js": [
    "fileUploadRpc ( contextFor ( deviceId ) )"
  ],
  "core/fileUploadRpc.js": [
    "callRpc ( method )"
  ],
  "core/fileUploads.js": [
    "context . rpc ( method )"
  ],
  "core/gitPane.js": [
    ") . call ( view )",
    "callRpc ( `git.${kind}` )",
    "callRpc ( method )",
    "readCheckoutRecord ( kind )",
    "readCheckoutRecord ( kind )",
    "syncActionRpc ( optionId )"
  ],
  "core/inboxDevices.js": [
    "context . rpc ( ... asked )"
  ],
  "core/isolation.js": [
    "callRpc ( target . rpc )"
  ],
  "core/optimistic.js": [
    "call ( handle )"
  ],
  "core/pathProbe.js": [
    "ask ( method )"
  ],
  "core/projectChangesRpc.js": [
    "context . rpc ( method )"
  ],
  "core/projectConsole.js": [
    "mutate ( held )",
    "mutate ( held ?. tabs || [ ] )"
  ],
  "core/projectFilesRpc.js": [
    "context . rpc ( method )",
    "context . rpc ( method )",
    "context . rpc ( method )"
  ],
  "core/projectTerminalRpc.js": [
    "call ( method )"
  ],
  "core/scopedCompactionEditor.js": [
    "callRpc ( scopes [ scope ] . method )"
  ],
  "core/session.js": [
    "adapter . call ( method )",
    "call ( start )",
    "createSessionRpc ( { transport , ... minted , bridgeEpoch , noCarrier : ( ) => new Error ( \"your device went offline\" ) , } )",
    "greetingGate . call ( ( ) => { const refusal = compactionWriteError ( method , params , adapter ?. capabilities . compaction ?. harnessThresholds ) ; if ( refusal ) throw new ApiError ( refusal . code , refusal . message ) ; return adapter . call ( method , params , options ) ; } )",
    "rpc . call ( method )"
  ],
  "core/sessionRpc.js": [
    "call ( method )"
  ],
  "core/syncLists.js": [
    "ask ( context )"
  ],
  "core/taskReviewCache.js": [
    "mutate ( verb )",
    "mutateRpc ( `tasks.review.${verb}` )",
    "reviewMutationRpc ( scope . deviceId )"
  ],
  "core/taskReviewControls.js": [
    "repository . mutate ( verb )"
  ],
  "core/taskReviewLifecycle.js": [
    "repository . mutate ( verb )"
  ],
  "core/taskReviewSupport.js": [
    "callRpc ( ... asked )",
    "callRpc ( ... asked )",
    "context . rpc ( ... asked )",
    "reviewRpc ( context )"
  ],
  "core/taskWaitingOnActions.js": [
    "call ( method )"
  ],
  "core/trackerPages.js": [
    "ask ( asked )"
  ],
  "core/trackerTaskPage.js": [
    "state . callRpc ( ... args )",
    "state . callRpc ( method )",
    "state . callRpc ( method )",
    "state . callRpc ( method )",
    "state . callRpc ( method )"
  ],
  "core/watchedTaskFollower.js": [
    "context . rpc ( method )"
  ],
  "core/watchToggle.js": [
    "call ( watchingNow ? request . watch : request . unwatch )"
  ],
  "core/workspaceReviewEntry.js": [
    "context . rpc ( ... asked )"
  ],
  "core/writeOperations.js": [
    "call . call ( method )",
    "raw ( method )",
    "raw ( nextMethod )",
    "raw ( row . request . method )"
  ],
  "sheets/workspaceSettings.js": [
    "callRpc ( method )"
  ],
  "terminal/manager.js": [
    "call ( method )",
    "projectTerminalRpc ( method )"
  ],
  "terminal/session.js": [
    "_call ( method )",
    "_userCall ( method )",
    "call ( method )",
    "createSessionRpc ( { transport : this . transport , sessionId , sessionKeyB64 , deviceId , bridgeEpoch , noCarrier : ( ) => new TerminalSocketLost ( this . _closed ? \"closed\" : \"disconnected\" ) , } )",
    "rpc . call ( method )",
    "this . _call ( verb )",
    "this . _userTerminalRpc ( this . deviceId )"
  ],
  "views/deviceProjects.js": [
    "creationCall ( chosenId )"
  ],
  "views/deviceSettings.js": [
    "session . call ( ... asked )"
  ],
  "views/projectChanges.js": [
    "projectChangesRpc ( context )"
  ],
  "views/projectView.js": [
    "projectFilesRpc ( state . context )"
  ],
  "views/workspaceView.js": [
    "context . rpc ( method )",
    "context . rpc ( method )",
    "workspaceResultRpc ( state )"
  ],
  "views/worktreeReview.js": [
    "adopting . runCall ( method )"
  ]
};

const helper = /^(?:rpc|call|callRpc|ask|signal|callWhileActive|askBridge|homeCall|targetCall|askDevice|creationCall|mutateRpc|mutate|readPages|readCheckoutRecord|raw|runCall|_call|_userCall)$/;

// A small lexical scanner, not a JavaScript AST: strings, template expression
// bodies, comments and regexes are handled so HTML/comments cannot invent call
// sites. Template expressions get their own token stream for nested calls.
function scan(source) {
  const streams = [], literals = [];
  let cursor = 0;
  const pushString = (tokens, start, quote) => {
    cursor++;
    let value = "";
    while (cursor < source.length && source[cursor] !== quote) {
      if (source[cursor] === "\\") {
        cursor++;
        const escape = source[cursor++];
        if (escape === "u" || escape === "x") {
          const length = escape === "u" ? 4 : 2;
          value += String.fromCharCode(parseInt(source.slice(cursor, cursor + length), 16));
          cursor += length;
        } else value += ({ n: "\n", r: "\r", t: "\t" })[escape] ?? escape;
      } else value += source[cursor++];
    }
    cursor++;
    const token = { kind: "string", value, start, end: cursor };
    tokens.push(token); literals.push(token);
  };
  function code(nested = false) {
    const tokens = [];
    streams.push(tokens);
    let braces = 0;
    while (cursor < source.length) {
      const start = cursor, char = source[cursor], next = source[cursor + 1];
      if (/\s/.test(char)) { cursor++; continue; }
      if (char === "/" && next === "/") {
        cursor = source.indexOf("\n", cursor);
        if (cursor < 0) cursor = source.length;
        continue;
      }
      if (char === "/" && next === "*") {
        const end = source.indexOf("*/", cursor + 2);
        assert.notEqual(end, -1, "unterminated source comment");
        cursor = end + 2; continue;
      }
      if (char === '"' || char === "'") { pushString(tokens, start, char); continue; }
      if (char === "`") {
        cursor++;
        let dynamic = false;
        while (cursor < source.length && source[cursor] !== "`") {
          if (source[cursor] === "\\") { cursor += 2; continue; }
          if (source[cursor] === "$" && source[cursor + 1] === "{") {
            dynamic = true; cursor += 2; code(true); continue;
          }
          cursor++;
        }
        cursor++;
        const raw = source.slice(start, cursor);
        const token = { kind: dynamic ? "template" : "string", value: dynamic ? raw : raw.slice(1, -1), start, end: cursor };
        tokens.push(token);
        if (!dynamic) literals.push(token);
        continue;
      }
      // A regex can contain quotes or comment-looking characters. Recognize
      // expression-start positions used by this source tree.
      const previous = tokens.at(-1)?.value;
      if (char === "/" && (!previous || /^(?:[=(,:!\[?{;]|return|=>|&&|\|\|)$/.test(previous))) {
        cursor++;
        let inClass = false;
        while (cursor < source.length) {
          const part = source[cursor++];
          if (part === "\\") { cursor++; continue; }
          if (part === "[") inClass = true;
          if (part === "]") inClass = false;
          if (part === "/" && !inClass) break;
        }
        while (/[a-z]/i.test(source[cursor] || "")) cursor++;
        tokens.push({ kind: "regex", value: source.slice(start, cursor), start, end: cursor });
        continue;
      }
      if (nested && char === "}" && braces === 0) { cursor++; return; }
      if (char === "{") braces++;
      if (char === "}") braces--;
      const word = /^[A-Za-z_$][\w$]*/.exec(source.slice(cursor));
      const operator = /^(?:\.\.\.|=>|\?\.|&&|\|\||===|!==|==|!=)/.exec(source.slice(cursor));
      const value = word?.[0] || operator?.[0] || char;
      cursor += value.length;
      tokens.push({ kind: word ? "word" : "symbol", value, start, end: cursor });
    }
  }
  code();
  return { streams, literals };
}

function firstArgument(tokens, opening, position = 0) {
  const argument = [];
  let depth = 0;
  for (let index = opening + 1; index < tokens.length; index++) {
    const token = tokens[index];
    if (!depth && token.value === ")") break;
    if (!depth && token.value === ",") {
      if (!position) break;
      position--; argument.length = 0; continue;
    }
    if (["(", "[", "{"].includes(token.value)) depth++;
    if ([")", "]", "}"].includes(token.value)) depth--;
    argument.push(token);
  }
  return argument;
}

function consumers(source, file) {
  const { streams, literals } = scan(source);
  const methods = new Set(literals.filter(({ value }) => value === "ping" || resourceNamespace.test(value) && methodShape.test(value)).map(({ value }) => value));
  for (const value of metadata[file] || []) methods.delete(value);
  const dynamic = [];
  for (const tokens of streams) {
    for (let index = 0; index < tokens.length; index++) {
      const token = tokens[index];
      if (token.value !== "(") continue;
      const name = tokens[index - 1]?.value;
      const returnedCall = name === ")" && tokens[index - 2]?.value === "(";
      const reviewSubmit = name === "submit" && ["core/taskReviewControls.js", "core/taskReviewLifecycle.js"].includes(file);
      const selected = helper.test(name || "") || /Rpc$/.test(name || "") || returnedCall || reviewSubmit;
      if (!selected || tokens[index - 2]?.value === "function") continue;
      const afterContext = ["callWhileActive", "askBridge", "raw", "projectTerminalRpc"].includes(name)
        || name === "ask" && ["core/cacheSync.js", "core/syncTaskPhases.js"].includes(file)
        || name === "call" && tokens[index - 3]?.value === "call";
      const argument = firstArgument(tokens, index, Number(afterContext));
      if (!argument.length) continue;
      if (argument.length === 1 && argument[0].kind === "string") {
        const value = name === "mutate" || reviewSubmit ? `tasks.review.${argument[0].value}` : argument[0].value;
        if (methodShape.test(value)) methods.add(value);
        continue;
      }
      const member = [".", "?."].includes(tokens[index - 2]?.value);
      const before = tokens.slice(Math.max(0, index - (returnedCall ? 4 : member ? 3 : 1)), index).map(({ value }) => value).join(" ");
      dynamic.push(`${before} ( ${argument.map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ")} )`);
    }
  }
  return { methods, dynamic: dynamic.sort() };
}

async function sources(directory = new URL("spa/src/", root), prefix = "") {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = new URL(entry.name, directory);
    if (entry.isDirectory()) files.push(...await sources(new URL(`${entry.name}/`, directory), `${prefix}${entry.name}/`));
    else if (entry.name.endsWith(".js")) files.push({ file: `${prefix}${entry.name}`, source: await readFile(file, "utf8") });
  }
  return files.sort((left, right) => left.file.localeCompare(right.file));
}

test("literal collection finds unknown methods without a registry intersection", () => {
  const found = consumers('context.rpc("new_family.future"); callRpc("agent.start"); context.call("thread.page");', "synthetic.js");
  assert.deepEqual([...found.methods].sort(), ["agent.start", "new_family.future", "thread.page"]);
});

test("comments and regexes do not invent consumers; nested template calls do", () => {
  const found = consumers('// rpc("fake.comment")\nconst r = /"fake.regex"/; const html = `x ${rpc("capture.get")}`;', "synthetic.js");
  assert.deepEqual([...found.methods], ["capture.get"]);
  assert.notDeepEqual(consumers('rpc(`git.${kind}`)', "synthetic.js").dynamic, consumers('rpc(`git.${other}`)', "synthetic.js").dynamic);
});

test("review suffix dispatch expands new literal verbs instead of dropping them", () => {
  assert.ok(consumers('repository.mutate("future_action", params)', "synthetic.js").methods.has("tasks.review.future_action"));
  assert.ok(consumers('submit("future_action", params)', "core/taskReviewLifecycle.js").methods.has("tasks.review.future_action"));
});

test("git template dispatch retains its reviewed status/log expansion", async () => {
  const source = await readFile(new URL("spa/src/core/gitPane.js", root), "utf8");
  const normalized = scan(source).streams[0].map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ");
  assert.ok(normalized.includes('forceRefresh = async ( kinds = [ "status" , "log" ] , checkOnly = false ) =>'), "Review the new default git kind expansion");
  assert.ok(normalized.includes('[ "status" , "log" ] . map ( ( kind ) => kinds . includes ( kind ) ? readCheckoutRecord ( kind ) : null )'), "Review the new dynamic git kind expansion");
});

test("greeting-selected operation status retains the reviewed thread.operation contract", async () => {
  const source = await readFile(new URL("spa/src/core/chatRepository.js", root), "utf8");
  const normalized = scan(source).streams[0].map(({ kind, value }) => kind === "string" ? JSON.stringify(value) : value).join(" ");
  assert.ok(normalized.includes('const statusMethod = typeof offered ?. status_method === "string" && offered . status_method ? offered . status_method : "thread.operation" ;'), "Review the operation-status method producer and expand coverage for any new contract");
  const greeting = await readFile(new URL("bridge/src/app/runtime/terminals.rs", root), "utf8");
  assert.match(greeting, /"status_method"\s*:\s*"thread\.operation"/, "Expand coverage when the bridge advertises a different operation status method");
});

test("every SPA resource reference is mapped or explicitly excluded", async () => {
  const manifestPath = new URL("fixtures/api/http/resource-manifest.json", root);
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.ok(Array.isArray(manifest.operations), `${fileURLToPath(manifestPath)}: operations must be an array`);
  assert.ok(Array.isArray(manifest.exclusions), "manifest exclusions must be an array");
  const covered = new Set([...manifest.operations, ...manifest.exclusions].map((operation) => operation.method));
  const missing = [];
  for (const { file, source } of await sources()) {
    for (const method of consumers(source, file).methods) if (!covered.has(method)) missing.push(`${file}: ${method}`);
  }
  assert.deepEqual(missing.sort(), [], "Add resource routes or deliberate documented exclusions for these consumers");
});

test("dynamic method dispatch sites remain explicitly accounted for", async () => {
  const actual = {};
  for (const { file, source } of await sources()) {
    const { dynamic } = consumers(source, file);
    if (dynamic.length) actual[file] = dynamic;
  }
  assert.deepEqual(actual, dynamicSites, "Review new/changed dynamic sites and their concrete method expansion before updating this inventory");
});
