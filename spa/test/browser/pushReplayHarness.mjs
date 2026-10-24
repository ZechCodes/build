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

export async function runPushReplay({ revision, intervalMs = 12, stallMs = 250 } = {}) {
  const session = await openLayoutSession({ plugins: [pushReplayPlugin({ revision })] });
  try {
    return await session.withPage(async ({ page, basePath }) => {
      await loadBrowserModules(page, { cache: "src/core/localCache.js", app: "src/appState.js", sync: "src/core/cacheSync.js", renderer: "src/core/thread.js", feed: "src/core/taskFeed.js" }, basePath);
      await page.evaluate(async ({ intervalMs, stallMs }) => {
        const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
        const stats = { gets: 0, transactions: 0, pendingTransactions: 0, peakTransactions: 0,
          pendingAppliers: 0, peakAppliers: 0, cacheLatencies: [], renderLatencies: [], rpcCalls: 0 };
        const fixture = window.__pushReplay = { stats, watchers: [], emitted: new Map(), server: new Map() };
        const { cache, sync, renderer, feed, app: { App } } = window.__layoutModules;
        App.devices = [{ id: "replay-device" }];
        App.route = { name: "project", deviceId: "replay-device", projectId: "p0", agent: "agent-0" };
        const address = (entityId, kind, sub = "") => ({ deviceId: "replay-device", entityId, kind, sub });
        const projects = Array.from({ length: 6 }, (_, index) => ({ project_id: `p${index}`, name: `Project ${index}`, entity_id: `run-${index}` }));
        const agents = Array.from({ length: 82 }, (_, index) => ({ id: `agent-${index}`, conversation_id: `agent-${index}`, thread_id: `thread-${index}`, thread_generation_revision: 1 }));
        const rows = projects.map((project, index) => ({ kind: "branch", project_id: project.project_id, run_id: project.entity_id,
          worktree_id: `wt-${index}`, branch: `build/${index}`, state: "building", agents: agents.filter((_, number) => number % 6 === index),
          last_activity: "2026-10-24T16:00:00Z" }));
        fixture.context = { deviceId: "replay-device", session: {}, active: () => true,
          cacheScope: { deviceId: "replay-device", active: () => true }, rpc: async (method, params = {}) => {
            stats.rpcCalls++;
            await sleep(15);
            const row = rows.find((row) => row.run_id === params.entity_id);
            if (method === "agent.list") return { entity_id: row.run_id, pinned_agent_id: row.agents[0].id, agents: row.agents };
            if (method === "thread.page") return { thread_id: params.thread_id, items: [], has_more: false };
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
        for (let index = 0; index < 727; index++) await cache.writeCached(address(`padding-${index}`, "status"), { head: "seed" });
        document.body.innerHTML = '<output id="open-conversation">0</output><main id="conversation"></main>';
        const openAddress = address("run-0", "thread", "agent-0");
        cache.subscribeCache(openAddress, async () => {
          const held = (await cache.readCached(openAddress))?.value;
          document.querySelector("#conversation").innerHTML = renderer.threadHtml(held);
          for (const item of held?.items || []) {
            const sequence = item.data.sequence;
            if (!fixture.emitted.has(sequence) || fixture.seen?.has(sequence)) continue;
            (fixture.seen ||= new Set()).add(sequence);
            stats.cacheLatencies.push(performance.now() - fixture.emitted.get(sequence));
            document.querySelector("#open-conversation").textContent = String(sequence);
            await new Promise(requestAnimationFrame);
            stats.renderLatencies.push(performance.now() - fixture.emitted.get(sequence));
          }
        });
        stats.seedRecords = (await cache.cachedAddresses({ deviceId: "replay-device" })).length;
        stats.seedFeedBytes = JSON.stringify((await cache.readCached(address("", "feed")))?.value).length;
        stats.seedWorkspaceBytes = JSON.stringify((await cache.readCached(address("", "workspaces")))?.value).length;
        await sync.startPushReplay();
        await feed.startFeed();
        const stopFeedListener = feed.subscribeFeed(() => { stats.feedPaints = (stats.feedPaints || 0) + 1; });
        const originalTransaction = IDBDatabase.prototype.transaction;
        const originalGet = IDBObjectStore.prototype.get;
        IDBObjectStore.prototype.get = function (...args) { stats.gets++; return originalGet.apply(this, args); };
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
          for (const [key, value] of Object.entries(work)) {
            stats[`peakWork_${key}`] = Math.max(stats[`peakWork_${key}`] || 0, value);
          }
        };
        const sampleTimer = setInterval(sampleWork, 1);
        for (let frame = 0; frame < 60; frame++) {
          const row = rows[frame % 6];
          const agent = row.agents[0];
          const sequence = Math.floor(frame / 6) + 1;
          const item = { type: "message", data: { id: `message-${frame}`, sequence, role: "agent", body: `frame ${frame}`, created_at: "2026-10-24T16:00:00Z" } };
          fixture.expectedMessages[frame % 6].push(item);
          const tip = { agent_id: agent.id, conversation_id: agent.id, thread_id: agent.thread_id, thread_generation_revision: 1,
            since_sequence: sequence - 1, last_sequence: sequence, thread_total: sequence, items: [item] };
          if (frame % 6 === 0) fixture.emitted.set(sequence, performance.now());
          // Empty, already-covered tips are duplicated by multiple active
          // subscriptions; include them without inventing cursor holes.
          const duplicate = { ...tip, since_sequence: Math.max(0, sequence - 1), last_sequence: Math.max(0, sequence - 1), items: [] };
          fixture.deliveries.push(Promise.resolve(inbox.onChanges([{ entity_id: row.run_id, state: { ...row, replay_frame: frame }, thread: [tip, duplicate] }])));
          await sleep(intervalMs);
          if (frame < 59 && fixture.seen?.size) stats.renderProgressDuringReplay = true;
        }
        await Promise.all(fixture.deliveries);
        const deadline = performance.now() + 20000;
        while ((stats.pendingAppliers || stats.pendingTransactions || Object.values(fixture.work || {}).some(Boolean) || fixture.seen?.size !== 10) && performance.now() < deadline) await sleep(20);
        await sleep(50);
        clearInterval(sampleTimer);
        sampleWork();
        stats.finalWork = fixture.work || null;
        fixture.elapsedMs = performance.now() - fixture.started;
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
          && thread.deliveredSequence === 10 && JSON.stringify(thread.items) === JSON.stringify(fixture.expectedMessages[index]));
        fixture.messageMismatch = fixture.exactFinalMessages ? null : { actual: fixture.finalThreads[0], expected: fixture.expectedMessages[0] };
        fixture.exactFinalStates = fixture.finalRows.every((row, index) =>
          Object.entries({ ...rows[index], replay_frame: 54 + index }).every(([key, value]) => JSON.stringify(row?.[key]) === JSON.stringify(value)));
        fixture.rendered = document.querySelector("#open-conversation").textContent;
        fixture.renderedText = document.querySelector("#conversation").textContent;
        stopFeedListener();
        feed.stopFeed();
        sync.stopCacheSync();
        db.close();
      }, { basePath, intervalMs, stallMs });
      return page.evaluate(() => {
        const { stats, elapsedMs, finalThreads, finalRows, rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch } = window.__pushReplay;
        const percentile = (values, fraction) => [...values].sort((a, b) => a - b)[Math.floor((values.length - 1) * fraction)] ?? null;
        return { ...stats, elapsedMs, getsPerSecond: stats.gets / (elapsedMs / 1000),
          cacheP95Ms: percentile(stats.cacheLatencies, .95), cacheMaxMs: Math.max(...stats.cacheLatencies),
          renderP95Ms: percentile(stats.renderLatencies, .95), rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch,
          sequences: finalThreads.map((thread) => thread?.items.map((item) => item.data.sequence)),
          finalFrames: finalRows.map((row) => row?.replay_frame) };
      });
    });
  } finally { await session.close(); }
}
