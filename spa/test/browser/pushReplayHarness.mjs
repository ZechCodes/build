import { execFileSync } from "node:child_process";
import { loadBrowserModules, openLayoutSession } from "./layoutHarness.mjs";

// This fixture owns an isolated Chromium context; no authenticated device or
// live bridge is opened. Only connection inputs are replaced. All IndexedDB,
// generation, applier and subscription code comes from the production modules.
export function pushReplayPlugin({ revision } = {}) {
  return {
    name: "push-replay-fixture", enforce: "pre",
    resolveId(source, importer) {
      if (!importer?.endsWith("/src/core/cacheSync.js")) return null;
      if (["./deviceContexts.js", "./changeEvents.js"].includes(source)) return `\0push-replay:${source}`;
      return null;
    },
    load(id) {
      if (id === "\0push-replay:./deviceContexts.js") return `
        export const contextFor=()=>window.__pushReplay.context;
        export const liveContexts=()=>[];
        export const onDeviceStateChanged=()=>()=>{};`;
      if (id === "\0push-replay:./changeEvents.js") return `
        export const bridgeCapabilities=()=>({agents:{projectWorkers:true},changes:{subscriptions:true}});
        export const onSubscriptionHeld=()=>()=>{};
        export const subscriptionsSettledFor=async()=>{};
        export const watchChanges=(registration)=>{window.__pushReplay.watchers.push(registration);return {dispose(){}}};`;
      if (revision && id.includes("/spa/src/") && /\.[cm]?js$/.test(id)) {
        const path = `spa/src/${id.split("/spa/src/")[1]}`;
        return execFileSync("git", ["show", `${revision}:${path}`], { cwd: new URL("../../../", import.meta.url), encoding: "utf8" });
      }
      return null;
    },
    transform(source, id) {
      if (!id.endsWith("/src/core/cacheSync.js")) return null;
      return source.replace("async function applyItem(", "async function replayOriginalApplyItem(") + `
        async function applyItem(...args) {
          const stats=window.__pushReplay.stats;
          stats.peakAppliers=Math.max(stats.peakAppliers,++stats.pendingAppliers);
          stats.applierStarts=(stats.applierStarts||0)+1;
          try { return await replayOriginalApplyItem(...args); } finally { stats.pendingAppliers--; }
        }
        export async function startPushReplay() {
          holdingLock=true;
          await subscribeDevice(syncContext(contextFor('replay-device')));
        }
      `;
    },
  };
}

export async function runPushReplay({ revision, intervalMs = 12, stallMs = 250, frames = 60, mixed = false, drainMs = 20000 } = {}) {
  const session = await openLayoutSession({ plugins: [pushReplayPlugin({ revision })] });
  try {
    return await session.withPage(async ({ page, basePath }) => {
      await loadBrowserModules(page, { cache: "src/core/localCache.js", app: "src/appState.js", sync: "src/core/cacheSync.js", renderer: "src/core/thread.js", feed: "src/core/taskFeed.js", visible: "src/core/syncVisibleScopes.js", watched: "src/core/watchedTaskFollower.js", lineage: "src/core/agentLineage.js" }, basePath, { timeoutMs: 60_000 });
      await page.evaluate(async ({ intervalMs, stallMs, frames, mixed, drainMs }) => {
        const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
        const stats = { gets: 0, transactions: 0, pendingTransactions: 0, peakTransactions: 0,
          pendingAppliers: 0, peakAppliers: 0, cacheLatencies: [], cacheApplyTurns: [], renderLatencies: [], rpcCalls: 0, rowGets: 0, backlogSamples: [] };
        const fixture = window.__pushReplay = { stats, watchers: [], emitted: new Map(), emittedTurns: new Map(), server: new Map() };
        const { cache, sync, renderer, feed, visible, watched, lineage, app: { App } } = window.__layoutModules;
        App.devices = [{ id: "replay-device" }];
        App.route = { name: "project", deviceId: "replay-device", projectId: "p0", agent: "agent-0" };
        const address = (entityId, kind, sub = "") => ({ deviceId: "replay-device", entityId, kind, sub });
        const projects = Array.from({ length: 6 }, (_, index) => ({ project_id: `p${index}`, name: `Project ${index}`, entity_id: `run-${index}`, deviceId: "replay-device" }));
        const agents = Array.from({ length: 82 }, (_, index) => ({ id: `agent-${index}`, conversation_id: `agent-${index}`, thread_id: `thread-${index}`, thread_generation_revision: 1 }));
        const rows = projects.map((project, index) => ({ kind: "branch", deviceId: "replay-device", project_id: project.project_id, run_id: project.entity_id,
          worktree_id: `wt-${index}`, branch: `build/${index}`, state: "building", agents: agents.filter((_, number) => number % 6 === index),
          last_activity: "2026-10-24T16:00:00Z" }));
        fixture.resetRow = { ...rows[5], run_id: "reset-fixture", agents: [{ id: "reset-agent", conversation_id: "reset-agent", thread_id: "reset-old", thread_generation_revision: 1 }] };
        fixture.context = { deviceId: "replay-device", session: {}, active: () => true,
          cacheScope: { deviceId: "replay-device", active: () => true }, rpc: async (method, params = {}) => {
            stats.rpcCalls++;
            await sleep(15);
            const row = rows.find((row) => row.run_id === params.entity_id) || fixture.resetRow;
            if (method === "agent.list") return { entity_id: row.run_id, pinned_agent_id: row.agents[0].id, agents: row.agents };
            if (method === "thread.page") return { thread_id: params.thread_id, thread_generation_revision: 1,
              items: (fixture.server.get(params.thread_id) || []).filter((item) => item.data.sequence > (params.after_sequence || 0)), has_more: false };
            return {};
          } };
        await cache.writeCached(address("", "projects"), projects);
        await cache.writeCached(address("", "feed"), { items: [...rows, ...Array.from({ length: 117 }, (_, index) => ({ kind: "branch", run_id: `padding-${index}`, project_id: "p0", body: "x".repeat(2900) }))], plans: [], runs: [] });
        await cache.writeCached(address("", "workspaces"), agents.map((agent, index) => ({ workspace_id: `ws${index}`, project_id: `p${index % 6}`, metadata: "x".repeat(2550) })));
        for (const row of rows) {
          await cache.writeCached(address(row.run_id, "row"), row);
          for (const agent of row.agents) await cache.writeCached(address(row.run_id, "thread", agent.id), {
            thread_id: agent.thread_id, thread_generation_revision: 1, items: [], deliveredSequence: 0, activityDigests: [], knownTotalItems: 0,
          });
        }
        if (mixed) {
          for (let index = 0; index < 117; index++) await cache.writeCached(address(`padding-${index}`, "row"), {
            kind: "branch", run_id: `padding-${index}`, project_id: "p0", state: "building", agents: [], body: "x".repeat(2900),
          });
          await cache.writeCached(address("retired", "row"), { ...rows[0], run_id: "retired" });
          await cache.writeCached(address("retired", "status"), { head: "must-be-removed" });
          await cache.writeCached(address("reset-fixture", "row"), fixture.resetRow);
          await cache.writeCached(address("reset-fixture", "thread", "reset-agent"), {
            thread_id: "reset-old", thread_generation_revision: 1, items: [{ type: "message", data: { id: "old-reset-message", sequence: 1 } }], deliveredSequence: 1,
          });
        }
        for (let index = 0; index < 727; index++) await cache.writeCached(address(`padding-${index}`, "status"), { head: "seed" });
        document.body.innerHTML = '<output id="open-conversation">0</output><main id="conversation"></main>';
        const openAddress = address("run-0", "thread", "agent-0");
        const stopThread = visible.registerSyncThread(() => ({ deviceId: "replay-device", entityId: "run-0",
          agentId: "agent-0", conversationId: "agent-0", threadId: "thread-0" }));
        cache.subscribeCache(openAddress, async () => {
          const held = (await cache.readCached(openAddress))?.value;
          const appliedAtRead = stats.applierStarts || 0;
          document.querySelector("#conversation").innerHTML = renderer.threadHtml(held);
          for (const item of held?.items || []) {
            const sequence = item.data.sequence;
            if (!fixture.emitted.has(sequence) || fixture.seen?.has(sequence)) continue;
            (fixture.seen ||= new Set()).add(sequence);
            stats.cacheLatencies.push(performance.now() - fixture.emitted.get(sequence));
            stats.cacheApplyTurns.push(appliedAtRead - fixture.emittedTurns.get(sequence));
            document.querySelector("#open-conversation").textContent = String(sequence);
            await new Promise(requestAnimationFrame);
            stats.renderLatencies.push(performance.now() - fixture.emitted.get(sequence));
          }
        });
        stats.seedRecords = (await cache.cachedAddresses({ deviceId: "replay-device" })).length;
        stats.seedFeedBytes = JSON.stringify((await cache.readCached(address("", "feed")))?.value).length;
        stats.seedWorkspaceBytes = JSON.stringify((await cache.readCached(address("", "workspaces")))?.value).length;
        const watchedTasks = mixed ? watched.followWatchedTasks({ onChange: () => { stats.watchedTaskPaints = (stats.watchedTaskPaints || 0) + 1; } }) : null;
        watchedTasks?.follow(projects.map((project) => ({ ...project, id: project.project_id })));
        const mountedLineage = mixed ? lineage.mountAgentLineage({ deviceId: "replay-device", projectId: "p0",
          onChanged: () => { stats.lineagePaints = (stats.lineagePaints || 0) + 1; } }) : null;
        mountedLineage?.start();
        if (mixed) await sleep(50); // Let the initial mounted cache readers warm.
        await sync.startPushReplay();
        await feed.startFeed();
        const stopFeedListener = feed.subscribeFeed(() => { stats.feedPaints = (stats.feedPaints || 0) + 1; });
        const originalTransaction = IDBDatabase.prototype.transaction;
        const originalGet = IDBObjectStore.prototype.get;
        IDBObjectStore.prototype.get = function (...args) {
          stats.gets++;
          if (String(args[0]).split("|")[2] === "row") stats.rowGets++;
          return originalGet.apply(this, args);
        };
        IDBDatabase.prototype.transaction = function (...args) {
          const tx = originalTransaction.apply(this, args);
          if (this.name !== "build-cache") return tx;
          stats.transactions++;
          stats.peakTransactions = Math.max(stats.peakTransactions, ++stats.pendingTransactions);
          const done = () => stats.pendingTransactions--;
          tx.addEventListener("complete", done, { once: true });
          tx.addEventListener("abort", done, { once: true });
          return tx;
        };
        // A real readwrite transaction occupies the records store. Repeated
        // requests keep it alive until the deliberate storage stall expires.
        const db = await new Promise((resolve) => { const request = indexedDB.open("build-cache"); request.onsuccess = () => resolve(request.result); });
        const stall = originalTransaction.call(db, "records", "readwrite");
        const until = performance.now() + stallMs;
        const keepAlive = () => { const request = originalGet.call(stall.objectStore("records"), "stall"); request.onsuccess = () => { if (performance.now() < until) keepAlive(); }; };
        keepAlive();
        fixture.started = performance.now();
        const inbox = fixture.watchers.find((watcher) => watcher.id === "s-inbox");
        fixture.deliveries = [];
        fixture.expectedMessages = Array.from({ length: 6 }, () => []);
        const sampleWork = () => {
          const work = sync.pushWorkInFlight?.("replay-device");
          if (!work) return;
          fixture.work = work;
          const now = performance.now() - fixture.started;
          if (!stats.backlogSamples.length || now - stats.backlogSamples.at(-1).at >= 1000)
            stats.backlogSamples.push({ at: now, pending: work.pending, backgroundPending: work.backgroundPending });
          for (const [key, value] of Object.entries(work)) {
            stats[`peakWork_${key}`] = Math.max(stats[`peakWork_${key}`] || 0, value);
          }
        };
        const sampleTimer = setInterval(sampleWork, 1);
        for (let frame = 0; frame < frames; frame++) {
          const row = rows[frame % 6];
          const agent = row.agents[0];
          const sequence = Math.floor(frame / 6) + 1;
          const item = { type: "message", data: { id: `message-${frame}`, sequence, role: "agent", body: `frame ${frame}`, created_at: "2026-10-24T16:00:00Z" } };
          fixture.expectedMessages[frame % 6].push(item);
          fixture.server.set(agent.thread_id, fixture.expectedMessages[frame % 6]);
          const tip = { agent_id: agent.id, conversation_id: agent.id, thread_id: agent.thread_id, thread_generation_revision: 1,
            since_sequence: sequence - 1, last_sequence: sequence, thread_total: sequence, items: [item] };
          if (frame % 6 === 0) {
            fixture.emitted.set(sequence, performance.now());
            fixture.emittedTurns.set(sequence, stats.applierStarts || 0);
          }
          // Empty, already-covered tips are duplicated by multiple active
          // subscriptions; include them without inventing cursor holes.
          const duplicate = { ...tip, since_sequence: Math.max(0, sequence - 1), last_sequence: Math.max(0, sequence - 1), items: [] };
          if (mixed && frame % 36 === 12) tip.items = []; // A genuine missing inline delta, repaired from thread.page.
          if (mixed) row.agents[0].topic = `topic-${frame}`;
          const changes = [{ entity_id: row.run_id, state: structuredClone({ ...row, replay_frame: frame }), thread: [tip, duplicate] }];
          if (mixed) {
            changes.unshift({ entity_id: "board", state: { revision: frame } });
            changes.push({ entity_id: `notification-${frame % 8}`, terminals: { tabs: [{ term_id: `tab-${frame}` }] } });
            if (frame % 240 === 120) changes.push({ entity_id: "board", state: { revision: frame, removed: ["retired"] } });
            // Exercise a real reset barrier on an independent conversation.
            if (frame % 240 === 180) {
              fixture.resetRow.agents[0] = { id: "reset-agent", conversation_id: "reset-agent", thread_id: `reset-${frame}`, thread_generation_revision: frame };
              changes.push({ entity_id: "reset-fixture", thread: [{ agent_id: "reset-agent",
              conversation_id: "reset-agent", thread_id: `reset-${frame}`, thread_generation_revision: frame,
              since_sequence: 0, last_sequence: 0, reset: true, items: [] }] });
            }
          }
          fixture.deliveries.push(Promise.resolve(inbox.onChanges(changes)));
          await sleep(intervalMs);
          if (frame < frames - 1 && fixture.seen?.size) stats.renderProgressDuringReplay = true;
        }
        fixture.inputEnded = performance.now();
        await Promise.all(fixture.deliveries);
        const deadline = performance.now() + drainMs;
        while ((stats.pendingAppliers || stats.pendingTransactions || Object.values(fixture.work || {}).some(Boolean) || fixture.seen?.size !== Math.ceil(frames / 6)) && performance.now() < deadline) await sleep(20);
        await sleep(50);
        clearInterval(sampleTimer);
        sampleWork();
        stats.finalWork = fixture.work || null;
        fixture.elapsedMs = performance.now() - fixture.started;
        stats.drainMs = performance.now() - fixture.inputEnded;
        IDBObjectStore.prototype.get = originalGet;
        IDBDatabase.prototype.transaction = originalTransaction;
        fixture.finalThreads = [];
        fixture.finalRows = [];
        for (const row of rows) {
          fixture.finalThreads.push((await cache.readCached(address(row.run_id, "thread", row.agents[0].id)))?.value);
          fixture.finalRows.push((await cache.readCached(address(row.run_id, "row")))?.value);
        }
        fixture.exactFinalMessages = fixture.finalThreads.every((thread, index) =>
          thread.thread_id === rows[index].agents[0].thread_id
          && thread.deliveredSequence === fixture.expectedMessages[index].length && JSON.stringify(thread.items) === JSON.stringify(fixture.expectedMessages[index]));
        fixture.messageMismatch = fixture.exactFinalMessages ? null : { actual: fixture.finalThreads[0], expected: fixture.expectedMessages[0] };
        fixture.exactFinalStates = fixture.finalRows.every((row, index) =>
          Object.entries({ ...rows[index], replay_frame: Math.floor((frames - 1 - index) / 6) * 6 + index }).every(([key, value]) => JSON.stringify(row?.[key]) === JSON.stringify(value)));
        if (mixed) {
          stats.retiredAddresses = (await cache.cachedAddresses({ deviceId: "replay-device", entityId: "retired" })).length;
          stats.finalResetThread = (await cache.readCached(address("reset-fixture", "thread", "reset-agent")))?.value;
        }
        fixture.rendered = document.querySelector("#open-conversation").textContent;
        fixture.renderedText = document.querySelector("#conversation").textContent;
        watchedTasks?.dispose();
        mountedLineage?.dispose();
        stopFeedListener();
        feed.stopFeed();
        stopThread();
        sync.stopCacheSync();
        db.close();
      }, { basePath, intervalMs, stallMs, frames, mixed, drainMs });
      return page.evaluate(() => {
        const { stats, elapsedMs, finalThreads, finalRows, rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch } = window.__pushReplay;
        const percentile = (values, fraction) => [...values].sort((a, b) => a - b)[Math.floor((values.length - 1) * fraction)] ?? null;
        return { ...stats, elapsedMs, getsPerSecond: stats.gets / (elapsedMs / 1000),
          cacheP95Ms: percentile(stats.cacheLatencies, .95), cacheMaxMs: Math.max(...stats.cacheLatencies),
          renderP95Ms: percentile(stats.renderLatencies, .95), renderMaxMs: Math.max(...stats.renderLatencies), rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch,
          sequences: finalThreads.map((thread) => thread?.items.map((item) => item.data.sequence)),
          finalFrames: finalRows.map((row) => row?.replay_frame) };
      });
    });
  } finally { await session.close(); }
}
