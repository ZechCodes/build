import { loadBrowserModules, mountLayout } from "./layoutHarness.mjs";
import { SHELL_HTML, SHELL_STYLES } from "./filesExplorerSeed.mjs";

export const PROJECT_KEY = "layout-device/layout-project";
export const ROW_KEYS = ["workspace:layout-device/attention", "workspace:layout-device/running",
  "task:layout-device/layout-project/380", "capture:failed", "capture:routing",
  "project-agent:layout-device/layout-project", "workspace:layout-device/read", "branch:quiet"];

/** An isolated inbox using the production row, project model and head painters.
 * No account, device session, pairing or bridge is involved. */
export async function mountStatusDotInbox(page, basePath, {
  grouped = false, folded = false, ownUnread = 2, ownWorking = false,
} = {}) {
  await mountLayout(page, `<div id="shell"><aside id="inbox-rail" aria-label="Inbox">
    <div class="inbox-head"><span class="dim">Inbox</span></div><div id="inbox-list"></div>
    </aside><main id="view"></main></div>`, { basePath });
  await loadBrowserModules(page, {
    rows: "src/core/inbox.js", projects: "src/core/inboxProjects.js",
  }, basePath);
  await page.evaluate(({ grouped, folded, ownUnread, ownWorking, projectKey }) => {
    const { rows, projects } = window.__layoutModules;
    const now = Date.now();
    const entry = (key, kind, name, over = {}) => ({
      key, kind, name, title: name, projectId: "layout-project", project: "Build",
      projectKey, deviceId: "layout-device", entityId: key, state: "inactive", working: false,
      unreadCount: 0, ready: false, canFinish: false, facts: "+12 / −3", reason: "",
      route: { name: "workspace", workspaceId: key, deviceId: "layout-device" },
      anchorMs: now - 100_000, lastActivityMs: now - 10_000, ...over,
    });
    const entries = [
      entry("workspace:layout-device/attention", "workspace", "Inbox rows: keep the whole title clear", {
        state: "working", working: true, unreadCount: 3, ready: true, canFinish: true,
      }),
      entry("workspace:layout-device/running", "workspace", "Running without unread activity", {
        state: "working", working: true,
      }),
      entry("task:layout-device/layout-project/380", rows.TRACKER_TASK, "#380 Inbox rows: status dot", {
        state: "unread", unreadCount: 1,
      }),
      entry("capture:failed", "capture", "Failed capture: retry next to its status", {
        captureId: "failed", captureState: "failed", state: "unread", unreadCount: 1,
      }),
      entry("capture:routing", "capture", "Capture being routed without unread", {
        captureId: "routing", captureState: "routing", state: "working", working: true,
      }),
      entry(`project-agent:${projectKey}`, rows.PROJECT_AGENT, "Build", {
        state: ownWorking ? "working" : ownUnread ? "unread" : "inactive",
        working: ownWorking, unreadCount: ownUnread, facts: "",
      }),
      entry("workspace:layout-device/read", "workspace", "Read and idle workspace", { facts: "Getting started" }),
      entry("branch:quiet", "branch", "Quiet row with unread activity and menu", {
        state: "unread", unreadCount: 1, lastActivityMs: now - 2 * 86_400_000,
      }),
    ];
    const host = document.querySelector("#inbox-list");
    const ui = { showProject: !grouped };
    const paint = (row) => rows.inboxRowHtml(row, { ...ui, quiet: row.key === "branch:quiet" });
    if (!grouped) host.innerHTML = entries.map(paint).join("");
    else {
      const project = { id: "layout-project", projectKey, deviceId: "layout-device", name: "Build",
        session_started_ms: now - 100_000, last_activity_ms: now - 10_000 };
      const { blocks } = projects.workspaceProjectBlocks(entries, [project]);
      const fold = { folded: new Set(folded ? [projectKey] : []) };
      host.innerHTML = `<div class="inbox-projects">${projects.projectBlockHtml(blocks[0], fold)}</div>`;
      host.querySelector(".inbox-project-rows").innerHTML = entries
        .filter((row) => row.kind !== rows.PROJECT_AGENT).map(paint).join("");
    }
    if (innerWidth <= 900) document.body.classList.add("inbox-collapsed", "inbox-popover-open");
    window.__statusDotEntries = entries;
  }, { grouped, folded, ownUnread, ownWorking, projectKey: PROJECT_KEY });
  await page.evaluate(() => document.fonts.ready);
}

export async function inboxTextPositions(page) {
  return page.locator("#inbox-list .inbox-entry:visible").evaluateAll((rows) => Object.fromEntries(rows.map((row) => {
    const name = row.querySelector(".stitle").getBoundingClientRect();
    return [row.dataset.key, { left: name.left, top: name.top }];
  })));
}

/** A real project Workspaces page with a cached reclaimable workspace. Its
 * in-memory RPC keeps Reclaim pending so both button labels can be measured. */
export async function mountReclaimStatusDot(page, basePath) {
  await mountLayout(page, SHELL_HTML, { basePath, styles: SHELL_STYLES });
  await loadBrowserModules(page, {
    app: "src/app.js", project: "src/views/projectView.js", contexts: "src/core/deviceContexts.js",
    cache: "src/core/localCache.js", feed: "src/core/taskFeed.js", adapter: "src/core/bridgeApi/v1/index.js",
  }, basePath);
  await page.evaluate(async () => {
    const { app, project, contexts, cache, feed, adapter } = window.__layoutModules;
    const deviceId = "reclaim-device";
    const projectKey = `${deviceId}/reclaim-project`;
    const stamp = { deviceId, projectKey, project_id: "reclaim-project" };
    const record = { ...stamp, id: "reclaim-project", name: "Build" };
    const workspace = { ...stamp, id: "reclaim-workspace", workspaceKey: `${deviceId}/reclaim-workspace`,
      name: "Workspace with a deliberately long name that stays clear of Reclaim and its status dot",
      status: "ready", entity_id: "reclaim-run", managed: true,
      directories: [{ source_id: "repo", branch: "build/status-dot", is_git: true }],
      lifecycle: { idle: true, reclaimable: true, holds: [], tasks: [], dirty_files: 0,
        unpushed_commits: 0, behind_commits: 0, size_bytes: 17_200_000_000, measured_at_ms: 1 },
    };
    const run = { ...stamp, kind: "branch", run_id: "reclaim-run", working: true, unread: true,
      agents: [{ id: "reclaim-agent", watched: true, working: true, unread_count: 1 }] };
    app.App.devices = [{ id: deviceId, name: "Fixture device", status: "online" }];
    app.App.route = { name: "project", deviceId, projectId: "reclaim-project", tab: "workspaces" };
    const call = async (method) => method === "workspace.reclaim" ? new Promise(() => {}) : {};
    const context = contexts.adoptDeviceSession({ deviceId, call, close() {}, peer() {}, onCarrier() {}, onPush() {} });
    contexts.adoptBridgeSelection(context, { version: "3.9.0" }, adapter.create(call, { api_version: "3.9.0", capabilities: [] }));
    await cache.writeCached({ deviceId, entityId: "", kind: "projects" }, [record]);
    await cache.writeCached({ deviceId, entityId: "", kind: "workspaces" }, [workspace]);
    await cache.writeCached({ deviceId, entityId: "", kind: "feed" }, { items: [run], runs: [run] });
    await feed.startFeed();
    await project.renderProject();
  });
  await page.locator("[data-workspace-reclaim]").waitFor({ state: "attached" });
}
