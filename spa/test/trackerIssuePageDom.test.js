/** @vitest-environment jsdom */
// One issue's page: the issue, the timeline of comments and events interleaved,
// the composer, and the rail of everything about it that can be changed.
//
// Every rail control writes ONE field through the smallest verb that says what
// it means. The page never sends a whole record.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, issue } from "./trackerWireFixture.js";
import issuesGetFixture from "../../fixtures/api/v1/issues.get.json";

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
    // #57: whether this bridge can carry files on an issue.
    issues: { attachments: carriesAttachments, watching: carriesWatching },
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
const EVERY_KIND = ["state", "thread", "git", "files", "terminals", "issues"];
let carriedKinds = EVERY_KIND;
/** Whether the bridge under test has `issues.attach` (1.8). */
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
  event({ id: "ie-1", kind: "created", at: "2026-08-21T10:00:00Z" }),
  comment({ id: "ic-1", created_at: "2026-08-21T10:01:00Z", body: "This reproduces on a phone too." }),
  event({
    id: "ie-2", kind: "moved", at: "2026-08-21T10:02:00Z",
    actor: { kind: "agent", agent_id: "agent-1" }, payload: { from: "backlog", to: "in_progress" },
  }),
  comment({ id: "ic-2", created_at: "2026-08-21T10:03:00Z", author: { kind: "agent", agent_id: "agent-1" }, body: "On it." }),
];

let host, call, page, trackerCache, mountIssuePage;
/** A write this bridge refuses, set by the case that wants one. The caller is
 *  captured at mount, so a case cannot hand over a new `call` afterwards. */
let refuses = null;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const answerFor = (over = {}, timeline = TIMELINE) => ({
  issue: issue({ id: "issue-1", number: 12, status: "in_progress", ...over }),
  timeline,
});

const mount = async (over = {}, { waitForPaint = true } = {}) => {
  page = mountIssuePage(host, {
    projectId: "proj-1",
    deviceId: "dev-1",
    projectKey: PROJECT_KEY,
    issueId: "issue-1",
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
  [...host.querySelectorAll(".issue-entry")].map((row) => {
    const who = row.querySelector(".issue-entry-head strong")?.textContent || "";
    const said = row.querySelector(".issue-comment-body")?.textContent;
    return said === undefined
      ? row.querySelector(".issue-event-text").textContent.replace(/\s+/g, " ").trim()
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
  ({ mountIssuePage } = await import("../src/core/trackerIssuePage.js"));
  await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
  refuses = null;
  call = vi.fn(async (method) => {
    if (refuses?.method === method) throw new Error(refuses.message);
    return method === "issues.get" ? answerFor() : {};
  });
});

afterEach(() => {
  page?.dispose();
});

describe("mounted issue identity links", () => {
  it("links an unwatched prose author, then removes dead routes when the workspace cache changes", async () => {
    const agentId = "agent-01K5ZQ8M4T0J7WQ2R6X3YB9C4E";
    const workspaceId = "ws-3f2a91c4";
    const agent = { kind: "agent", agent_id: agentId };
    feed.items = [];
    feed.workspaces = [{ id: workspaceId, workspace_id: workspaceId,
      name: "spa-flaky-tests", projectKey: PROJECT_KEY, entity_id: "run-unwatched" }];
    const wire = structuredClone(issuesGetFixture.result.issue);
    wire.id = "issue-1";
    wire.body = `Ask @agent:${agentId} on this issue.`;
    wire.assignee = agent;
    wire.links.workspace_ids = [workspaceId];
    call = vi.fn(async (method) => method === "issues.get" ? {
      issue: wire,
      timeline: [{ type: "comment", id: "ic-unwatched", author: agent,
        body: "I can take this.", created_at: "2026-09-19T10:12:00Z" }],
    } : {});
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-comment .issue-entry-head a[href*='agent=']")).not.toBeNull());
    expect(host.querySelector(".issue-page-body a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".issue-assignee-current a[href*='agent=']")).not.toBeNull();
    expect(host.querySelector(".issue-links a[href*='workspace']")).not.toBeNull();

    feed.workspaces = [];
    page.feedMoved();
    expect(host.querySelector(".issue-comment .issue-entry-head a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".issue-page-body a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".issue-assignee-current a[href*='agent=']")).toBeNull();
    expect(host.querySelector(".issue-links a[href*='workspace']")).toBeNull();
    expect(host.querySelector(".issue-comment .issue-entry-head").textContent).toContain("spa-flaky-tests · Fix drag");
  });
});

describe("issue unread navigation", () => {
  it("does not claim unread history on a bridge without issue read marks", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-page-title")).not.toBeNull());
    expect(host.querySelector(".issue-unread-line")).toBeNull();
    expect(host.querySelector(".new-messages-pill")).toBeNull();
  });

  it("paints a cached pre-visit mark, then jumps from the floating pill to the first unread entry", async () => {
    carriesWatching = true;
    const timeline = [
      event({ id: "ie-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "ic-01M37FGQD48628P29BG1A4BB02", author: { kind: "user" } }),
      event({ id: "ie-01M37FGQD48628P29BG1A4BB03", actor: { kind: "agent", agent_id: "agent-1" } }),
      comment({ id: "ic-01M37FGQD48628P29BG1A4BB04", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", {
      issue: issue({ id: "issue-1", read_through: timeline[0].id }),
      timeline,
    });
    // Keep the live read pending so this assertion can only pass from the
    // actual cached issue-page wiring, before this visit's mark returns.
    call = vi.fn((method) => method === "issues.get" ? new Promise(() => {}) : Promise.resolve({}));
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-unread-line")).not.toBeNull());
    const line = host.querySelector(".issue-unread-line");
    expect(line.nextElementSibling.classList.contains("issue-event")).toBe(true);
    expect(line.previousElementSibling.id).toBe(`comment-${timeline[1].id}`);
    expect(listed("issues.read_through")[0][1].event_id).toBe(timeline[3].id);

    const hostRect = vi.spyOn(host, "getBoundingClientRect").mockReturnValue({ bottom: 600 });
    const lineRect = vi.spyOn(line, "getBoundingClientRect").mockReturnValue({ top: 900 });
    const scrollIntoView = vi.fn();
    line.scrollIntoView = scrollIntoView;
    host.dispatchEvent(new Event("scroll"));
    const pill = host.querySelector(".new-messages-pill");
    expect(pill.getAttribute("aria-label")).toBe("Jump to first unread activity");
    expect(pill.closest(".new-messages-dock").hidden).toBe(false);
    pill.click();
    expect(scrollIntoView).toHaveBeenCalledWith({ behavior: "smooth", block: "start" });
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
      event({ id: "ie-01M37FGQD48628P29BG1A4BB01", actor: { kind: "user" } }),
      comment({ id: "ic-01M37FGQD48628P29BG1A4BB02", author: { kind: "agent", agent_id: "agent-1" } }),
      comment({ id: "ic-01M37FGQD48628P29BG1A4BB03", author: { kind: "agent", agent_id: "agent-1" } }),
    ];
    const stale = answerFor({ read_through: timeline[0].id }, timeline);
    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", stale);
    // Leave issues.get outstanding across both visits: only the cache can
    // paint, and the accepted mark must outlive the first page instance.
    call = vi.fn((method) => method === "issues.get" ? new Promise(() => {}) : Promise.resolve({ issue: {
      read_through: timeline[2].id,
    } }));
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-unread-line")).not.toBeNull());
    await vi.waitFor(async () => expect((await trackerCache.readIssueRecord("dev-1", "proj-1", "issue-1"))?.issue.read_through)
      .toBe(timeline[2].id));

    page.dispose();
    const seen = [];
    await mount({ onIssueRead: (one) => seen.push(one.read_through) }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-page-title")).not.toBeNull());
    expect(host.querySelector(".issue-unread-line")).toBeNull();
    expect(host.querySelector(".new-messages-pill")).toBeNull();

    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", stale);
    expect((await trackerCache.readIssueRecord("dev-1", "proj-1", "issue-1")).issue.read_through).toBe(timeline[2].id);
    await vi.waitFor(() => expect(seen.length).toBeGreaterThan(1));
    expect(host.querySelector(".issue-unread-line")).toBeNull();
  });

  it("keeps read floors monotonic and scoped to one device, project and issue", async () => {
    const first = "ie-01M37FGQD48628P29BG1A4BB01";
    const later = "ic-01M37FGQD48628P29BG1A4BB03";
    for (const [device, project, id] of [
      ["dev-1", "proj-1", "issue-1"], ["dev-2", "proj-1", "issue-1"],
      ["dev-1", "proj-2", "issue-1"], ["dev-1", "proj-1", "issue-2"],
    ]) await trackerCache.writeIssueRecord(device, project, id, answerFor({ id, read_through: first }));
    await trackerCache.advanceIssueReadThrough("dev-1", "proj-1", "issue-1", later);
    await trackerCache.advanceIssueReadThrough("dev-1", "proj-1", "issue-1", first);
    await trackerCache.advanceIssueReadThrough("dev-1", "proj-1", "issue-1", "comment-3");
    expect((await trackerCache.readIssueRecord("dev-1", "proj-1", "issue-1")).issue.read_through).toBe(later);
    for (const [device, project, id] of [
      ["dev-2", "proj-1", "issue-1"], ["dev-1", "proj-2", "issue-1"], ["dev-1", "proj-1", "issue-2"],
    ]) expect((await trackerCache.readIssueRecord(device, project, id)).issue.read_through).toBe(first);
  });
});

describe("the issue", () => {
  it("reads it by id and draws its title and number", async () => {
    await mount();
    expect(listed("issues.get")[0][1]).toEqual({ issue_id: "issue-1" });
    expect(host.querySelector(".issue-page-title").textContent).toBe("Kanban drag does not persist");
    expect(host.querySelector(".issue-number").textContent).toBe("#12");
  });

  it("renders the body as markdown", async () => {
    call = vi.fn(async () => answerFor({ body: "Dragging a card **does not** persist." }));
    await mount();
    expect(host.querySelector(".issue-page-body strong").textContent).toBe("does not");
  });

  it("says so rather than leaving a gap when there is no description", async () => {
    call = vi.fn(async () => answerFor({ body: "" }));
    await mount();
    expect(host.querySelector(".issue-page-body").textContent).toBe("No description.");
  });

  // Open/closed is independent of the Done column: both are shown.
  it("shows the state and the column as two separate facts", async () => {
    call = vi.fn(async () => answerFor({ state: "closed", status: "in_progress" }));
    await mount();
    expect(host.querySelector(".issue-page-state").textContent).toBe("Closed");
    expect(host.querySelector("#issue-status").value).toBe("in_progress");
  });

  it("paints from the cache before the bridge answers", async () => {
    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", {
      issue: issue({ id: "issue-1", number: 12, title: "From the cache" }),
      timeline: [],
    });
    let settle;
    call = vi.fn((method) => (method === "issues.get" ? new Promise((resolve) => { settle = resolve; }) : Promise.resolve({})));
    await mount();
    expect(host.querySelector(".issue-page-title").textContent).toBe("From the cache");
    settle(answerFor());
    await flush();
    expect(host.querySelector(".issue-page-title").textContent).toBe("Kanban drag does not persist");
  });

  it("writes what it read back through the cache, so the revisit is warm", async () => {
    await mount();
    const held = await trackerCache.readIssueRecord("dev-1", "proj-1", "issue-1");
    expect(held.issue.number).toBe(12);
    expect(held.timeline).toHaveLength(4);
  });

  it("restores an unsent comment and clears it only after a successful send", async () => {
    const { readCached } = await import("../src/core/localCache.js");
    const { uiAddress } = await import("../src/core/localUiState.js");
    const address = uiAddress({ deviceId: "dev-1", entityId: "issue-1", view: "tracker-issue", kind: "draft", sub: "proj-1" });
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    field.value = "Keep this thought";
    field.dispatchEvent(new Event("input", { bubbles: true }));
    await vi.waitFor(async () => expect((await readCached(address))?.value.body).toBe("Keep this thought"));
    page.dispose();
    await mount({ issueId: "issue-2" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    expect(host.querySelector("#issue-comment").value).toBe("");
    page.dispose();
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")?.value).toBe("Keep this thought"));
    host.querySelector("[data-issue-composer]").dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await vi.waitFor(() => expect(listed("issues.comment")).toHaveLength(1));
    await vi.waitFor(async () => expect((await readCached(address))?.value.body).toBe(""));
  });

  it("repaints from an issue-cache write while issues.get stays absent", async () => {
    call = vi.fn(() => new Promise(() => {}));
    await mount();
    expect(host.querySelector(".issue-page-title")).toBeNull();

    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", {
      issue: issue({ id: "issue-1", number: 12, title: "Announced detail" }),
      timeline: [],
    });
    await flush();

    expect(host.querySelector(".issue-page-title").textContent).toBe("Announced detail");
  });
});

describe("the timeline", () => {
  it("renders a mentioned comment from the bridge's issues.get fixture", async () => {
    call = vi.fn(async () => issuesGetFixture.result);
    await mount({ issueId: issuesGetFixture.params.issue_id }, { waitForPaint: false });

    const mentioned = issuesGetFixture.result.timeline.find((entry) => entry.type === "comment" && entry.mentions_user);
    await vi.waitFor(() => expect(host.querySelector(`#comment-${mentioned.id}`)).not.toBeNull());
    const row = host.querySelector(`#comment-${mentioned.id}`);
    expect(row?.classList.contains("issue-comment-mentioned")).toBe(true);
    expect(row.querySelector(".issue-comment-body").textContent).toBe(mentioned.body);
  });

  it("keeps a mentioned comment marked through cached, live, and read-state paints", async () => {
    const mentioned = comment({ id: "ic-mention", author: { kind: "agent", agent_id: "agent-1" }, mentions_user: true });
    const plain = comment({ id: "ic-plain", author: { kind: "agent", agent_id: "agent-1" } });
    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", answerFor({}, [mentioned, plain]));
    let settle;
    call = vi.fn((method) => method === "issues.get"
      ? new Promise((resolve) => { settle = resolve; })
      : Promise.resolve({}));
    await mount({}, { waitForPaint: false });

    const marked = () => host.querySelector("#comment-ic-mention");
    const unmarked = () => host.querySelector("#comment-ic-plain");
    await vi.waitFor(() => {
      expect(marked()).not.toBeNull();
      expect(unmarked()).not.toBeNull();
      expect(listed("issues.get")).toHaveLength(1);
    });
    expect(settle).toBeTypeOf("function");
    expect(marked().classList.contains("issue-comment-mentioned")).toBe(true);
    expect(unmarked().classList.contains("issue-comment-mentioned")).toBe(false);

    settle(answerFor({ read_through: "ic-mention" }, [{ ...mentioned, body: "New live wording" }, plain]));
    await vi.waitFor(() => expect(marked().querySelector(".issue-comment-body").textContent).toBe("New live wording"));
    expect(marked().classList.contains("issue-comment-mentioned")).toBe(true);

    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", answerFor({}, [
      { ...mentioned, mentions_user: false }, { ...plain, mentions_user: true },
    ]));
    await vi.waitFor(() => expect(unmarked().classList.contains("issue-comment-mentioned")).toBe(true));
    expect(marked().classList.contains("issue-comment-mentioned")).toBe(false);
  });

  it("scrolls to and highlights a routed comment after the cache paints", async () => {
    const scrollIntoView = vi.fn();
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    await trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", answerFor());
    call = vi.fn(() => new Promise(() => {}));
    await mount({ commentId: "ic-2" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#comment-ic-2")?.classList.contains("issue-comment-target")).toBe(true));
    expect(scrollIntoView).toHaveBeenCalledWith({ block: "center" });
  });

  it("opens a missing comment at the top without an error", async () => {
    await mount({ commentId: "ic-missing" }, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector(".issue-page-title")?.textContent).toBe("Kanban drag does not persist"));
    expect(host.querySelector(".issue-comment-target")).toBeNull();
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
    expect(host.querySelectorAll(".issue-comment strong")[1].textContent).toBe("wire-facade · Agent 1");
  });

  it("says nothing has happened yet when the timeline is empty", async () => {
    call = vi.fn(async () => answerFor({}, []));
    await mount();
    expect(host.querySelector(".issue-empty").textContent).toContain("Nothing has happened");
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

  it("sends a comment and re-reads the issue", async () => {
    await mount();
    const field = host.querySelector("#issue-comment");
    field.value = "  Looking at the drag handler.  ";
    field.dispatchEvent(new Event("input"));
    call.mockClear();
    host.querySelector("[data-issue-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(listed("issues.comment")[0][1]).toEqual({ issue_id: "issue-1", body: "Looking at the drag handler." });
    expect(listed("issues.get")).toHaveLength(1);
  });

  it("sends Ctrl+Enter from the mounted comment box through issues.comment", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    typeComment(field, "From the keyboard");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true }).defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(listed("issues.comment")).toHaveLength(1));
    expect(listed("issues.comment")[0][1]).toEqual({ issue_id: "issue-1", body: "From the keyboard" });
  });

  it("sends Cmd+Enter while leaving plain Enter available for a newline", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    typeComment(field, "Mac comment");
    call.mockClear();

    expect(pressEnter(field).defaultPrevented).toBe(false);
    expect(field.value).toBe("Mac comment");
    expect(listed("issues.comment")).toHaveLength(0);
    expect(pressEnter(field, { metaKey: true }).defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(listed("issues.comment")).toHaveLength(1));
    expect(listed("issues.comment")[0][1].body).toBe("Mac comment");
  });

  it("does not send an empty comment on Ctrl+Enter", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    typeComment(field, "   ");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(listed("issues.comment")).toHaveLength(0);
  });

  it("ignores Ctrl+Enter during IME composition", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    typeComment(field, "Still composing");
    call.mockClear();

    expect(pressEnter(field, { ctrlKey: true, isComposing: true }).defaultPrevented).toBe(false);
    expect(listed("issues.comment")).toHaveLength(0);
  });

  it("does not post twice when Ctrl+Enter is pressed again while sending", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => expect(host.querySelector("#issue-comment")).not.toBeNull());
    const field = host.querySelector("#issue-comment");
    typeComment(field, "Only once");
    let finishSend;
    call.mockImplementation((method) => method === "issues.comment"
      ? new Promise((resolve) => { finishSend = resolve; })
      : Promise.resolve(method === "issues.get" ? answerFor() : {}));
    call.mockClear();

    pressEnter(field, { ctrlKey: true });
    pressEnter(field, { ctrlKey: true });
    await vi.waitFor(() => expect(listed("issues.comment")).toHaveLength(1));
    const sendingField = host.querySelector("#issue-comment");
    expect(sendingField.disabled).toBe(true);
    pressEnter(sendingField, { ctrlKey: true });
    expect(listed("issues.comment")).toHaveLength(1);
    finishSend({});
    await vi.waitFor(() => expect(host.querySelector("#issue-comment").disabled).toBe(false));
    expect(listed("issues.comment")).toHaveLength(1);
  });

  it("will not send an empty one", async () => {
    await mount();
    expect(host.querySelector('.issue-composer button[type="submit"]').disabled).toBe(true);
  });

  // #57: the comment box takes files too, and only against a bridge that can
  // carry them. A press that cannot work is worse than no press.
  it("offers the paperclip on a bridge that carries files, and none on one that does not", async () => {
    await mount();
    expect(host.querySelector(".issue-composer .composer-attach")).not.toBeNull();
    expect(host.querySelector(".issue-composer .composer.attachable")).not.toBeNull();

    page?.dispose?.();
    carriesAttachments = false;
    document.body.innerHTML = '<div id="page"></div>';
    host = document.querySelector("#page");
    await mount();
    expect(host.querySelector(".issue-composer .composer-attach")).toBeNull();
    expect(host.querySelector(".issue-composer .composer-tray")).toBeNull();
    expect(host.querySelector(".issue-composer .composer-dropmask")).toBeNull();
    // Still a comment box, still sends.
    expect(host.querySelector("#issue-comment")).not.toBeNull();
  });

  it("sends a dropped file with the comment, through issues.attach", async () => {
    await mount({}, { waitForPaint: false });
    await vi.waitFor(() => {
      expect(host.querySelector("[data-issue-composer]")).not.toBeNull();
      expect(host.querySelector("#issue-comment")?.disabled).toBe(false);
      expect(host.querySelector(".issue-composer .composer.attachable")).not.toBeNull();
    });
    call.mockImplementation(async (method) => {
      if (method === "issues.attach") {
        return { name: "shot.png", path: "/store/abc-shot.png", mime: "image/png", size: 3 };
      }
      return method === "issues.get" ? answerFor() : {};
    });
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", {
      value: { files: [new File(["png"], "shot.png", { type: "image/png" })] },
    });
    host.querySelector("[data-issue-composer]").dispatchEvent(event);
    await vi.waitFor(() => {
      expect(host.querySelector(".issue-composer .composer-chip.ready .composer-chip-name")?.textContent).toBe("shot.png");
      expect(host.querySelector('.issue-composer button[type="submit"]')?.disabled).toBe(false);
    });
    expect(listed("issues.attach")[0][1]).toMatchObject({ project_id: "proj-1", filename: "shot.png" });

    // A comment that is only a screenshot is a comment: the press turns on
    // with an empty box.
    call.mockClear();
    pressEnter(host.querySelector("#issue-comment"), { ctrlKey: true });
    await vi.waitFor(() => expect(listed("issues.comment")).toHaveLength(1));
    expect(listed("issues.comment")[0][1]).toEqual({
      issue_id: "issue-1",
      body: "",
      attachments: [{ name: "shot.png", path: "/store/abc-shot.png", mime: "image/png", size: 3 }],
    });
  });

  it("says why a refused comment did not land", async () => {
    await mount();
    const field = host.querySelector("#issue-comment");
    field.value = "hello";
    field.dispatchEvent(new Event("input"));
    refuses = { method: "issues.comment", message: "issue is closed" };
    host.querySelector("[data-issue-composer]").dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(notifyError).toHaveBeenCalledWith("Could not add this comment", "issue is closed");
  });
});

describe("the rail", () => {
  const railWrite = async (act) => {
    call.mockClear();
    await act();
    await flush();
  };

  it("closes an open issue and reopens a closed one", async () => {
    await mount();
    expect(host.querySelector("[data-issue-state]").textContent).toBe("Close issue");
    await railWrite(() => host.querySelector("[data-issue-state]").click());
    expect(listed("issues.close")[0][1]).toEqual({ issue_id: "issue-1" });
  });

  it("reopens rather than closing when the issue is already closed", async () => {
    call = vi.fn(async () => answerFor({ state: "closed" }));
    await mount();
    expect(host.querySelector("[data-issue-state]").textContent).toBe("Reopen issue");
    await railWrite(() => host.querySelector("[data-issue-state]").click());
    expect(listed("issues.reopen")[0][1]).toEqual({ issue_id: "issue-1" });
  });

  // Closing does not move it to Done and Done does not close it; the rail says
  // so rather than leaving the reader to find out.
  it("says that closing does not move it to Done", async () => {
    await mount();
    expect(host.querySelector(".issue-rail").textContent).toContain("Closing does not move it to Done.");
  });

  it("moves the issue through the column select, sending only that field", async () => {
    await mount();
    await railWrite(() => {
      const select = host.querySelector("#issue-status");
      select.value = "in_review";
      select.dispatchEvent(new Event("change"));
    });
    expect(listed("issues.update")[0][1]).toEqual({ issue_id: "issue-1", status: "in_review" });
  });

  it("sets the priority, sending only that field", async () => {
    await mount();
    await railWrite(() => {
      const select = host.querySelector("#issue-priority");
      select.value = "high";
      select.dispatchEvent(new Event("change"));
    });
    expect(listed("issues.update")[0][1]).toEqual({ issue_id: "issue-1", priority: "high" });
  });

  it("saves the labels on Enter, as the list the verb stores", async () => {
    await mount();
    const field = host.querySelector("#issue-labels");
    field.value = " bug , ui , bug ";
    await railWrite(() => field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", cancelable: true })));
    expect(listed("issues.update")[0][1]).toEqual({ issue_id: "issue-1", labels: ["bug", "ui"] });
  });

  it("opens the assignee picker, standing on who holds it", async () => {
    call = vi.fn(async () => answerFor({ assignee: { kind: "agent", agent_id: "agent-1" } }));
    await mount();
    host.querySelector("[data-issue-assign]").click();
    await flush();
    expect(openAssigneePicker.mock.calls[0][0].current).toBe("agent:agent-1");
  });

  it("says that assigning starts an agent, before it is pressed", async () => {
    await mount();
    expect(host.querySelector(".issue-rail").textContent).toContain("Assigning hands the issue to an agent and starts it.");
  });
});

describe("the links", () => {
  const linked = async (links) => {
    call = vi.fn(async () => answerFor({ links: { workspace_ids: [], branches: [], commits: [], conversation_ids: [], parent_issue_id: null, ...links } }));
    await mount();
    return [...host.querySelectorAll(".issue-link")];
  };

  it("opens the workspace it is being worked in, by that workspace's name", async () => {
    const rows = await linked({ workspace_ids: ["ws-1"] });
    expect([rows[0].textContent, rows[0].querySelector("a").getAttribute("href")]).toEqual([
      "wire-facade", "#/device/dev-1/project/proj-1/workspace/ws-1/changes",
    ]);
  });

  it("opens the branch through the branch surface", async () => {
    const rows = await linked({ branches: ["build/issues-spa"] });
    expect(rows[0].querySelector("a").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/branch/build%2Fissues-spa/changes");
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
    expect(host.querySelector(".issue-rail").textContent).toContain("Nothing linked yet");
  });
});

describe("the push", () => {
  it("subscribes the project for issues", async () => {
    await mount();
    expect(watchers.map((one) => [one.entity, one.kinds])).toEqual([["proj-1", ["issues"]]]);
  });

  it("asks a bridge that does not carry issues for no kind at all", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await mount();
    expect(watchers.map((one) => one.kinds)).toEqual([[]]);
  });

  it("re-reads this issue when an item names it", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", issues: { issue_ids: ["issue-1"], truncated: false } }]);
    await flush();
    expect(listed("issues.get")).toHaveLength(1);
  });

  // The reader is typing a comment when the issue moves under them. The page
  // redraws to show the move, and the caret is exactly where it was.
  it("keeps the caret in the comment box across a re-read", async () => {
    await mount();
    const field = host.querySelector("#issue-comment");
    field.value = "Reproduced on the phone";
    field.dispatchEvent(new Event("input"));
    const typed = host.querySelector("#issue-comment");
    typed.focus();
    typed.setSelectionRange(10, 10);
    call.mockImplementation(async (method) => (method === "issues.get" ? answerFor({ status: "in_review" }) : {}));
    watchers[0].onChanges([{ entity_id: "proj-1", issues: { issue_ids: ["issue-1"], truncated: false } }]);
    await flush();

    const after = host.querySelector("#issue-comment");
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
    const before = host.querySelector("#issue-comment");
    before.focus();
    page.feedMoved();
    page.feedMoved();
    expect(host.querySelector("#issue-comment")).toBe(before);
    expect(document.activeElement).toBe(before);
  });

  it("leaves an item about another issue alone", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", issues: { issue_ids: ["issue-9"], truncated: false } }]);
    await flush();
    expect(listed("issues.get")).toEqual([]);
  });

  // Truncation means "refetch", so a truncated item is news about everything.
  it("re-reads when the ids were dropped as truncated", async () => {
    await mount();
    call.mockClear();
    watchers[0].onChanges([{ entity_id: "proj-1", issues: { issue_ids: [], truncated: true } }]);
    await flush();
    expect(listed("issues.get")).toHaveLength(1);
  });
});

// The project agent's rail stands beside this page, so the page says which
// issue is open. Only from the READ: the route names an id, and an agent told
// an id and nothing else is no better off.
describe("saying which issue is open", () => {
  it("reports the issue once it has been read, and not before", async () => {
    const seen = [];
    let settle;
    call = vi.fn((method) => (method === "issues.get" ? new Promise((resolve) => { settle = resolve; }) : Promise.resolve({})));
    page = mountIssuePage(host, {
      projectId: "proj-1",
      deviceId: "dev-1",
      projectKey: PROJECT_KEY,
      issueId: "issue-1",
      callRpc: call,
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => feed,
      navigate: vi.fn(),
      onIssueRead: (issue) => seen.push(issue),
    });
    await flush();
    expect(seen).toEqual([]);
    settle(answerFor());
    await flush();
    expect(seen.map((one) => [one.number, one.title])).toEqual([[12, "Kanban drag does not persist"]]);
  });

  it("reports it again when a push re-reads it, so a retitled issue is not stale", async () => {
    const seen = [];
    await mount({ onIssueRead: (issue) => seen.push(issue.title) });
    expect(seen).toHaveLength(1);
    watchers[0].onChanges([{ entity_id: "proj-1", issues: { issue_ids: ["issue-1"], truncated: false } }]);
    await flush();
    expect(seen.length).toBeGreaterThan(1);
  });
});

// #24. A phone's session dies at the network layer every few minutes and every
// call in flight dies with it. The page had already painted from the cache, so
// "Could not read this issue" was a complaint about a copy that was on screen
// the whole time.
describe("a read that fails because the session dropped", () => {
  const WENT = "your device went offline";
  const cached = () =>
    trackerCache.writeIssueRecord("dev-1", "proj-1", "issue-1", {
      issue: issue({ id: "issue-1", number: 12, title: "From the cache" }),
      timeline: [],
    });
  const note = () => host.querySelector(".read-wait")?.textContent ?? null;

  it("keeps the cached copy and says nothing", async () => {
    await cached();
    refuses = { method: "issues.get", message: WENT };
    await mount();
    expect(host.querySelector(".issue-page-title").textContent).toBe("From the cache");
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("marks when that copy was read, while the machine is being reconnected to", async () => {
    await cached();
    refuses = { method: "issues.get", message: WENT };
    await mount();
    expect(note()).toContain("reconnecting");
  });

  it("reads again when the machine is back, with nothing polled in between", async () => {
    await cached();
    refuses = { method: "issues.get", message: WENT };
    await mount();
    expect(listed("issues.get")).toHaveLength(1);
    refuses = null;
    await reconnect();
    expect(listed("issues.get")).toHaveLength(2);
    expect(host.querySelector(".issue-page-title").textContent).toBe("Kanban drag does not persist");
    expect(note()).toBeNull();
    expect(notifyError).not.toHaveBeenCalled();
  });

  // The bridge answered. The reader has to hear that, dropped session or not.
  it("still says a refusal out loud", async () => {
    await cached();
    refuses = { method: "issues.get", message: "no such issue" };
    await mount();
    expect(notifyError).toHaveBeenCalledWith("Could not read this issue", "no such issue");
  });

  // Nothing on screen to be quiet about: the wait is the same, but its failure
  // is said.
  it("waits with nothing on screen, then says so when the retry fails too", async () => {
    refuses = { method: "issues.get", message: WENT };
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
    await reconnect();
    expect(notifyError).toHaveBeenCalledWith("Could not read this issue", WENT);
  });
});
