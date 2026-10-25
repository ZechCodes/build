import { writeFile } from "node:fs/promises";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
import { LIVE_SYNC_SHAPE } from "./liveSyncShape.mjs";
import { pushReplayPlugin } from "./pushReplayHarness.mjs";

export function liveSyncPlugin(options) {
  const plugin = pushReplayPlugin(options);
  const originalTransform = plugin.transform;
  plugin.transform = (source, id) => {
    let code = originalTransform(source, id) || source;
    const wrap = (name, label) => code.replace(`function ${name}() {`, `function ${name}() { return window.__liveSyncMetrics ? window.__liveSyncMetrics.time("${label}", () => profileOriginal${name}()) : profileOriginal${name}(); } function profileOriginal${name}() {`);
    if (id.endsWith("/src/core/inboxView.js")) {
      code = wrap("draw", "inboxDraw");
      code = code.replace("function profileOriginaldraw() {", 'function profileOriginaldraw() { if (window.__liveDisableInbox) { window.__liveSyncMetrics?.count("inboxDrawAblated"); return; }');
    }
    if (id.endsWith("/src/core/agentCanvas.js")) {
      code = wrap("paint", "canvasPaint");
      code = code.replace("function profileOriginalpaint() {", 'function profileOriginalpaint() { if (window.__liveDisableCanvas) { window.__liveSyncMetrics?.count("canvasPaintAblated"); return; }');
    }
    if (id.endsWith("/src/core/localCache.js")) {
      code = code.replace("export function readCached(address) {", "export function readCached(address) { window.__liveSyncMetrics?.read(address);");
      code = code.replace("export const subscribeCache = subscribe;", 'export const subscribeCache = (prefix, listener) => subscribe(prefix, (address, ...args) => { window.__liveSyncMetrics?.count("invalidation:" + (address?.kind || "prefix")); return listener(address, ...args); });');
    }
    if (id.endsWith("/src/core/watchedTaskFollower.js")) {
      code = code.replace("async function reread() {", 'async function reread() { window.__liveSyncMetrics?.count("watchedReread:" + projectId);');
      code = code.replace("onChange();", 'window.__liveSyncMetrics?.count("watchedPublishes"); onChange();');
      code = code.replace("tasks = record?.tasks || [];", 'window.__liveSyncMetrics?.count("watchedAccepted:" + projectId); tasks = record?.tasks || [];');
    }
    if (id.endsWith("/src/core/pushQueue.js")) {
      code = code.replace("function append(context, item) {", 'function append(context, item) { window.__liveSyncMetrics?.stage("admission", item);');
      code = code.replace("async function run(job) {", 'async function run(job) { window.__liveSyncMetrics?.stage("workerStart", job.item);');
    }
    return code;
  };
  return plugin;
}

export async function runLiveSyncReplay({ revision, intervalMs = 375, stallMs = 0, frames = 120, mixed = true, drainMs = 20000, cpuProfilePath, ablateCanvas = false, ablateInbox = false, cacheCanvasRect = false, boardMembership = 6, staticDomTarget = 1284, tasksPerProject = 143, retainedMessages = 330, resetFrame = 180, removalFrame = 120 } = {}) {
  const session = await openLayoutSession({ plugins: [liveSyncPlugin({ revision })] });
  try {
    return await session.withPage(async ({ page, basePath }) => {
      const pageErrors = [];
      page.on("pageerror", (error) => pageErrors.push(String(error)));
      const cdp = await page.context().newCDPSession(page);
      let cpuProfile;
      await cdp.send("Performance.enable");
      await page.exposeFunction("__startLiveSyncMeasurement", async () => {
        if (cpuProfilePath) { await cdp.send("Profiler.enable"); await cdp.send("Profiler.start"); }
      });
      await page.exposeFunction("__stopLiveSyncMeasurement", async () => {
        if (cpuProfilePath) cpuProfile = (await cdp.send("Profiler.stop")).profile;
      });
      await mountLayout(page, `<div id="shell"><aside id="inbox-rail"><div id="inbox-list"></div></aside>
        <div id="view"><header id="toolbar">Replay</header><div id="view-body"><main id="root">
        <output id="open-conversation">0</output><section id="conversation"></section></main>
        <aside id="agent-rail"></aside></div><div id="console-region"></div></div></div>`, { basePath });
      await loadBrowserModules(page, { cache: "src/core/localCache.js", app: "src/appState.js", sync: "src/core/cacheSync.js", renderer: "src/core/thread.js", feed: "src/core/taskFeed.js", visible: "src/core/syncVisibleScopes.js", watched: "src/core/watchedTaskFollower.js", lineage: "src/core/agentLineage.js", inbox: "src/core/inboxView.js", agentRail: "src/core/agentRail.js", tracker: "src/core/trackerCache.js", metrics: "test/browser/liveSyncMetrics.mjs", contexts: "src/core/deviceContexts.js", canvas: "src/core/agentCanvas.js" }, basePath, { timeoutMs: 60_000 });
      await page.evaluate(async ({ intervalMs, stallMs, frames, mixed, drainMs, tasksPerProject, retainedMessages, shape, ablateCanvas, cacheCanvasRect, boardMembership, staticDomTarget, ablateInbox, resetFrame, removalFrame }) => {
        const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
        const stats = { gets: 0, transactions: 0, pendingTransactions: 0, peakTransactions: 0,
          pendingAppliers: 0, peakAppliers: 0, cacheLatencies: [], cacheApplyTurns: [], renderLatencies: [], renderSchedulerTurns: [], rpcCalls: 0, rowGets: 0, backlogSamples: [] };
        const fixture = window.__pushReplay = { stats, watchers: [], emitted: new Map(), emittedTurns: new Map(), emittedSchedulerTurns: new Map(), server: new Map() };
        const { cache, sync, renderer, feed, visible, watched, lineage, inbox, agentRail, tracker, metrics, contexts, canvas, app: { App } } = window.__layoutModules;
        metrics.installLiveSyncMetrics();
        App.deviceFilter = null;
        App.devices = [{ id: "replay-device" }];
        App.route = { name: "branch", branch: "build/0", deviceId: "replay-device", projectId: "p0", agent: "agent-0" };
        const address = (entityId, kind, sub = "") => ({ deviceId: "replay-device", entityId, kind, sub });
        const projects = Array.from({ length: 6 }, (_, index) => ({ id: `p${index}`, projectKey: `replay-device/p${index}`, project_id: `p${index}`, name: `Project ${index}`, metadata: "p".repeat(1750), entity_id: `run-${index}`, deviceId: "replay-device" }));
        const agents = Array.from({ length: 109 }, (_, index) => ({ id: `agent-${index}`, conversation_id: `agent-${index}`, thread_id: `thread-${index}`, thread_generation_revision: 1, ordinal: index + 1, working: index === 0, state: "live", watched: index === 0, unread_count: index === 0 ? 1 : 0, read_through_sequence: 1100, name: `Agent ${index}` }));
        const rows = projects.map((project, index) => ({ kind: "branch", deviceId: "replay-device", project_id: project.project_id, projectKey: project.projectKey, run_id: project.entity_id,
          worktree_id: `wt-${index}`, branch: `build/${index}`, state: "building", agents: agents.filter((_, number) => number % 6 === index),
          last_activity_ms: index === 0 ? Date.now() : Date.now() - 30 * 86400000, session_started_ms: (index === 0 ? Date.now() : Date.now() - 30 * 86400000) - 60000 }));
        fixture.projectRosters = rows.map((row, index) => ({ entity_id: row.run_id, pinned_agent_id: row.agents[0].id,
          agents: row.agents.slice(0, [1, 2, 2, 2, 1, 1][index]) }));
        rows.forEach((row, index) => { row.agents = fixture.projectRosters[index].agents; });
        fixture.resetRow = { ...rows[5], run_id: "reset-fixture", agents: [{ id: "reset-agent", conversation_id: "reset-agent", thread_id: "reset-old", thread_generation_revision: 1 }] };
        fixture.context = { deviceId: "replay-device", session: {}, active: () => true,
          cacheScope: { deviceId: "replay-device", active: () => true }, rpc: async (method, params = {}) => {
            stats.rpcCalls++;
            stats.rpcMethods ||= {}; stats.rpcMethods[method] = (stats.rpcMethods[method] || 0) + 1;
            if (method === "board.list") return { items: [...fixture.serverRows.values()].filter((row) => boardMembership === 204 || rows.some((active) => active.run_id === row.run_id)), runs: [], plans: [] };
            if (method === "project.list") return { projects };
            if (method === "workspace.list") return { workspaces: fixture.serverWorkspaces || [] };
            if (method === "tasks.list") return { tasks: fixture.serverTasks?.get(params.project_id) || [], has_more: false };
            if (method === "tasks.columns") return { columns: [] };
            if (method === "tasks.get") {
              const task = [...(fixture.serverTasks?.values() || [])].flat().find((task) => task.id === params.task_id);
              return task ? { task, timeline: [] } : null;
            }
            if (method === "entity.seen") {
              const agent = agents.find((agent) => agent.id === params.agent_id);
              if (agent) { agent.read_through_sequence = Math.max(agent.read_through_sequence, params.read_through_sequence || 0); agent.unread_count = 0;
                for (const row of fixture.serverRows?.values() || []) {
                  const owned = row.agents?.find((held) => held.id === agent.id);
                  if (owned) { owned.read_through_sequence = agent.read_through_sequence; owned.unread_count = 0; }
                }
              }
              return { ok: true };
            }
            await sleep(15);
            const row = fixture.serverRows?.get(params.entity_id) || rows.find((row) => row.run_id === params.entity_id) || fixture.resetRow;
            if (method === "models.list") return { default_provider: "codex", providers: [{ id: "codex", label: "Codex", models: [], efforts: [] }] };
            if (method === "agent.list") return { entity_id: row.run_id, pinned_agent_id: row.agents[0]?.id || null, agents: (fixture.projectRosters.find((roster) => roster.entity_id === row.run_id) || row).agents };
            if (method === "thread.page") return { thread_id: params.thread_id, thread_generation_revision: 1,
              items: (fixture.server.get(params.thread_id) || []).filter((item) => item.data.sequence > (params.after_sequence || 0)), has_more: false };
            return {};
          } };
        await cache.writeCached(address("", "projects"), projects);
        await cache.writeCached(address("", "feed"), { items: [...rows, ...Array.from({ length: 196 }, (_, index) => ({ kind: "branch", run_id: `padding-${index}`, project_id: `p${index % 6}`, branch: `retained/${index}`, state: "building", last_activity: new Date(index === 0 ? Date.now() : Date.now() - 30 * 86400000).toISOString(), last_activity_ms: index === 0 ? Date.now() : Date.now() - 30 * 86400000, body: "x".repeat(2900) }))], plans: [], runs: [] });
        await cache.writeCached(address("", "workspaces"), Array.from({ length: 104 }, (_, index) => ({ id: `ws${index}`, workspace_id: `ws${index}`, workspaceKey: `replay-device/ws${index}`, projectKey: `replay-device/p${index % 6}`, deviceId: "replay-device", name: `Retained workspace ${index}`, entity_id: `padding-${index}`, status: "ready", managed: true, session_started_ms: (index === 0 ? Date.now() : Date.now() - 30 * 86400000) - 60000, last_activity_ms: index === 0 ? Date.now() : Date.now() - 30 * 86400000, project_id: `p${index % 6}`, metadata: "x".repeat(2139) })));
        for (const row of rows) {
          await cache.writeCached(address(row.run_id, "row"), row);
          for (const agent of agents.filter((_, index) => index % 6 === Number(row.run_id.slice(4)))) await cache.writeCached(address(row.run_id, "thread", agent.id), {
            thread_id: agent.thread_id, thread_generation_revision: 1, items: Array.from({ length: agent.id === "agent-0" ? retainedMessages : 0 }, (_, n) => ({ type: "message", data: { id: `retained-${agent.id}-${n}`, sequence: 1100 - retainedMessages + n + 1, role: "agent", body: `Retained fixture message ${n} ${"r".repeat(n === 0 ? 1200 : 1000)}` } })), deliveredSequence: 1100, activityDigests: [], knownTotalItems: 1100,
          });
        }
        if (mixed) {
          for (let index = 0; index < 196; index++) await cache.writeCached(address(`padding-${index}`, "row"), {
            kind: "branch", run_id: `padding-${index}`, project_id: `p${index % 6}`, branch: `retained/${index}`, last_activity: new Date(index === 0 ? Date.now() : Date.now() - 30 * 86400000).toISOString(), last_activity_ms: index === 0 ? Date.now() : Date.now() - 30 * 86400000, state: "building", agents: [], body: "x".repeat(2900),
          });
          await cache.writeCached(address("retired", "row"), { ...rows[0], run_id: "retired", branch: "retired/barrier", agents: [] });
          await cache.writeCached(address("retired", "status"), { head: "must-be-removed" });
          await cache.writeCached(address("reset-fixture", "row"), fixture.resetRow);
          await cache.writeCached(address("reset-fixture", "thread", "reset-agent"), {
            thread_id: "reset-old", thread_generation_revision: 1, items: [{ type: "message", data: { id: "old-reset-message", sequence: 1 } }], deliveredSequence: 1,
          });
        }
        await cache.writeCached(address("", "agent-lineage-support"), { namesMakers: true });
        await cache.writeCached(address("", "project-agents-support"), { projectWorkers: true });
        await cache.writeCached(address("", "needs-you-rule"), { askedOnly: false });
        for (const roster of fixture.projectRosters) await cache.writeCached(address(roster.entity_id, "project-agents"), roster);
        fixture.serverTasks = new Map();
        for (const project of projects) {
          const tasks = Array.from({ length: tasksPerProject }, (_, index) => ({ id: `task-${project.id}-${index}`, number: index + 1,
            project_id: project.id, title: `Fixture task ${index}`, state: "open", status: "in_progress", watched: project.id === "p0" && index < 2,
            assignee: { kind: "user" }, updated_at: new Date().toISOString(), created_at: new Date().toISOString() }));
          fixture.serverTasks.set(project.id, tasks);
          await tracker.writeTasksRecord("replay-device", project.id, tracker.tasksRecord(tasks, []));
          for (const task of tasks) await tracker.writeTaskRecord("replay-device", project.id, task.id, { task, timeline: [] });
        }
        const seededCounts = {};
        for (const held of await cache.cachedAddresses({ deviceId: "replay-device" })) seededCounts[held.kind] = (seededCounts[held.kind] || 0) + 1;
        for (const [kind, total] of Object.entries(shape.counts)) {
          for (let index = seededCounts[kind] || 0; index < total; index++) await cache.writeCached(address(total === 1 ? "" : kind.startsWith("tracker-") ? `p${index % 6}` : `padding-${index % 196}`, kind, total === 1 ? "" : `retained-${index}`), { fixture: true });
        }
        fixture.serverRows = new Map();
        for (const held of await cache.cachedAddresses({ deviceId: "replay-device" })) if (held.kind === "row") {
          const value = (await cache.readCached(held))?.value;
          fixture.serverRows.set(held.entityId, value);
        }
        fixture.serverWorkspaces = (await cache.readCached(address("", "workspaces")))?.value;
        contexts.adoptDeviceSession({ deviceId: "replay-device", call: fixture.context.rpc, close() {}, peer() {}, onCarrier() {}, onPush() {} });
        const openAddress = address("run-0", "thread", "agent-0");
        const stopThread = visible.registerSyncThread(() => ({ deviceId: "replay-device", entityId: "run-0",
          agentId: "agent-0", conversationId: "agent-0", threadId: "thread-0" }));
        cache.subscribeCache(openAddress, async () => {
          const held = (await cache.readCached(openAddress))?.value;
          const appliedAtRead = stats.applierStarts || 0;
          // Actual agentRail is the only conversation DOM renderer in this fixture.
          for (const item of held?.items || []) {
            const sequence = item.data.sequence;
            if (!fixture.emitted.has(sequence) || fixture.seen?.has(sequence)) continue;
            (fixture.seen ||= new Set()).add(sequence);
            stats.cacheLatencies.push(performance.now() - fixture.emitted.get(sequence));
            stats.cacheApplyTurns.push(appliedAtRead - fixture.emittedTurns.get(sequence));
            document.querySelector("#open-conversation").textContent = String(sequence);
            await new Promise(requestAnimationFrame);
            stats.renderLatencies.push(performance.now() - fixture.emitted.get(sequence));
            stats.renderSchedulerTurns.push((stats.schedulerStarts || 0) - fixture.emittedSchedulerTurns.get(sequence));
          }
        });
        const originalTransaction = IDBDatabase.prototype.transaction;
        const originalGet = IDBObjectStore.prototype.get;
        const originalPut = IDBObjectStore.prototype.put;
        IDBObjectStore.prototype.put = function (record, key, ...args) {
          if (key === "replay-device|run-0|thread|agent-0") this.transaction.addEventListener("complete", () => {
            window.__liveSyncMetrics.stage("cacheCommit", { entity_id: "run-0", thread: [{ last_sequence: record.value?.deliveredSequence }] });
          }, { once: true });
          return originalPut.call(this, record, key, ...args);
        };
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
        async function waitForQuiet() {
          const watchdog = performance.now() + 60000;
          let quietSince = performance.now(), previous = "";
          while (performance.now() < watchdog) {
            const work = sync.pushWorkInFlight("replay-device");
            const pending = stats.pendingTransactions || stats.pendingAppliers || sync.passInFlight("replay-device")
              || Object.values(work || {}).some(Boolean);
            const fingerprint = JSON.stringify([stats.transactions, stats.rpcCalls, window.__liveSyncMetrics.timings.inboxDraw?.count || 0]);
            if (pending || fingerprint !== previous) { quietSince = performance.now(); previous = fingerprint; }
            if (performance.now() - quietSince >= 150) return { pendingTransactions: stats.pendingTransactions,
              pendingAppliers: stats.pendingAppliers, passRunning: Boolean(sync.passInFlight("replay-device")), work };
            await sleep(25);
          }
          throw new Error("Replay fixture did not quiesce before the measurement boundary");
        }
        stats.seedRecords = (await cache.cachedAddresses({ deviceId: "replay-device" })).length;
        stats.seedRecordsBeforeWarm = stats.seedRecords;
        stats.seedOwnThreadChars = JSON.stringify((await cache.readCached(openAddress))?.value).length;
        stats.seedFeedBytes = JSON.stringify((await cache.readCached(address("", "feed")))?.value).length;
        stats.seedWorkspaceBytes = JSON.stringify((await cache.readCached(address("", "workspaces")))?.value).length;
        inbox.mountInboxList();
        const rail = agentRail.mountAgentRail(document.querySelector("#agent-rail"), {
          kind: "branch", branch: "build/0", deviceId: "replay-device", projectId: "p0", entityId: "run-0", openAgentId: "agent-0", panelOpen: true,
          call: fixture.context.rpc,
        });
        await sync.startPushReplay();
        await feed.startFeed();
        await sleep(1000);
        const warmStarted = performance.now();
        for (let pass = sync.passInFlight("replay-device"); pass; pass = sync.passInFlight("replay-device")) await pass;
        stats.warmBoundary = await waitForQuiet();
        stats.warmPassStillRunning = Boolean(sync.passInFlight("replay-device"));
        stats.warmWaitMs = performance.now() - warmStarted;
        fixture.expectedStates = new Map();
        const staticDetail = document.createElement("section");
        staticDetail.id = "fixture-retained-detail";
        staticDetail.setAttribute("aria-label", "Retained static file detail");
        document.querySelector("#root").append(staticDetail);
        const missingNodes = Math.max(0, staticDomTarget - document.querySelectorAll("*").length);
        for (let index = 0; index < missingNodes; index++) {
          const line = document.createElement("span");
          line.textContent = index % 20 === 0 ? "Retained file detail " : "·";
          staticDetail.append(line);
        }
        stats.seedRecords = (await cache.cachedAddresses({ deviceId: "replay-device" })).length;
        stats.seedRailMarkup = document.querySelector("#agent-rail").innerHTML;
        stats.seedDomElements = document.querySelectorAll("*").length;
        stats.seedVisibleInboxRows = [...document.querySelectorAll("#inbox-list [data-key]")].filter((node) => node.getBoundingClientRect().height > 0).length;
        stats.seedCanvasCount = document.querySelectorAll("canvas").length;
        stats.animatingCanvasCount = canvas.animatingRendererCount();
        stats.seedInboxRows = document.querySelectorAll("#inbox-list .inbox-entry").length;
        const realSeen = new Set();
        stats.realPaintLatencies = [];
        stats.realPaintSchedulerTurns = [];
        const observeRealPaint = new MutationObserver(() => {
          for (const [sequence, at] of fixture.emitted) {
            if (realSeen.has(sequence)) continue;
            const row = document.querySelector(`#agent-rail [data-sequence="${sequence}"]`);
            if (!row || !row.textContent.includes(`frame ${(sequence - 1101) * 6}`)) continue;
            realSeen.add(sequence);
            requestAnimationFrame(() => {
              stats.realPaintLatencies.push(performance.now() - at);
              stats.realPaintSchedulerTurns.push((stats.schedulerStarts || 0) - fixture.emittedSchedulerTurns.get(sequence));
              window.__liveSyncMetrics.stage("mountedPaint", { entity_id: "run-0", thread: [{ last_sequence: sequence }] });
            });
          }
        });
        observeRealPaint.observe(document.querySelector("#agent-rail"), { subtree: true, childList: true, characterData: true });
        const stopFeedListener = feed.subscribeFeed(() => { stats.feedPaints = (stats.feedPaints || 0) + 1; });
        // A real readwrite transaction occupies the records store. Repeated
        // requests keep it alive until the deliberate storage stall expires.
        const db = await new Promise((resolve) => { const request = indexedDB.open("build-cache"); request.onsuccess = () => resolve(request.result); });
        const stall = originalTransaction.call(db, "records", "readwrite");
        const until = performance.now() + stallMs;
        const keepAlive = () => { const request = originalGet.call(stall.objectStore("records"), "stall"); request.onsuccess = () => { if (performance.now() < until) keepAlive(); }; };
        keepAlive();
        window.__liveSyncMetrics.active = true;
        const beforeProbeGets = stats.gets;
        await Promise.all(Array.from({ length: 32 }, () => cache.writeCached(address("run-1", "row"), rows[1])));
        stats.measurementBoundary = await waitForQuiet();
        stats.noopProbe = { directGets: stats.gets - beforeProbeGets,
          counters: structuredClone(window.__liveSyncMetrics.counters), timings: structuredClone(window.__liveSyncMetrics.timings),
          reads: structuredClone(window.__liveSyncMetrics.reads) };
        Object.assign(window.__liveSyncMetrics, { counters: {}, timings: {}, reads: {}, readStacks: {}, stages: [], drawCallsites: {} });
        stats.gets = 0; stats.rowGets = 0; stats.transactions = 0; stats.peakTransactions = stats.pendingTransactions;
        window.__liveDisableInbox = ablateInbox;
        window.__liveDisableCanvas = ablateCanvas;
        window.__liveSyncCacheCanvasRect = cacheCanvasRect;
        await window.__startLiveSyncMeasurement();
        window.__liveSyncMetrics.active = true;
        fixture.started = performance.now();
        const inboxSubscription = fixture.watchers.find((watcher) => watcher.id === "s-inbox");
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
          const sequence = 1100 + Math.floor(frame / 6) + 1;
          const item = { type: "message", data: { id: `message-${frame}`, sequence, role: "agent", body: `frame ${frame}`, created_at: "2026-10-24T16:00:00Z" } };
          fixture.expectedMessages[frame % 6].push(item);
          fixture.server.set(agent.thread_id, fixture.expectedMessages[frame % 6]);
          const tip = { agent_id: agent.id, conversation_id: agent.id, thread_id: agent.thread_id, thread_generation_revision: 1,
            since_sequence: sequence - 1, last_sequence: sequence, thread_total: sequence, items: [item] };
          if (frame % 6 === 0) {
            fixture.emitted.set(sequence, performance.now());
            fixture.emittedTurns.set(sequence, stats.applierStarts || 0);
            fixture.emittedSchedulerTurns.set(sequence, stats.schedulerStarts || 0);
          }
          // Empty, already-covered tips are duplicated by multiple active
          // subscriptions; include them without inventing cursor holes.
          const duplicate = { ...tip, since_sequence: Math.max(0, sequence - 1), last_sequence: Math.max(0, sequence - 1), items: [] };
          if (mixed && frame % 36 === 12) tip.items = []; // A genuine missing inline delta, repaired from thread.page.
          if (mixed) row.agents[0].topic = `topic-${frame}`;
          const pushedState = structuredClone({ ...row, replay_frame: frame });
          fixture.expectedStates.set(row.run_id, pushedState);
          fixture.serverRows.set(row.run_id, structuredClone(pushedState));
          const changes = [{ entity_id: row.run_id, state: pushedState, thread: [tip, duplicate] }];
          if (mixed) {
            changes.unshift({ entity_id: "board", state: { revision: frame } });
            changes.push({ entity_id: `notification-${frame % 8}`, terminals: { tabs: [{ term_id: `tab-${frame}` }] } });
            if (frame === removalFrame) changes.push({ entity_id: "board", state: { revision: frame, removed: ["retired"] } });
            // Exercise a real reset barrier on an independent conversation.
            if (frame === resetFrame) {
              fixture.resetRow.agents[0] = { id: "reset-agent", conversation_id: "reset-agent", thread_id: `reset-${frame}`, thread_generation_revision: frame };
              changes.push({ entity_id: "reset-fixture", thread: [{ agent_id: "reset-agent",
              conversation_id: "reset-agent", thread_id: `reset-${frame}`, thread_generation_revision: frame,
              since_sequence: 0, last_sequence: 0, reset: true, items: [] }] });
            }
          }
          window.__liveSyncMetrics.stage("receipt", changes[1]);
          fixture.deliveries.push(Promise.resolve(inboxSubscription.onChanges(changes)));
          await sleep(intervalMs);
          if (frame < frames - 1 && fixture.seen?.size) stats.renderProgressDuringReplay = true;
        }
        fixture.inputEnded = performance.now();
        stats.inputDurationMs = fixture.inputEnded - fixture.started;
        stats.framesReceived = frames;
        stats.framesPerSecond = frames / (stats.inputDurationMs / 1000);
        const inputEndedSchedulerTurns = stats.schedulerStarts || 0;
        const deadline = fixture.inputEnded + drainMs;
        let deliveryTimer;
        await Promise.race([Promise.all(fixture.deliveries), new Promise((resolve) => {
          deliveryTimer = setTimeout(() => { stats.deliveryDeadlineReached = true; resolve(); }, drainMs);
        })]);
        clearTimeout(deliveryTimer);
        while ((stats.pendingAppliers || stats.pendingTransactions || Object.values(fixture.work || {}).some(Boolean) || fixture.seen?.size !== Math.ceil(frames / 6) || stats.renderLatencies.length !== Math.ceil(frames / 6) || stats.realPaintLatencies.length !== Math.ceil(frames / 6)) && performance.now() < deadline) await sleep(20);
        await sleep(50);
        clearInterval(sampleTimer);
        sampleWork();
        stats.finalWork = fixture.work || null;
        window.__liveSyncMetrics.active = false;
        await window.__stopLiveSyncMeasurement();
        stats.ui = { counters: window.__liveSyncMetrics.counters, timings: window.__liveSyncMetrics.timings,
          drawCallsites: window.__liveSyncMetrics.drawCallsites, reads: window.__liveSyncMetrics.reads, readStacks: window.__liveSyncMetrics.readStacks, stages: window.__liveSyncMetrics.stages };
        observeRealPaint.disconnect();
        fixture.elapsedMs = performance.now() - fixture.started;
        stats.drainMs = performance.now() - fixture.inputEnded;
        stats.drainSchedulerTurns = (stats.schedulerStarts || 0) - inputEndedSchedulerTurns;
        IDBObjectStore.prototype.get = originalGet;
        IDBObjectStore.prototype.put = originalPut;
        IDBDatabase.prototype.transaction = originalTransaction;
        fixture.finalThreads = [];
        fixture.finalRows = [];
        for (const row of rows) {
          fixture.finalThreads.push((await cache.readCached(address(row.run_id, "thread", row.agents[0].id)))?.value);
          fixture.finalRows.push((await cache.readCached(address(row.run_id, "row")))?.value);
        }
        fixture.exactFinalMessages = fixture.finalThreads.every((thread, index) =>
          thread.thread_id === rows[index].agents[0].thread_id
          && thread.deliveredSequence === 1100 + fixture.expectedMessages[index].length && JSON.stringify(thread.items.filter((item) => item.data.sequence > 1100)) === JSON.stringify(fixture.expectedMessages[index]));
        fixture.messageMismatch = fixture.exactFinalMessages ? null : { actual: fixture.finalThreads[0], expected: fixture.expectedMessages[0] };
        const readFields = new Set(["read_through_sequence", "read_from_sequence", "unread_count"]);
        const withoutReadFields = (agent) => Object.fromEntries(Object.entries(agent).filter(([key]) => !readFields.has(key)));
        fixture.exactFinalStates = fixture.finalRows.every((held, index) => {
          const expected = fixture.expectedStates.get(rows[index].run_id);
          if (!expected) return false;
          const agentsMatch = JSON.stringify(held?.agents?.map(withoutReadFields)) === JSON.stringify(expected.agents.map(withoutReadFields));
          return agentsMatch && Object.entries(expected).filter(([key]) => key !== "agents")
            .every(([key, value]) => JSON.stringify(held?.[key]) === JSON.stringify(value));
        });
        const expectedRead = fixture.expectedStates.get("run-0").agents[0].read_through_sequence;
        const actualRead = fixture.finalRows[0].agents[0].read_through_sequence;
        const receivedThrough = 1100 + Math.ceil(frames / 6);
        stats.readMarkerConstraints = actualRead >= expectedRead && actualRead <= receivedThrough
          && fixture.finalRows[0].agents[0].thread_id === "thread-0";
        stats.readMarkerLag = { pushed: expectedRead, canonical: actualRead, acknowledged: rows[0].agents[0].read_through_sequence, receivedThrough };
        stats.actualAgents = fixture.finalRows[0]?.agents;
        stats.expectedAgents = rows[0].agents;
        stats.stateMismatch = fixture.finalRows.map((held, index) => Object.keys(rows[index]).filter((key) => JSON.stringify(held?.[key]) !== JSON.stringify(rows[index][key])));
        if (mixed) {
          stats.finalNotificationTabs = [];
          for (let index = 0; index < 8; index++) stats.finalNotificationTabs.push(
            (await cache.readCached(address(`notification-${index}`, "terminals")))?.value?.tabs);
          stats.retiredAddresses = (await cache.cachedAddresses({ deviceId: "replay-device", entityId: "retired" })).length;
          stats.finalResetThread = (await cache.readCached(address("reset-fixture", "thread", "reset-agent")))?.value;
        }
        fixture.rendered = document.querySelector("#open-conversation").textContent;
        fixture.renderedText = document.querySelector("#conversation").textContent;
        inbox.unmountInboxList();
        rail.dispose();
        stopFeedListener();
        feed.stopFeed();
        stopThread();
        sync.stopCacheSync();
        db.close();
      }, { basePath, intervalMs, stallMs, frames, mixed, drainMs, tasksPerProject, retainedMessages, shape: LIVE_SYNC_SHAPE, ablateCanvas, cacheCanvasRect, boardMembership, staticDomTarget, ablateInbox, resetFrame, removalFrame });
      if (cpuProfilePath) await writeFile(cpuProfilePath, JSON.stringify(cpuProfile));
      const browserMetrics = (await cdp.send("Performance.getMetrics")).metrics;
      const result = await page.evaluate(() => {
        const { stats, elapsedMs, finalThreads, finalRows, rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch } = window.__pushReplay;
        const percentile = (values, fraction) => [...values].sort((a, b) => a - b)[Math.max(0, Math.ceil(values.length * fraction) - 1)] ?? null;
        return { ...stats, elapsedMs, getsPerSecond: stats.gets / (elapsedMs / 1000),
          cacheP95Ms: percentile(stats.cacheLatencies, .95), cacheMaxMs: Math.max(...stats.cacheLatencies),
          realPaintP95Ms: percentile(stats.realPaintLatencies, .95), realPaintMaxMs: Math.max(...stats.realPaintLatencies), renderP95Ms: percentile(stats.renderLatencies, .95), renderMaxMs: Math.max(...stats.renderLatencies), rendered, renderedText, exactFinalMessages, exactFinalStates, messageMismatch,
          sequences: finalThreads.map((thread) => thread?.items.map((item) => item.data.sequence)),
          finalFrames: finalRows.map((row) => row?.replay_frame) };
      });
      return { ...result, browserMetrics, pageErrors };
    });
  } finally { await session.close(); }
}
