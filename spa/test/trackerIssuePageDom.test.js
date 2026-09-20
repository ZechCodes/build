/** @vitest-environment jsdom */
// One issue's page: the issue, the timeline of comments and events interleaved,
// the composer, and the rail of everything about it that can be changed.
//
// Every rail control writes ONE field through the smallest verb that says what
// it means. The page never sends a whole record.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, comment, event, issue } from "./trackerWireFixture.js";

let watchers = [];
// The real module refuses a registration that names a cadence — nothing in
// this client polls — so this stand-in refuses one too. A poll creeping back
// into a surface fails here rather than only in a browser.
const RETIRED = ["intervalMs", "keepPolling", "catchUpOnVisible"];
vi.mock("../src/core/changeEvents.js", () => ({
  // The greeting says which kinds a bridge carries; a stand-in that
  // answers none would have the sync layer ask for none of the new ones.
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: carriedKinds } }),
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

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

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

const mount = async (over = {}) => {
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
  await flush();
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
  notifyError.mockClear();
  openAssigneePicker.mockClear();
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
});

describe("the timeline", () => {
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

  it("will not send an empty one", async () => {
    await mount();
    expect(host.querySelector(".issue-composer button").disabled).toBe(true);
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
