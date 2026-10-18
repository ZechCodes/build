// Credential-free qualification against an explicitly selected Pi 0.86.1 binary.
// Invoke under nice -n 10. This is opt-in; ordinary tests do not launch Pi.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const binary = process.argv[2];
assert.ok(binary?.startsWith('/'), 'pass the absolute pinned Pi 0.86.1 executable');
const root = await mkdtemp(join(tmpdir(), 'build-pi-compaction-qualification-'));
const extension = String.raw`
import { appendFileSync } from 'node:fs';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai';
import { registerCompactionTelemetry } from './build-tools.mjs';
const record = (value) => appendFileSync(process.env.QUAL_EVENTS, JSON.stringify(value) + '\n');
let request = 0;
let failSummary = false;
let queued = false;
export default function (pi) {
  pi.registerProvider('qualification', {
    baseUrl: 'http://invalid.invalid', apiKey: 'synthetic-placeholder', api: 'qualification',
    models: [{id: 'local', name: 'Local fixture', reasoning: false, input: ['text'],
      cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0}, contextWindow: 64000, maxTokens: 1024}],
    streamSimple(model) {
      const stream = createAssistantMessageEventStream();
      const overflow = process.env.QUAL_SCENARIO === 'overflow' && request === 1;
      request += 1;
      const failed = overflow || failSummary;
      const output = {role: 'assistant', content: [{type: 'text', text: 'OK'}],
        api: model.api, provider: model.provider, model: model.id,
        usage: {input: 50000, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 50001,
          cost: {input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0}},
        stopReason: failed ? 'error' : 'stop', timestamp: Date.now()};
      if (failed) output.errorMessage = overflow ? 'prompt is too long: 70000 tokens > 64000 maximum' : 'fixture summary rejected';
      queueMicrotask(() => {
        stream.push(failed ? {type: 'error', reason: 'error', error: output}
          : {type: 'done', reason: 'stop', message: output});
        stream.end();
      });
      return stream;
    },
  });
  for (const type of ['agent_start', 'turn_end', 'agent_end', 'agent_settled', 'session_compact', 'session_compact_failed']) {
    pi.on(type, (event, ctx) => {
      record({type, reason: event.reason, aborted: event.aborted,
        willRetry: event.willRetry, idle: ctx.isIdle(), pending: ctx.hasPendingMessages(), usage: ctx.getContextUsage()});
      if (event.reason === 'manual') setTimeout(() => record({type: 'manual_deferred',
        idle: ctx.isIdle(), pending: ctx.hasPendingMessages()}), 0);
      if (type === 'agent_start' && process.env.QUAL_SCENARIO === 'queued' && !queued) {
        queued = true;
        pi.sendUserMessage('Synthetic queued follow-up.', {deliverAs: 'followUp'});
      }
    });
  }
  pi.on('session_before_compact', (event, ctx) => {
    record({type: event.type, reason: event.reason, willRetry: event.willRetry, idle: ctx.isIdle(),
      usage: ctx.getContextUsage()});
  });
  registerCompactionTelemetry(pi, {owner: process.env.QUAL_OWNER, thread_id: 'thread-qualification',
    launch_id: 'launch-qualification', tokens: 150000, enabled: true},
    (event) => appendFileSync(process.env.QUAL_TELEMETRY, JSON.stringify(event) + '\n'),
    (reason) => { throw new Error(reason); });
  pi.on('session_before_compact', (event) => {
    if (process.env.QUAL_SCENARIO.endsWith('failed')) { failSummary = true; return; }
    return {compaction: {summary: 'Synthetic qualification summary.',
      firstKeptEntryId: event.preparation.firstKeptEntryId,
      tokensBefore: event.preparation.tokensBefore}};
  });
  pi.registerCommand('qualify-compact', {
    handler(_args, ctx) {
      const result = ctx.compact({
        onComplete: () => record({type: 'callback_complete', idle: ctx.isIdle(), pending: ctx.hasPendingMessages(), usage: ctx.getContextUsage()}),
        onError: () => record({type: 'callback_error', idle: ctx.isIdle(), pending: ctx.hasPendingMessages()}),
      });
      record({type: 'compact_return', undefined: result === undefined});
    },
  });
}
`;

async function records(path) {
  try {
    return (await readFile(path, 'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse);
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    throw error;
  }
}

async function waitFor(predicate, child, diagnostics) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    assert.equal(child.exitCode, null, diagnostics());
    await new Promise((done) => setTimeout(done, 20));
  }
  throw new Error(`qualification timed out: ${diagnostics()}`);
}

function assertOverflow(events, diagnostics) {
  const compact = events.findIndex((event) => event.type === 'session_compact' && event.reason === 'overflow');
  const settled = events.findLastIndex((event) => event.type === 'agent_settled');
  assert.ok(compact >= 0 && compact < settled, diagnostics());
  assert.equal(events[compact].willRetry, true);
  assert.ok(events.filter((event) => event.type === 'agent_start').length >= 3, diagnostics());
}

function assertManual(events, scenario) {
  const failed = scenario.endsWith('failed');
  if (!scenario.startsWith('direct')) {
    assert.ok(events.some((event) => event.type === 'compact_return' && event.undefined));
    assert.ok(events.some((event) => event.type === (failed ? 'callback_error' : 'callback_complete')));
  }
  const terminal = events.filter((event) => event.reason === 'manual').at(-1);
  assert.equal(terminal.type, failed ? 'session_compact_failed' : 'session_compact');
  assert.equal(events.at(-1).type, 'manual_deferred');
  assert.equal(events.at(-1).idle, true);
  assert.equal(events.at(-1).pending, false);
  assert.equal(terminal.usage.tokens, failed ? 50001 : null);
  if (failed) assert.ok(!events.some((event) => event.type === 'session_compact'));
}

function assertLifecycle(events, scenario, diagnostics) {
  assert.ok(events.some((event) => event.type === 'session_before_compact' && event.reason === 'threshold'), diagnostics());
  assert.ok(events.some((event) => event.type === 'session_compact_failed' && event.reason === 'threshold' && event.aborted), diagnostics());
  const end = events.findIndex((event) => event.type === 'agent_end');
  const settled = events.findLastIndex((event) => event.type === 'agent_settled');
  assert.ok(end >= 0 && settled > end, diagnostics());
  assert.equal(events[settled].idle, true);
  if (scenario === 'overflow') return assertOverflow(events, diagnostics);
  if (scenario === 'queued') {
    const firstSettled = events.findIndex((event) => event.type === 'agent_settled');
    assert.equal(events.slice(0, firstSettled).filter((event) => event.type === 'turn_end').length, 2);
  }
  assertManual(events, scenario);
}

function assertTelemetry(telemetry, scenario, owner) {
  assert.equal(telemetry[0].type, 'starting');
  assert.ok(telemetry.some((event) => event.type === 'ready'));
  assert.ok(telemetry.every((event, index) => event.owner === owner && event.sequence === index + 1));
  assert.ok(!telemetry.some((event) => event.type === 'unsupported'));
  telemetry = telemetry.filter((event) => event.type !== 'heartbeat');
  assert.equal(telemetry.at(-1).type, 'settled');
  const completed = telemetry.findIndex((event) => event.type === 'compaction' && event.completed);
  if (scenario.endsWith('failed')) {
    assert.equal(completed, -1);
    assert.equal(telemetry.at(-2).tokens, 50001);
  } else {
    assert.ok(completed >= 0);
    assert.equal(telemetry[completed + 1].type, 'context');
    assert.equal(telemetry[completed + 1].tokens, null);
  }
}

async function runScenario(scenario) {
  const home = join(root, scenario);
  const agentDir = join(home, 'agent');
  const eventPath = join(home, 'events.jsonl');
  const telemetryPath = join(home, 'telemetry.jsonl');
  const owner = randomUUID();
  await mkdir(agentDir, {recursive: true});
  await writeFile(join(agentDir, 'settings.json'), JSON.stringify({
    compaction: {enabled: true, keepRecentTokens: 10, reserveTokens: 16384},
    retry: {enabled: false},
  }));
  const child = spawn(binary, ['--mode', 'rpc', '--no-session', '--session-id', owner, '--no-extensions',
    '--extension', join(root, 'extension.ts'), '--no-skills', '--no-prompt-templates',
    '--no-themes', '--no-tools', '--provider', 'qualification', '--model', 'local'], {
    cwd: home,
    env: {HOME: home, PATH: '/usr/bin:/bin', PI_CODING_AGENT_DIR: agentDir, PI_OFFLINE: '1',
      QUAL_EVENTS: eventPath, QUAL_SCENARIO: scenario, QUAL_OWNER: owner, QUAL_TELEMETRY: telemetryPath},
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const closed = new Promise((done) => child.once('close', done));
  let spawnError;
  child.on('error', (error) => { spawnError = error.message; });
  const lines = [];
  let pending = '';
  let stderr = '';
  child.stdout.on('data', (chunk) => {
    pending += chunk;
    const split = pending.split('\n');
    pending = split.pop();
    for (const line of split) { if (line) lines.push(JSON.parse(line)); }
  });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const diagnostics = () => JSON.stringify({scenario, stderr, spawnError, lines: lines.map(({type, command, success, reason, errorMessage}) =>
    ({type, command, success, reason, errorMessage}))});
  const send = (value) => child.stdin.write(`${JSON.stringify(value)}\n`);
  try {
    send({id: 'seed', type: 'prompt', message: 'Synthetic seed context. '.repeat(4000)});
    await waitFor(async () => (await records(eventPath)).some((event) => event.type === 'agent_settled'), child, diagnostics);
    send({id: 'prompt', type: 'prompt', message: 'Synthetic follow-up context. '.repeat(4000)});
    await waitFor(async () => (await records(eventPath)).filter((event) => event.type === 'agent_settled').length >= 2, child, diagnostics);
    if (scenario !== 'overflow') {
      send(scenario.startsWith('direct') ? {id: 'manual', type: 'compact'}
        : {id: 'manual', type: 'prompt', message: '/qualify-compact'});
      await waitFor(async () => (await records(eventPath)).some((event) => event.type === 'manual_deferred'), child, diagnostics);
      await new Promise((done) => setTimeout(done, 150));
    }
    const events = await records(eventPath);
    const telemetry = await records(telemetryPath);
    assertLifecycle(events, scenario, diagnostics);
    assertTelemetry(telemetry, scenario, owner);
    return {scenario, events, telemetry};
  } finally {
    child.kill('SIGTERM');
    const timeout = setTimeout(() => child.kill('SIGKILL'), 2000);
    await closed;
    clearTimeout(timeout);
  }
}

try {
  await writeFile(join(root, 'extension.ts'), extension);
  const buildExtension = fileURLToPath(new URL('../bridge/src/harness/build-tools.ts', import.meta.url));
  await writeFile(join(root, 'build-tools.mjs'), await readFile(buildExtension));
  const evidence = [];
  for (const scenario of ['manual', 'overflow', 'failed', 'direct_manual', 'direct_failed', 'queued']) evidence.push(await runScenario(scenario));
  console.log(JSON.stringify({binary: resolve(binary), evidence}, null, 2));
} finally {
  await rm(root, {recursive: true, force: true});
}
