/** @vitest-environment jsdom */
// One task's page: the task, the timeline of comments and events interleaved,
// the composer, and the rail of everything about it that can be changed.
//
// Every rail control writes ONE field through the smallest verb that says what
// it means. The page never sends a whole record.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, task } from "./trackerWireFixture.js";
import tasksGetFixture from "../../fixtures/api/v1/tasks.get.json";

let watchers = [];
// The real module refuses a registration that names a cadence — nothing in
// this client polls — so this stand-in refuses one too. A poll creeping back
// into a surface fails here rather than only in a browser.
const RETIRED = ["intervalMs", "keepPolling", "catchUpOnVisible"];
vi.mock("../src/core/changeEvents.js", () => ({
  // The greeting says which kinds a bridge carries; a stand-in that
  // answers none would have the sync layer ask for none of the new ones.
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: carriedKinds },
    // #57: whether this bridge can carry files on a task.
    tasks: { attachments: carriesAttachments, watching: carriesWatching },
  }),
  watchChanges: (registration) => {
    const named = RETIRED.filter((option) => option in registration);
    if (named.length) throw new TypeError(`watchChanges does not poll: remove ${named.join(", ")}`);
    const watcher = { ...registration, disposed: false };
    watchers.push(watcher);
    return { dispose: () => { watcher.disposed = true; } };
  },
}));

/** What this device's greeting says its subscriptions carry. A case naming a
 *  shorter list is an older bridge answering, not another machine. */
const EVERY_KIND = ["state", "thread", "git", "files", "terminals", "tasks"];
let carriedKinds = EVERY_KIND;
/** Whether the bridge under test has `tasks.attach` (1.8). */
let carriesAttachments = true;
let carriesWatching = false;

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

/** The machine this page is reading, as core/transientRead.js asks about it.
 *  A case moves it; `reconnect()` is the session coming back. */
let away = true;
let reconnecting = true;
let movedListeners = new Set();
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({
    away: () => away,
    reconnecting: () => reconnecting,
    moved: (fn) => {
      movedListeners.add(fn);
      return () => movedListeners.delete(fn);
    },
  }),
}));

const reconnect = async () => {
  away = false;
  reconnecting = false;
  [...movedListeners].forEach((fn) => fn());
  await flush();
};

const openAssigneePicker = vi.fn(() => ({ close: vi.fn(), setCatalog: vi.fn() }));
vi.mock("../src/core/trackerAssigneePicker.js", () => ({
  openAssigneePicker: (...args) => openAssigneePicker(...args),
}));

const PROJECT_KEY = "dev-1|proj-1";

const feed = {
  workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "wire-facade", projectKey: PROJECT_KEY, entity_id: "run-1" }],
  items: [{ kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: "agent-1", ordinal: 1 }] }],
};

const TIMELINE = [
  event({ id: "te-1", kind: "created", at: "2026-08-21T10:00:00Z" }),
  comment({ id: "tc-1", created_at: "2026-08-21T10:01:00Z", body: "This reproduces on a phone too." }),
  event({
    id: "te-2", kind: "moved", at: "2026-08-21T10:02:00Z",
    actor: { kind: "agent", agent_id: "agent-1" }, payload: { from: "backlog", to: "in_progress" },
  }),
  comment({ id: "tc-2", created_at: "2026-08-21T10:03:00Z", author: { kind: "agent", agent_id: "agent-1" }, body: "On it." }),
];

let host, call, page, trackerCache, mountTaskPage;
/** A write this bridge refuses, set by the case that wants one. The caller is
 *  captured at mount, so a case cannot hand over a new `call` afterwards. */
let refuses = null;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const answerFor = (over = {}, timeline = TIMELINE) => ({
  task: task({ id: "task-1", number: 12, status: "in_progress", ...over }),
  timeline,
});

const mount = async (over = {}, { waitForPaint = true } = {}) => {
  page = mountTaskPage(host, {
    projectId: "proj-1",
    deviceId: "dev-1",
    projectKey: PROJECT_KEY,
    taskId: "task-1",
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => feed,
    navigate: vi.fn(),
    ...over,
  });
  if (waitForPaint) await flush();
  return page;
};

const listed = (method) => call.mock.calls.filter(([name]) => name === method);

/** What each timeline row SAYS: who and what, without the avatar's initial or
 *  the time beside it — those are marks, not the sentence. */
const entries = () =>
  [...host.querySelectorAll(".task-entry")].map((row) => {
    const who = row.querySelector(".task-entry-head strong")?.textContent || "";
    const said = row.querySelector(".task-comment-body")?.textContent;
    return said === undefined
      ? row.querySelector(".task-event-text").textContent.replace(/\s+/g, " ").trim()
      : `${who} ${said}`.trim();
  });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  watchers = [];
  carriedKinds = EVERY_KIND;
  carriesAttachments = true;
  carriesWatching = false;
  away = true;
  reconnecting = true;
  movedListeners = new Set();
  notifyError.mockClear();
  openAssigneePicker.mockClear();
  feed.workspaces = [{ id: "ws-1", workspace_id: "ws-1", name: "wire-facade", projectKey: PROJECT_KEY, entity_id: "run-1" }];
  feed.items = [{ kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: "agent-1", ordinal: 1 }] }];
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountTaskPage } = await import("../src/core/trackerTaskPage.js"));
  await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
  refuses = null;
  call = vi.fn(async (method) => {
    if (refuses?.method === method) throw new Error(refuses.message);
    return method === "tasks.get" ? answerFor() : {};
  });
});

afterEach(() => {
  page?.dispose();
});

describe("mounted task identity links", () => {
  it("links an unwatched prose author, then removes dead routes when the workspace cache changes", async () => {
    const agentId = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
    const workspaceId = "ws-3f2a91c4";
    const agent = { kind: "agent", agent_id: agentId };
    feed.items = [];
    feed.workspaces = [{ id: workspaceId, workspace_id: workspaceId,
      name: "spa-flaky-tests", projectKey: PROJECT_KEY, entity_id: "run-unwatched" }];
    const wire = structuredClone(tasksGetFixture.result.task);
    wire.id = "task-1";
    wire.body = `Ask @agent:${agentId} on this task.`;
    wire.assignee = agent;
    wire.links.workspace_ids = [workspaceId];
    call = vi.fn(async (method) => method === "tasks.get" ? {
      task: wire,
      timeline: [{ type: "comment", id: "tc-unwatched", author: agent,
        body: "I can take this.", created_at: "2026-09-19T10:12:00Z" }],
    } : {});
    // What a reference in the body names comes off the index the app fills
    // from this same feed (core/referenceIndexFeed.js, #229).
    const index = await import("../src/core/referenceIndex.js");
    index.holdReferenceSources({ feed, tasks: {} });
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-comment .task-entry-head a[href*='agent=']")).not.toBeNull());
    expect(host.querySelector(".task-page-body a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".task-assignee-current a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".task-links a[href*='workspace']")).not.toBeNull();

    feed.workspaces = [];
    index.holdReferenceSources({ feed: { ...feed }, tasks: {} });
    page.feedMoved();
    expect(host.querySelector(".task-comment .task-entry-head a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".task-page-body a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".task-assignee-current a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".task-links a[href*='workspace']")).toBeNull();
    expect(host.querySelector(".task-comment .task-entry-head").textContent).toContain("spa-flaky-tests · Fix drag");
  });
});

describe("task unread navigation", () => {
  it("does not mark a task read in a hidden tab, and marks it when the tab is shown", async () => {
    carriesWatching = true;
    const timeline = [
      event({ id: "te-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB02", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", read_through: timeline[0].id }),
      timeline,
    });
    call = vi.fn((method) => method === "tasks.get" ? new Promise(() => {}) : Promise.resolve({}));
    const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(true);
    try {
      await mount({}, { waitForPaint: false });
      await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull());
      expect(listed("tasks.read_through")).toEqual([]);
      hidden.mockReturnValue(false);
      document.dispatchEvent(new Event("visibilitychange"));
      await vi.waitFor(() => expect(listed("tasks.read_through")).toHaveLength(1));
      expect(listed("tasks.read_through")[0][1].event_id).toBe(timeline[1].id);
    } finally {
      hidden.mockRestore();
    }
  });

  it("does not mark a task read in a visible unfocused window, and marks it on focus", async () => {
    carriesWatching = true;
    const timeline = [
      event({ id: "te-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB02", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", read_through: timeline[0].id }),
      timeline,
    });
    call = vi.fn((method) => method === "tasks.get" ? new Promise(() => {}) : Promise.resolve({}));
    const focused = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    try {
      await mount({}, { waitForPaint: false });
      await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull());
      expect(listed("tasks.read_through")).toEqual([]);
      focused.mockReturnValue(true);
      window.dispatchEvent(new Event("focus"));
      await vi.waitFor(() => expect(listed("tasks.read_through")).toHaveLength(1));
      expect(listed("tasks.read_through")[0][1].event_id).toBe(timeline[1].id);
    } finally {
      focused.mockRestore();
    }
  });

  it("does not claim unread history on a bridge without task read marks", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-page-title")).not.toBeNull());
    expect(host.querySelector(".task-unread-line")).toBeNull();
    expect(host.querySelector(".new-messages-pill")).toBeNull();
  });

  it("paints a cached pre-visit mark, then jumps from the floating pill to the first unread entry", async () => {
    const shell = document.createElement("div");
    host.before(shell);
    shell.append(host);
    shell.scrollTo = vi.fn();
    document.documentElement.scrollTo = vi.fn();
    carriesWatching = true;
    const timeline = [
      event({ id: "te-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB02", author: { kind: "user" } }),
      event({ id: "te-01M37FGQD48628P29BG1A4BB03", kind: "moved", payload: { from: "ready", to: "in_progress" },
        actor: { kind: "agent", agent_id: "agent-1" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB04", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", read_through: timeline[0].id }),
      timeline,
    });
    // Keep the live read pending so this assertion can only pass from the
    // actual cached task-page wiring, before this visit's mark returns.
    call = vi.fn((method) => method === "tasks.get" ? new Promise(() => {}) : Promise.resolve({}));
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull());
    const line = host.querySelector(".task-unread-line");
    expect(line.nextElementSibling.classList.contains("task-event")).toBe(true);
    expect(line.previousElementSibling.id).toBe(`comment-${timeline[1].id}`);
    expect(listed("tasks.read_through")[0][1].event_id).toBe(timeline[3].id);

    const hostRect = vi.spyOn(host, "getBoundingClientRect").mockReturnValue({ top: 0, bottom: 600 });
    const lineRect = vi.spyOn(line, "getBoundingClientRect").mockReturnValue({ top: 900 });
    host.scrollTo = vi.fn();
    host.dispatchEvent(new Event("scroll"));
    const pill = host.querySelector(".new-messages-pill");
    expect(pill.getAttribute("aria-label")).toBe("Jump to first unread activity");
    expect(pill.closest(".new-messages-dock").hidden).toBe(false);
    pill.click();
    expect(host.scrollTo).toHaveBeenCalledWith({ top: 900, behavior: "smooth" });
    expect(shell.scrollTo).not.toHaveBeenCalled();
    expect(document.documentElement.scrollTo).not.toHaveBeenCalled();
    expect(pill.closest(".new-messages-dock").hidden).toBe(true);

    lineRect.mockReturnValue({ top: 100 });
    host.dispatchEvent(new Event("scroll"));
    lineRect.mockReturnValue({ top: 900 });
    host.dispatchEvent(new Event("scroll"));
    expect(pill.closest(".new-messages-dock").hidden).toBe(false);
    lineRect.mockReturnValue({ top: 100 });
    host.dispatchEvent(new Event("scroll"));
    expect(pill.closest(".new-messages-dock").hidden).toBe(true);
    hostRect.mockRestore();
    lineRect.mockRestore();
  });

  it("uses an accepted cached read floor on revisit, even after a stale detail repaint", async () => {
    carriesWatching = true;
    const timeline = [
      event({ id: "te-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB02", author: { kind: "agent", agent_id: "agent-1" } }),
      comment({ id: "tc-01M37FGQD48628P29BG1A4BB03", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    const stale = answerFor({ read_through: timeline[0].id }, timeline);
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", stale);
    // Leave tasks.get outstanding across both visits: only the cache can
    // paint, and the accepted mark must outlive the first page instance.
    call = vi.fn((method) => method === "tasks.get" ? new Promise(() => {}) : Promise.resolve({ task: {
      read_through: timeline[2].id,
    } }));
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull());
    await vi.waitFor(async () => expect((await trackerCache.readTaskRecord("dev-1", "proj-1", "task-1"))?.task.read_through)
      .toBe(timeline[2].id));

    page.dispose();
    const seen = [];
    await mount({ onTaskRead: (one) => seen.push(one.read_through) }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-page-title")).not.toBeNull());
    expect(host.querySelector(".task-unread-line")).toBeNull();
    expect(host.querySelector(".new-messages-pill")).toBeNull();

    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", stale);
    expect((await trackerCache.readTaskRecord("dev-1", "proj-1", "task-1")).task.read_through).toBe(timeline[2].id);
    await vi.waitFor(() => expect(seen.length).toBeGreaterThan(1));
    expect(host.querySelector(".task-unread-line")).toBeNull();
  });

  it("keeps read floors monotonic and scoped to one device, project and task", async () => {
    const first = "te-01M37FGQD48628P29BG1A4BB01";
    const later = "tc-01M37FGQD48628P29BG1A4BB03";
    for (const [device, project, id] of [
      ["dev-1", "proj-1", "task-1"], ["dev-2", "proj-1", "task-1"],
      ["dev-1", "proj-2", "task-1"], ["dev-1", "proj-1", "task-2"],
    ]) await trackerCache.writeTaskRecord(device, project, id, answerFor({ id, read_through: first }));
    await trackerCache.advanceTaskReadThrough("dev-1", "proj-1", "task-1", later);
    await trackerCache.advanceTaskReadThrough("dev-1", "proj-1", "task-1", first);
    await trackerCache.advanceTaskReadThrough("dev-1", "proj-1", "task-1", "comment-3");
    expect((await trackerCache.readTaskRecord("dev-1", "proj-1", "task-1")).task.read_through).toBe(later);
    for (const [device, project, id] of [
      ["dev-2", "proj-1", "task-1"], ["dev-1", "proj-2", "task-1"], ["dev-1", "proj-1", "task-2"],
    ]) expect((await trackerCache.readTaskRecord(device, project, id)).task.read_through).toBe(first);
  });
});

describe("the task", () => {
  it("reads it by id and draws its title and number", async () => {
    await mount();
    expect(listed("tasks.get")[0][1]).toEqual({ task_id: "task-1" });
    expect(host.querySelector(".task-page-title").textContent).toBe("Kanban drag does not persist");
    expect(host.querySelector(".task-number").textContent).toBe("#12");
  });

  it("renders the body as markdown", async () => {
    call = vi.fn(async () => answerFor({ body: "Dragging a card **does not** persist." }));
    await mount();
    expect(host.querySelector(".task-page-body strong").textContent).toBe("does not");
  });

  it("says so rather than leaving a gap when there is no description", async () => {
    call = vi.fn(async () => answerFor({ body: "" }));
    await mount();
    expect(host.querySelector(".task-page-body").textContent).toBe("No description.");
  });

  // Open/closed is independent of the Done column: both are shown.
  it("shows the state and the column as two separate facts", async () => {
    call = vi.fn(async () => answerFor({ state: "closed", status: "in_progress" }));
    await mount();
    expect(host.querySelector(".task-page-state").textContent).toBe("Closed");
    expect(host.querySelector("#task-status").value).toBe("in_progress");
  });

  it("paints from the cache before the bridge answers", async () => {
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", number: 12, title: "From the cache" }),
      timeline: [],
    });
    let settle;
    call = vi.fn((method) => (method === "tasks.get" ? new Promise((resolve) => { settle = resolve; }) : Promise.resolve({})));
    await mount();
    expect(host.querySelector(".task-page-title").textContent).toBe("From the cache");
    settle(answerFor());
    await flush();
    expect(host.querySelector(".task-page-title").textContent).toBe("Kanban drag does not persist");
  });

  it("writes what it read back through the cache, so the revisit is warm", async () => {
    await mount();
    const held = await trackerCache.readTaskRecord("dev-1", "proj-1", "task-1");
    expect(held.task.number).toBe(12);
    expect(held.timeline).toHaveLength(4);
  });

  it("restores an unsent comment and clears it only after a successful send", async () => {
    const { readUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const address = uiAddress({ deviceId: "dev-1", entityId: "task-1", view: "tracker-task", kind: "draft", sub: "proj-1" });
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    field.value = "Keep this thought";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.waitFor(async () => expect((await readUiRecord(address))?.value.body).toBe("Keep this thought"));
    page.dispose();
    await mount({ taskId: "task-2" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    expect(host.querySelector("#task-comment").value).toBe("");
    page.dispose();
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")?.value).toBe("Keep this thought"));
    host.querySelector("[data-task-composer]").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    await vi.waitFor(async () => expect((await readUiRecord(address))?.value.body).toBe(""));
  });

  it("repaints from a task-cache write while tasks.get stays absent", async () => {
    call = vi.fn(() => new Promise(() => {}));
    await mount();
    expect(host.querySelector(".task-page-title")).toBeNull();

    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", number: 12, title: "Announced detail" }),
      timeline: [],
    });
    await flush();

    expect(host.querySelector(".task-page-title").textContent).toBe("Announced detail");
  });
});

describe("the timeline", () => {
  it("renders a mentioned comment from the bridge's tasks.get fixture", async () => {
    call = vi.fn(async () => tasksGetFixture.result);
    await mount({ taskId: tasksGetFixture.params.task_id }, { waitForPaint: false });

    const mentioned = tasksGetFixture.result.timeline.find((entry) => entry.type === "comment" && entry.mentions_user);
    await vi.waitFor(() => expect(host.querySelector(`#comment-${mentioned.id}`)).not.toBeNull());
    const row = host.querySelector(`#comment-${mentioned.id}`);
    expect(row?.classList.contains("task-comment-mentioned")).toBe(true);
    expect(row.querySelector(".task-comment-body").textContent).toBe(mentioned.body);
  });

  it("keeps a mentioned comment marked through cached, live, and read-state paints", async () => {
    const mentioned = comment({ id: "tc-mention", author: { kind: "agent", agent_id: "agent-1" }, mentions_user: true });
    const plain = comment({ id: "tc-plain", author: { kind: "agent", agent_id: "agent-1" } });
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", answerFor({}, [mentioned, plain]));
    let settle;
    call = vi.fn((method) => method === "tasks.get"
      ? new Promise((resolve) => { settle = resolve; })
      : Promise.resolve({}));
    await mount({}, { waitForPaint: false });

    const marked = () => host.querySelector("#comment-tc-mention");
    const unmarked = () => host.querySelector("#comment-tc-plain");
    await vi.waitFor(() => {
      expect(marked()).not.toBeNull();
      expect(unmarked()).not.toBeNull();
      expect(listed("tasks.get")).toHaveLength(1);
    });
    expect(settle).toBeTypeOf("function");
    expect(marked().classList.contains("task-comment-mentioned")).toBe(true);
    expect(unmarked().classList.contains("task-comment-mentioned")).toBe(false);

    settle(answerFor({ read_through: "tc-mention" }, [{ ...mentioned, body: "New live wording" }, plain]));
    await vi.waitFor(() => expect(marked().querySelector(".task-comment-body").textContent).toBe("New live wording"));
    expect(marked().classList.contains("task-comment-mentioned")).toBe(true);

    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", answerFor({}, [
      { ...mentioned, mentions_user: false }, { ...plain, mentions_user: true },
    ]));
    await vi.waitFor(() => expect(unmarked().classList.contains("task-comment-mentioned")).toBe(true));
    expect(marked().classList.contains("task-comment-mentioned")).toBe(false);
  });

  it("scrolls to and highlights a routed comment after the cache paints", async () => {
    host.scrollTo = vi.fn();
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", answerFor());
    call = vi.fn(() => new Promise(() => {}));
    await mount({ commentId: "tc-2" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#comment-tc-2")?.classList.contains("task-comment-target")).toBe(true));
    expect(host.scrollTo).toHaveBeenCalledWith({ top: 0, behavior: "smooth" });
  });

  it("opens a missing comment at the top without an error", async () => {
    await mount({ commentId: "tc-missing" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-page-title")?.textContent).toBe("Kanban drag does not persist"));
    expect(host.querySelector(".task-comment-target")).toBeNull();
    expect(host.scrollTop).toBe(0);
    expect(notifyError).not.toHaveBeenCalled();
  });

  // Comments and events interleave into one ascending list, in the order the
  // bridge answered them.
  it("interleaves comments and events in the order they arrived", async () => {
    await mount();
    expect(entries()).toEqual([
      "You filed this",
      "You This reproduces on a phone too.",
      "wire-facade · Agent 1 moved this from Backlog to In progress",
      "wire-facade · Agent 1 On it.",
    ]);
  });

  it("names an agent by its workspace and its place on that workspace's strip", async () => {
    await mount();
    expect(host.querySelectorAll(".task-comment strong")[1].textContent).toBe("wire-facade · Agent 1");
  });

  it("says nothing has happened yet when the timeline is empty", async () => {
    call = vi.fn(async () => answerFor({}, []));
    await mount();
    expect(host.querySelector(".task-empty").textContent).toContain("Nothing has happened");
  });
});

describe("the composer", () => {
  const pressEnter = (field, modifiers = {}) => {
    const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...modifiers });
    field.dispatchEvent(event);
    return event;
  };

  const typeComment = (field, body) => {
    field.value = body;
    field.dispatchEvent(new Event("input", { bubbles: true }));
  };

  it("sends a comment and re-reads the task", async () => {
    await mount();
    const field = host.querySelector("#task-comment");
    field.value = "  Looking at the drag handler.  ";
    field.dispatchEvent(new Event("input"));
    call.mockClear();
    host.querySelector("[data-task-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(listed("tasks.comment")[0][1]).toEqual({ task_id: "task-1", body: "Looking at the drag handler." });
    expect(listed("tasks.get")).toHaveLength(1);
  });

  it("sends Ctrl+Enter from the mounted comment box through tasks.comment", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    typeComment(field, "From the keyboard");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true }).defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    expect(listed("tasks.comment")[0][1]).toEqual({ task_id: "task-1", body: "From the keyboard" });
  });

  it("sends Cmd+Enter while leaving plain Enter available for a newline", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    typeComment(field, "Mac comment");
    call.mockClear();

    expect(pressEnter(field).defaultPrevented).toBe(false);
    expect(field.value).toBe("Mac comment");
    expect(listed("tasks.comment")).toHaveLength(0);
    expect(pressEnter(field, { metaKey: true }).defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    expect(listed("tasks.comment")[0][1].body).toBe("Mac comment");
  });

  it("does not send an empty comment on Ctrl+Enter", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    typeComment(field, "   ");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(listed("tasks.comment")).toHaveLength(0);
  });

  it("ignores Ctrl+Enter during IME composition", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    typeComment(field, "Still composing");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true, isComposing: true }).defaultPrevented).toBe(false);
    expect(listed("tasks.comment")).toHaveLength(0);
  });

  it("does not post twice when Ctrl+Enter is pressed again while sending", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#task-comment")).not.toBeNull());
    const field = host.querySelector("#task-comment");
    typeComment(field, "Only once");
    let finishSend;
    call.mockImplementation((method) => method === "tasks.comment"
      ? new Promise((resolve) => { finishSend = resolve; })
      : Promise.resolve(method === "tasks.get" ? answerFor() : {}));
    call.mockClear();

    pressEnter(field, { ctrlKey: true });
    pressEnter(field, { ctrlKey: true });
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    const sendingField = host.querySelector("#task-comment");
    expect(sendingField.disabled).toBe(true);
    pressEnter(sendingField, { ctrlKey: true });
    expect(listed("tasks.comment")).toHaveLength(1);
    finishSend({});
    await vi.waitFor(() => expect(host.querySelector("#task-comment").disabled).toBe(false));
    expect(listed("tasks.comment")).toHaveLength(1);
  });

  it("will not send an empty one", async () => {
    await mount();
    expect(host.querySelector('.task-composer button[type="submit"]').disabled).toBe(true);
  });

  // #57: the comment box takes files too, and only against a bridge that can
  // carry them. A press that cannot work is worse than no press.
  it("offers the paperclip before the bridge has announced file support", async () => {
    await mount();
    expect(host.querySelector(".task-composer .composer-attach")).not.toBeNull();
    expect(host.querySelector(".task-composer .composer.attachable")).not.toBeNull();

    page?.dispose?.();
    carriesAttachments = false;
    document.body.innerHTML = '<div id="page"></div>';
    host = document.querySelector("#page");
    await mount();
    expect(host.querySelector(".task-composer .composer-attach")).not.toBeNull();
    expect(host.querySelector(".task-composer .composer-tray")).not.toBeNull();
    expect(host.querySelector(".task-composer .composer-dropmask")).not.toBeNull();
    expect(host.querySelector("#task-comment")).not.toBeNull();
  });

  it("reverts a refused watch and explains an unsupported command plainly", async () => {
    await mount();
    refuses = { method: "tasks.watch", message: "unknown method: tasks.watch" };
    host.querySelector(".rail-watch").click();
    await vi.waitFor(() => expect(notifyError).toHaveBeenCalled());
    expect(host.querySelector(".rail-watch").getAttribute("aria-pressed")).toBe("false");
    expect(notifyError).toHaveBeenCalledWith(
      "Could not change whether you are watching this task",
      "This bridge does not support watching tasks.",
    );
  });

  it("keeps the comment draft when an older bridge refuses an attachment", async () => {
    carriesAttachments = false;
    await mount();
    const field = host.querySelector("#task-comment");
    field.value = "Keep this draft";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    refuses = { method: "tasks.attach", message: "unknown method: tasks.attach" };
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: { files: [new File(["png"], "shot.png", { type: "image/png" })] } });
    host.querySelector("[data-task-composer]").dispatchEvent(drop);
    await vi.waitFor(() => expect(notifyError).toHaveBeenCalled());
    expect(field.value).toBe("Keep this draft");
    expect(host.querySelector(".composer-chip.failed .composer-chip-note")?.textContent)
      .toBe("This bridge does not support task attachments.");
    expect(notifyError).toHaveBeenCalledWith(
      "Could not attach that file", "shot.png: This bridge does not support task attachments.",
    );
  });

  it("sends a dropped file with the comment, through tasks.attach", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => {
      expect(host.querySelector("[data-task-composer]")).not.toBeNull();
      expect(host.querySelector("#task-comment")?.disabled).toBe(false);
      expect(host.querySelector(".task-composer .composer.attachable")).not.toBeNull();
    });
    call.mockImplementation(async (method) => {
      if (method === "tasks.attach") {
        return { name: "shot.png", path: "/store/abc-shot.png", mime: "image/png", size: 3 };
      }
      return method === "tasks.get" ? answerFor() : {};
    });
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", {
      value: { files: [new File(["png"], "shot.png", { type: "image/png" })] },
    });
    host.querySelector("[data-task-composer]").dispatchEvent(event);
    await vi.waitFor(() => {
      expect(host.querySelector(".task-composer .composer-chip.ready .composer-chip-name")?.textContent).toBe("shot.png");
      expect(host.querySelector('.task-composer button[type="submit"]')?.disabled).toBe(false);
    });
    expect(listed("tasks.attach")[0][1]).toMatchObject({ project_id: "proj-1", filename: "shot.png" });

    // A comment that is only a screenshot is a comment: the press turns on
    // with an empty box.
    call.mockClear();
    pressEnter(host.querySelector("#task-comment"), { ctrlKey: true });
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    expect(listed("tasks.comment")[0][1]).toEqual({
      task_id: "task-1",
      body: "",
      attachments: [{ name: "shot.png", path: "/store/abc-shot.png", mime: "image/png", size: 3 }],
    });
  });

  it("says why a refused comment did not land", async () => {
    await mount();
    const field = host.querySelector("#task-comment");
    field.value = "hello";
    field.dispatchEvent(new Event("input"));
    refuses = { method: "tasks.comment", message: "task is closed" };
    host.querySelector("[data-task-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(notifyError).toHaveBeenCalledWith("Could not add this comment", "task is closed");
  });
});

describe("the rail", () => {
  const railWrite = async (act) => {
    call.mockClear();
    await act();
    await flush();
  };

  it("closes an open task and reopens a closed one", async () => {
    await mount();
    expect(host.querySelector("[data-task-state]").textContent).toBe("Close task");
    await railWrite(() => host.querySelector("[data-task-state]").click());
    expect(listed("tasks.close")[0][1]).toEqual({ task_id: "task-1" });
  });

  it("reopens rather than closing when the task is already closed", async () => {
    call = vi.fn(async () => answerFor({ state: "closed" }));
    await mount();
    expect(host.querySelector("[data-task-state]").textContent).toBe("Reopen task");
    await railWrite(() => host.querySelector("[data-task-state]").click());
    expect(listed("tasks.reopen")[0][1]).toEqual({ task_id: "task-1" });
  });

  // Closing does not move it to Done and Done does not close it; the rail says
  // so rather than leaving the reader to find out.
  it("says that closing does not move it to Done", async () => {
    await mount();
    expect(host.querySelector(".task-rail").textContent).toContain("Closing does not move it to Done.");
  });

  it("moves the task through the column select, sending only that field", async () => {
    await mount();
    await railWrite(() => {
      const select = host.querySelector("#task-status");
      select.value = "in_review";
      select.dispatchEvent(new Event("change"));
    });
    expect(listed("tasks.update")[0][1]).toEqual({ task_id: "task-1", status: "in_review" });
  });

  it("sets the priority, sending only that field", async () => {
    await mount();
    await railWrite(() => {
      const select = host.querySelector("#task-priority");
      select.value = "high";
      select.dispatchEvent(new Event("change"));
    });
    expect(listed("tasks.update")[0][1]).toEqual({ task_id: "task-1", priority: "high" });
  });

  it("saves the labels on Enter, as the list the verb stores", async () => {
    await mount();
    const field = host.querySelector("#task-labels");
    field.value = " bug , ui , bug ";
    await railWrite(() => field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", cancelable: true })));
    expect(listed("tasks.update")[0][1]).toEqual({ task_id: "task-1", labels: ["bug", "ui"] });
  });

  it("opens the assignee picker, standing on who holds it", async () => {
    call = vi.fn(async () => answerFor({ assignee: { kind: "agent", agent_id: "agent-1" } }));
    await mount();
    host.querySelector("[data-task-assign]").click();
    await flush();
    expect(openAssigneePicker.mock.calls[0][0].current).toBe("agent:agent-1");
  });

  it("says that assigning starts an agent, before it is pressed", async () => {
    await mount();
    expect(host.querySelector(".task-rail").textContent).toContain("Assigning hands the task to an agent and starts it.");
  });
});

describe("the links", () => {
  const linked = async (links) => {
    call = vi.fn(async () => answerFor({ links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_task_id: null, ...links } }));
    await mount();
    return [...host.querySelectorAll(".task-link")];
  };

  it("opens the workspace it is being worked in, by that workspace's name", async () => {
    const rows = await linked({ workspace_ids: ["ws-1"] });
    expect([rows[0].textContent, rows[0].querySelector("a").getAttribute("href")]).toEqual([
      "wire-facade", "#/device/dev-1/project/proj-1/workspace/ws-1/changes",
    ]);
  });

  it("opens the branch through the branch surface", async () => {
    const rows = await linked({ branches: ["build/tasks-spa"] });
    expect(rows[0].querySelector("a").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/branch/build%2Ftasks-spa/changes");
  });

  // The conversation a dispatch delivered into is one press away.
  it("opens the conversation on the workspace that owns it", async () => {
    const rows = await linked({ conversation_ids: ["run-1"] });
    expect([rows[0].textContent, rows[0].querySelector("a").getAttribute("href")]).toEqual([
      "wire-facade · conversation", "#/device/dev-1/project/proj-1/workspace/ws-1/changes",
    ]);
  });

  // No surface is addressed by a bare hash, so a commit is shown, not linked.
  it("shows a commit rather than linking it nowhere", async () => {
    const rows = await linked({ commits: ["c8381faa9e1b2c3d4e5f60718293a4b5c6d7e8f9"] });
    expect(rows[0].querySelector("a")).toBeNull();
    expect(rows[0].textContent).toBe("c8381fa");
  });

  it("says what a dispatch would link when nothing is linked yet", async () => {
    await mount();
    expect(host.querySelector(".task-rail").textContent).toContain("Nothing linked yet");
  });
});

describe("the push", () => {
  it("subscribes the project for tasks", async () => {
    await mount();
    expect(watchers.map((one) => [one.entity, one.kinds])).toEqual([["proj-1", ["tasks"]]]);
  });

  it("asks a bridge that does not carry tasks for no kind at all", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await mount();
    expect(watchers.map((one) => one.kinds)).toEqual([[]]);
  });

  it("re-reads this task when an item names it", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-1"], truncated: false } }]);
    await flush();
    expect(listed("tasks.get")).toHaveLength(1);
  });

  // The reader is typing a comment when the task moves under them. The page
  // redraws to show the move, and the caret is exactly where it was.
  it("keeps the caret in the comment box across a re-read", async () => {
    await mount();
    const field = host.querySelector("#task-comment");
    field.value = "Reproduced on the phone";
    field.dispatchEvent(new Event("input"));
    const typed = host.querySelector("#task-comment");
    typed.focus();
    typed.setSelectionRange(10, 10);
    call.mockImplementation(async (method) => (method === "tasks.get" ? answerFor({ status: "in_review" }) : {}));
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-1"], truncated: false } }]);
    await flush();

    const after = host.querySelector("#task-comment");
    expect(host.textContent).toContain("In review");
    expect(after.value).toBe("Reproduced on the phone");
    expect(document.activeElement).toBe(after);
    expect(after.selectionStart).toBe(10);
  });

  // The feed moves every time any agent's state does, and most of those moves
  // change nothing on this page. A page that redrew for each one would drop
  // the reader's caret and scroll as often as agents work.
  it("does not redraw when the feed moves without changing what it shows", async () => {
    await mount();
    const before = host.querySelector("#task-comment");
    before.focus();
    page.feedMoved();
    page.feedMoved();
    expect(host.querySelector("#task-comment")).toBe(before);
    expect(document.activeElement).toBe(before);
  });

  it("leaves an item about another task alone", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-9"], truncated: false } }]);
    await flush();
    expect(listed("tasks.get")).toEqual([]);
  });

  // Truncation means "refetch", so a truncated item is news about everything.
  it("re-reads when the ids were dropped as truncated", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: [], truncated: true } }]);
    await flush();
    expect(listed("tasks.get")).toHaveLength(1);
  });
});

// The project agent's rail stands beside this page, so the page says which
// task is open. Only from the READ: the route names an id, and an agent told
// an id and nothing else is no better off.
describe("saying which task is open", () => {
  it("reports the task once it has been read, and not before", async () => {
    const seen = [];
    let settle;
    call = vi.fn((method) => (method === "tasks.get" ? new Promise((resolve) => { settle = resolve; }) : Promise.resolve({})));
    page = mountTaskPage(host, {
      projectId: "proj-1",
      deviceId: "dev-1",
      projectKey: PROJECT_KEY,
      taskId: "task-1",
      callRpc: call,
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => feed,
      navigate: vi.fn(),
      onTaskRead: (task) => seen.push(task),
    });
    await flush();
    expect(seen).toEqual([]);
    settle(answerFor());
    await flush();
    expect(seen.map((one) => [one.number, one.title])).toEqual([[12, "Kanban drag does not persist"]]);
  });

  it("reports it again when a push re-reads it, so a retitled task is not stale", async () => {
    const seen = [];
    await mount({ onTaskRead: (task) => seen.push(task.title) });
    expect(seen).toHaveLength(1);
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-1"], truncated: false } }]);
    await flush();
    expect(seen.length).toBeGreaterThan(1);
  });
});

// #24. A phone's session dies at the network layer every few minutes and every
// call in flight dies with it. The page had already painted from the cache, so
// "Could not read this task" was a complaint about a copy that was on screen
// the whole time.
describe("a read that fails because the session dropped", () => {
  const WENT = "your device went offline";
  const cached = () =>
    trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", number: 12, title: "From the cache" }),
      timeline: [],
    });
  const note = () => host.querySelector(".read-wait")?.textContent ?? null;

  it("keeps the cached copy and says nothing", async () => {
    await cached();
    refuses = { method: "tasks.get", message: WENT };
    await mount();
    expect(host.querySelector(".task-page-title").textContent).toBe("From the cache");
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("marks when that copy was read, while the machine is being reconnected to", async () => {
    await cached();
    refuses = { method: "tasks.get", message: WENT };
    await mount();
    expect(note()).toContain("reconnecting");
  });

  it("reads again when the machine is back, with nothing polled in between", async () => {
    await cached();
    refuses = { method: "tasks.get", message: WENT };
    await mount();
    expect(listed("tasks.get")).toHaveLength(1);
    refuses = null;
    await reconnect();
    expect(listed("tasks.get")).toHaveLength(2);
    expect(host.querySelector(".task-page-title").textContent).toBe("Kanban drag does not persist");
    expect(note()).toBeNull();
    expect(notifyError).not.toHaveBeenCalled();
  });

  // The bridge answered. The reader has to hear that, dropped session or not.
  it("still says a refusal out loud", async () => {
    await cached();
    refuses = { method: "tasks.get", message: "no such task" };
    await mount();
    expect(notifyError).toHaveBeenCalledWith("Could not read this task", "no such task");
  });

  // Nothing on screen to be quiet about: the wait is the same, but its failure
  // is said.
  it("waits with nothing on screen, then says so when the retry fails too", async () => {
    refuses = { method: "tasks.get", message: WENT };
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
    await reconnect();
    expect(notifyError).toHaveBeenCalledWith("Could not read this task", WENT);
  });
});

// #153: typing into the comment box made the page's scroll jump. Each
// keystroke's draft write came back through the cache and repainted the whole
// page, which stood up a new textarea under the reader's fingers. The box is
// made once per mount and updated in place; nothing the reader typed moves the
// scroller.
describe("typing in the comment box", () => {
  const settle = async () => {
    await new Promise((done) => setTimeout(done, 250)); // past the draft's debounce
    await flush();
  };

  /** Every write to the scroller's position, and every scrollTo on it. */
  const watchScroller = () => {
    const writes = [];
    let top = 0;
    Object.defineProperty(host, "scrollTop", {
      configurable: true,
      get: () => top,
      set: (value) => { writes.push(value); top = value; },
    });
    host.scrollTo = vi.fn();
    const windowScroll = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    return { writes, scrollTo: host.scrollTo, windowScroll };
  };

  const typeKey = (field, key) => {
    const at = field.selectionStart;
    field.value = field.value.slice(0, at) + key + field.value.slice(field.selectionEnd);
    field.setSelectionRange(at + 1, at + 1);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  };

  it("keeps one textarea, its focus and its caret, and never moves the scroller", async () => {
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    field.focus();
    const scroller = watchScroller();

    for (const key of "abc") {
      typeKey(field, key);
      await settle();
      expect(host.querySelector("#task-comment")).toBe(field);
      expect(document.activeElement).toBe(field);
    }

    expect(field.value).toBe("abc");
    expect(field.selectionStart).toBe(3);
    expect(field.selectionEnd).toBe(3);
    expect(scroller.writes).toEqual([]);
    expect(scroller.scrollTo).not.toHaveBeenCalled();
    expect(scroller.windowScroll).not.toHaveBeenCalled();
    scroller.windowScroll.mockRestore();
  });

  it("turns the send press on with the first character, without a repaint", async () => {
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    const send = host.querySelector('.task-composer button[type="submit"]');
    expect(send.disabled).toBe(true);
    typeKey(field, "a");
    expect(send.disabled).toBe(false);
    await settle();
    expect(host.querySelector('.task-composer button[type="submit"]')).toBe(send);
  });

  it("keeps the node, the draft and the caret when a comment is pushed mid-sentence", async () => {
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    field.focus();
    for (const key of "Half a") typeKey(field, key);
    field.setSelectionRange(4, 4);
    const scroller = watchScroller();

    const pushed = [...TIMELINE, comment({ id: "tc-3", created_at: "2026-08-21T10:04:00Z", body: "Pushed while typing." })];
    call.mockImplementation(async (method) => (method === "tasks.get" ? answerFor({}, pushed) : {}));
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-1"], truncated: false } }]);
    await settle();

    expect(host.textContent).toContain("Pushed while typing.");
    expect(host.querySelector("#task-comment")).toBe(field);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("Half a");
    expect(field.selectionStart).toBe(4);
    expect(scroller.writes).toEqual([]);
    scroller.windowScroll.mockRestore();
  });

  it("keeps the rows already on screen when a comment is pushed", async () => {
    await mount();
    await settle();
    const rows = [...host.querySelectorAll(".task-entry")];
    const pushed = [...TIMELINE, comment({ id: "tc-3", created_at: "2026-08-21T10:04:00Z", body: "Pushed." })];
    call.mockImplementation(async (method) => (method === "tasks.get" ? answerFor({}, pushed) : {}));
    watchers[0].onChanges([{ entity_id: "proj-1", tasks: { task_ids: ["task-1"], truncated: false } }]);
    await settle();
    const after = [...host.querySelectorAll(".task-entry")];
    expect(after).toHaveLength(rows.length + 1);
    rows.forEach((row, at) => expect(after[at]).toBe(row));
    expect(after.at(-1).textContent).toContain("Pushed.");
  });

  it("clears the box after a send, and restores a saved draft on remount", async () => {
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    typeKey(field, "x");
    await settle();
    page.dispose();
    await mount();
    await settle();
    expect(host.querySelector("#task-comment").value).toBe("x");
    const kept = host.querySelector("#task-comment");
    host.querySelector("[data-task-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await settle();
    expect(listed("tasks.comment")[0][1].body).toBe("x");
    expect(host.querySelector("#task-comment")).toBe(kept);
    expect(kept.value).toBe("");
    expect(kept.disabled).toBe(false);
    expect(host.querySelector('.task-composer button[type="submit"]').disabled).toBe(true);
  });

  it("keeps the timeline's nodes when only the draft changed", async () => {
    await mount();
    await settle();
    const timeline = host.querySelector(".task-timeline");
    const head = host.querySelector(".task-page-head");
    typeKey(host.querySelector("#task-comment"), "x");
    await settle();
    expect(host.querySelector(".task-timeline")).toBe(timeline);
    expect(host.querySelector(".task-page-head")).toBe(head);
  });
});

// #153 review round 1: the three ways the box or the page still moved.
describe("the comment box across what arrives while typing", () => {
  const settle = async () => {
    await new Promise((done) => setTimeout(done, 250));
    await flush();
  };
  const typeText = (field, text) => {
    for (const key of text) {
      const at = field.selectionStart;
      field.value = field.value.slice(0, at) + key + field.value.slice(field.selectionEnd);
      field.setSelectionRange(at + 1, at + 1);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    }
  };

  it("keeps the paperclip and textarea in place across a late greeting", async () => {
    carriesAttachments = false;
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    field.focus();
    typeText(field, "typed before the greeting");
    const clip = host.querySelector(".task-composer .composer-attach");
    const tray = host.querySelector(".task-composer .composer-tray");
    expect(clip).not.toBeNull();

    carriesAttachments = true;
    page.feedMoved();
    expect(host.querySelector("#task-comment")).toBe(field);
    expect(field.isConnected).toBe(true);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("typed before the greeting");
    expect(field.closest(".composer.attachable.task-comment-box")).not.toBeNull();
    expect(host.querySelector(".task-composer .composer-attach")).not.toBeNull();
    expect(host.querySelector(".task-composer .composer-attach")).toBe(clip);
    expect(host.querySelector(".task-composer .composer-tray")).toBe(tray);

    // The controls hung on late are wired: a dropped file goes up and rides
    // the comment.
    call.mockImplementation(async (method) => {
      if (method === "tasks.attach") return { name: "shot.png", path: "/store/shot.png", mime: "image/png", size: 3 };
      return method === "tasks.get" ? answerFor() : {};
    });
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", { value: { files: [new File(["png"], "shot.png", { type: "image/png" })] } });
    host.querySelector("[data-task-composer]").dispatchEvent(drop);
    await vi.waitFor(() => expect(host.querySelector(".task-composer .composer-chip.ready")).not.toBeNull());

    // A new greeting cannot remove controls or draft state from this page.
    carriesAttachments = false;
    page.feedMoved();
    expect(host.querySelector("#task-comment")).toBe(field);
    expect(host.querySelector(".task-composer .composer-attach")).toBe(clip);
    expect(host.querySelector(".task-composer .composer-tray")).toBe(tray);
    expect(field.closest(".composer")).not.toBeNull();
    carriesAttachments = true;
    page.feedMoved();
    expect(host.querySelector("#task-comment")).toBe(field);
    expect(host.querySelector(".task-composer .composer-tray")).toBe(tray);
    expect(host.querySelectorAll(".task-composer .composer-attach")).toHaveLength(1);

    call.mockClear();
    host.querySelector("[data-task-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await vi.waitFor(() => expect(listed("tasks.comment")).toHaveLength(1));
    expect(listed("tasks.comment")[0][1]).toEqual({
      task_id: "task-1",
      body: "typed before the greeting",
      attachments: [{ name: "shot.png", path: "/store/shot.png", mime: "image/png", size: 3 }],
    });
  });

  /** A paste, and what the browser does with it when nobody cancels it: the
   *  text goes in at the caret. jsdom does no default actions, so the test
   *  does this one. Answers whether the paste was cancelled. */
  const paste = (field, text) => {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { files: [], items: [], getData: (type) => (type === "text/plain" ? text : "") } });
    field.dispatchEvent(event);
    if (event.defaultPrevented) return true;
    typeText(field, text);
    return false;
  };
  const dropFile = (form) => {
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { files: [new File(["png"], "shot.png", { type: "image/png" })] } });
    form.dispatchEvent(event);
    return event.defaultPrevented;
  };
  const attachAnswers = () => call.mockImplementation(async (method) => {
    if (method === "tasks.attach") return { name: "shot.png", path: "/store/shot.png", mime: "image/png", size: 3 };
    return method === "tasks.get" ? answerFor() : {};
  });

  it("takes pastes and drops as files before any capability greeting", async () => {
    carriesAttachments = false;
    await mount();
    await settle();
    attachAnswers();
    const field = host.querySelector("#task-comment");
    const form = host.querySelector("[data-task-composer]");
    field.focus();
    expect(host.querySelector(".task-composer .composer-attach")).not.toBeNull();

    expect(paste(field, "y".repeat(1300)), "the long paste went in as text").toBe(true);
    expect(dropFile(form)).toBe(true);
    await vi.waitFor(() => expect(host.querySelectorAll(".task-composer .composer-chip.ready")).toHaveLength(2));
    expect(listed("tasks.attach").map(([, params]) => params.filename)).toEqual(["pasted-text-1.txt", "shot.png"]);
    expect(field.value).toBe("");
    expect(host.querySelector("#task-comment")).toBe(field);
  });

  it("keeps the selection and the field's scroll when another tab writes a different draft", async () => {
    const { writeUiRecord } = await import("../src/core/localUiStore.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const address = uiAddress({ deviceId: "dev-1", entityId: "task-1", view: "tracker-task", kind: "draft", sub: "proj-1" });
    await mount();
    await settle();
    const field = host.querySelector("#task-comment");
    let fieldScroll = 0;
    Object.defineProperty(field, "scrollTop", { configurable: true, get: () => fieldScroll, set: (value) => { fieldScroll = value; } });
    field.focus();
    typeText(field, "abcdef");
    await settle(); // persisted, and its own echo painted
    field.setSelectionRange(2, 4);
    fieldScroll = 40;

    await writeUiRecord(address, { body: "abcdefgh" }, { source: "another-tab", sequence: 1 });
    await settle();
    expect(host.querySelector("#task-comment")).toBe(field);
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("abcdefgh");
    expect([field.selectionStart, field.selectionEnd]).toEqual([2, 4]);
    expect(fieldScroll).toBe(40);

    // A shorter draft clamps the selection to what it holds.
    field.setSelectionRange(5, 7);
    await writeUiRecord(address, { body: "abc" }, { source: "another-tab", sequence: 2 });
    await settle();
    expect(field.value).toBe("abc");
    expect([field.selectionStart, field.selectionEnd]).toEqual([3, 3]);
  });

  it("takes the New messages pill away when the unread row goes away through the cache", async () => {
    carriesWatching = true;
    const users = Array.from({ length: 20 }, (_, at) =>
      comment({ id: `tc-01M37FGQD48628P29BG1A4B${String(at).padStart(3, "0")}`, author: { kind: "user" } }));
    const agent = comment({ id: "tc-01M37FGQD48628P29BG1A4BZZZ", author: { kind: "agent", agent_id: "agent-1" } });
    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", read_through: users.at(-1).id }),
      timeline: [...users, agent],
    });
    call = vi.fn((method) => method === "tasks.get" ? new Promise(() => {}) : Promise.resolve({}));
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).not.toBeNull());
    expect(host.querySelector(".new-messages-pill")).not.toBeNull();
    const rows = [...host.querySelectorAll(".task-entry")];

    await trackerCache.writeTaskRecord("dev-1", "proj-1", "task-1", {
      task: task({ id: "task-1", read_through: users.at(-1).id }),
      timeline: users,
    });
    await vi.waitFor(() => expect(host.querySelector(".task-unread-line")).toBeNull());
    expect(host.querySelector(".new-messages-pill")).toBeNull();
    expect(host.querySelectorAll(".new-messages-dock")).toHaveLength(0);
    expect([...host.querySelectorAll(".task-entry")]).toEqual(rows.slice(0, 20));
  });
});
