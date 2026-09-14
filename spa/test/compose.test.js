// The compose box's pure model: what the client holds when it cannot send, how
// a held capture reaches the daemon, when the `c` shortcut is the user asking
// for the box, and the row a capture reads as wherever its copy came from.

import { describe, it, expect } from "vitest";
import {
  CAPTURE_QUEUE_KEY,
  ROUTED_LINGER_MS,
  branchOptions,
  captureRow,
  composeManualAwayNote,
  composeOfflineNote,
  composePlaceholder,
  composePromptHtml,
  composeShortcutFires,
  flushCaptureQueue,
  loadCaptureQueue,
  manualRoute,
  manualRouteDestination,
  mergeCaptureRows,
  queuedCapture,
  queuedCaptureRow,
  routedCaptureExpired,
  saveCaptureQueue,
  withoutQueued,
} from "../src/core/compose.js";
import { memoryStorage, writeRefusingStorage } from "./memoryStorage.js";

const capture = (over = {}) => ({
  id: "capture-1",
  text: "fix the login redirect",
  created_at: "2026-08-14T10:00:00Z",
  state: "routing",
  routing: null,
  question: null,
  ...over,
});

describe("the capture queue", () => {
  it("keeps what was said when there is nothing to send it to", () => {
    const storage = memoryStorage();
    const queue = [queuedCapture("ship the thing", { id: "local-1", createdAt: "2026-08-14T10:00:00Z" })];
    saveCaptureQueue(queue, storage);
    expect(JSON.parse(storage.entries.get(CAPTURE_QUEUE_KEY))).toEqual([
      { id: "local-1", text: "ship the thing", createdAt: "2026-08-14T10:00:00Z" },
    ]);
    expect(loadCaptureQueue(storage)).toEqual(queue);
  });

  it("reads an unreadable or corrupt store as an empty queue", () => {
    expect(loadCaptureQueue(memoryStorage({ [CAPTURE_QUEUE_KEY]: "{not json" }))).toEqual([]);
    expect(loadCaptureQueue(memoryStorage({ [CAPTURE_QUEUE_KEY]: '{"text":"x"}' }))).toEqual([]);
    expect(loadCaptureQueue(memoryStorage())).toEqual([]);
  });

  it("keeps working when the device refuses to store the queue", () => {
    const readonly = writeRefusingStorage();
    expect(() => saveCaptureQueue([queuedCapture("x", { id: "l1", createdAt: "now" })], readonly)).not.toThrow();
  });

  it("drops only the entry it names, and leaves the queue it read alone", () => {
    const queue = [
      queuedCapture("first", { id: "l1", createdAt: "t1" }),
      queuedCapture("second", { id: "l2", createdAt: "t2" }),
    ];
    expect(withoutQueued(queue, "l1")).toEqual([queue[1]]);
    expect(queue.length).toBe(2);
  });

  it("sends what it holds oldest first and forgets each one the daemon took", async () => {
    const queue = [
      queuedCapture("first", { id: "l1", createdAt: "t1" }),
      queuedCapture("second", { id: "l2", createdAt: "t2" }),
    ];
    const seen = [];
    const { sent, remaining } = await flushCaptureQueue(queue, async (text) => {
      seen.push(text);
      return capture({ id: `capture-${seen.length}`, text });
    });
    expect(seen).toEqual(["first", "second"]);
    expect(sent.map(({ queued, capture: made }) => [queued.id, made.id])).toEqual([
      ["l1", "capture-1"],
      ["l2", "capture-2"],
    ]);
    expect(remaining).toEqual([]);
  });

  it("stops at the first capture it cannot send, and keeps that one and everything behind it", async () => {
    const queue = [
      queuedCapture("first", { id: "l1", createdAt: "t1" }),
      queuedCapture("second", { id: "l2", createdAt: "t2" }),
      queuedCapture("third", { id: "l3", createdAt: "t3" }),
    ];
    const { sent, remaining } = await flushCaptureQueue(queue, async (text) => {
      if (text === "second") throw new Error("device offline");
      return capture({ text });
    });
    expect(sent.map(({ queued }) => queued.id)).toEqual(["l1"]);
    expect(remaining.map((entry) => entry.id)).toEqual(["l2", "l3"]);
  });
});

describe("the compose shortcut", () => {
  const press = (over = {}) => ({ key: "c", metaKey: false, ctrlKey: false, altKey: false, target: {}, ...over });

  it("fires on a bare c", () => {
    expect(composeShortcutFires(press())).toBe(true);
  });

  it("never fires while a modifier is held — those are the browser's", () => {
    expect(composeShortcutFires(press({ metaKey: true }))).toBe(false);
    expect(composeShortcutFires(press({ ctrlKey: true }))).toBe(false);
    expect(composeShortcutFires(press({ altKey: true }))).toBe(false);
  });

  it("never fires on another key", () => {
    expect(composeShortcutFires(press({ key: "v" }))).toBe(false);
    expect(composeShortcutFires(press({ key: "C" }))).toBe(false);
  });

  it("never fires while the letter is being typed into something", () => {
    for (const tagName of ["INPUT", "TEXTAREA", "SELECT"]) {
      expect(composeShortcutFires(press({ target: { tagName } }))).toBe(false);
    }
    expect(composeShortcutFires(press({ target: { tagName: "DIV", isContentEditable: true } }))).toBe(false);
  });

  it("never fires into a terminal, where every letter belongs to the PTY", () => {
    const inTerminal = { tagName: "CANVAS", closest: (selector) => (selector.includes("term-screen") ? {} : null) };
    expect(composeShortcutFires(press({ target: inTerminal }))).toBe(false);
    const elsewhere = { tagName: "DIV", closest: () => null };
    expect(composeShortcutFires(press({ target: elsewhere }))).toBe(true);
  });
});

describe("a capture as an inbox row", () => {
  it("reads a daemon record in the same shape board.list gives the ones it carries", () => {
    const row = captureRow(
      capture({
        state: "routed",
        routing: { project_id: "p1", kind: "issue", target_id: "plan-7", routed_at: "t", rationale: "no branch names this" },
      }),
      { projectName: "relaydb" },
    );
    expect(row.kind).toBe("capture");
    expect(row.capture_id).toBe("capture-1");
    expect(row.project_id).toBe("p1");
    expect(row.project).toBe("relaydb");
    expect(row.issue_id).toBe("plan-7");
    expect(row.branch).toBeNull();
    expect(row.title).toBe("fix the login redirect");
    expect(row.state).toBe("routed");
    expect(row.working).toBe(false);
    expect(row.unread).toBe(false);
  });

  it("says a capture needs the user when the router asked something or gave up", () => {
    const asked = captureRow(capture({ state: "unrouted", question: { text: "which project?", asked_at: "t", answer: null } }));
    expect(asked.unread).toBe(true);
    expect(asked.unread_reason).toBe("router_question");
    expect(asked.question.text).toBe("which project?");

    const failed = captureRow(capture({ state: "failed" }));
    expect(failed.unread).toBe(true);
    expect(failed.unread_reason).toBe("routing_failed");
  });

  it("names a branch destination as a branch", () => {
    const row = captureRow(
      capture({ state: "routed", routing: { project_id: "p1", kind: "branch", target_id: "build/login", routed_at: "t" } }),
    );
    expect(row.branch).toBe("build/login");
    expect(row.issue_id).toBeNull();
  });

  it("reads a capture the client is still holding as a row of its own", () => {
    const row = queuedCaptureRow(queuedCapture("ship the thing\nand the other", { id: "local-1", createdAt: "2026-08-14T10:00:00Z" }));
    expect(row.kind).toBe("capture");
    expect(row.capture_id).toBe("local-1");
    expect(row.state).toBe("queued");
    expect(row.title).toBe("ship the thing");
    expect(row.working).toBe(true);
    expect(row.unread).toBe(false);
  });
});

describe("merging the client's captures into the feed", () => {
  const feedRow = (id) => ({ kind: "capture", capture_id: id, title: `feed ${id}`, state: "routing" });

  it("puts what the client holds beside what the feed carries", () => {
    const branch = { kind: "branch", run_id: "run-1" };
    const merged = mergeCaptureRows([branch, feedRow("capture-1")], [queuedCaptureRow(queuedCapture("held", { id: "l1", createdAt: "t" }))]);
    expect(merged.map((row) => row.capture_id || row.run_id)).toEqual(["run-1", "capture-1", "l1"]);
  });

  it("lets the daemon's copy win over the client's for the same capture", () => {
    const merged = mergeCaptureRows([feedRow("capture-1")], [captureRow(capture({ state: "unrouted" }))]);
    expect(merged.length).toBe(1);
    expect(merged[0].title).toBe("feed capture-1");
  });
});

describe("how long a routed capture stays visible", () => {
  const settled = (over = {}) => ({ state: "routed", settledAt: 1000, ...over });

  it("keeps a settled route on screen for a while, so the user can see and undo it", () => {
    expect(routedCaptureExpired(settled(), 1000 + ROUTED_LINGER_MS - 1)).toBe(false);
    expect(routedCaptureExpired(settled(), 1000 + ROUTED_LINGER_MS + 1)).toBe(true);
  });

  it("never expires a capture that is still unfinished business", () => {
    expect(routedCaptureExpired(settled({ state: "routing", settledAt: null }), 10 ** 9)).toBe(false);
    expect(routedCaptureExpired(settled({ state: "failed", settledAt: null }), 10 ** 9)).toBe(false);
  });
});

// ---- the advanced panel ------------------------------------------------------
// The manual flow, for when you already know the destination: it bypasses the
// router entirely and speaks the same two verbs the router's own tools do.

describe("the manual route", () => {
  it("files an inert issue: the record exists, and nothing runs until the first message", () => {
    expect(
      manualRoute({ kind: "issue", projectId: "p1", text: "add a /health endpoint", agentParams: { provider: "codex" } }),
    ).toEqual({
      method: "issue.create",
      params: { goal: "add a /health endpoint", project_id: "p1", dispatch: false, provider: "codex" },
    });
  });

  it("dispatches a branch in one call, naming the branch only when one was named", () => {
    expect(manualRoute({ kind: "branch", projectId: "p1", text: "finish the redirect", branch: "build/login" })).toEqual({
      method: "branch.dispatch",
      params: { project_id: "p1", instruction: "finish the redirect", branch: "build/login" },
    });
    expect(manualRoute({ kind: "branch", projectId: "p1", text: "finish the redirect", branch: "  " })).toEqual({
      method: "branch.dispatch",
      params: { project_id: "p1", instruction: "finish the redirect" },
    });
  });

  it("opens what it made", () => {
    expect(manualRouteDestination("issue", { project_id: "p1", issue_id: "iss-3" }, "p1")).toEqual({
      name: "issue",
      projectId: "p1",
      id: "iss-3",
    });
    expect(manualRouteDestination("branch", { project_id: "p1", branch: "build/login" }, "p1")).toEqual({
      name: "branch",
      projectId: "p1",
      branch: "build/login",
      tab: "changes",
    });
  });

  // The daemon cuts the branch with its state lock released, and the row is on
  // the board from the moment it is asked for. A reply that names nothing to
  // open leaves the capture where the board is already showing it.
  it("opens nothing when the reply names nothing", () => {
    expect(manualRouteDestination("branch", { project_id: "p1" }, "p1")).toBeNull();
    expect(manualRouteDestination("issue", { project_id: "p1" }, "p1")).toBeNull();
    expect(manualRouteDestination("branch", null, "p1")).toBeNull();
  });
});

describe("the branches a project already has", () => {
  it("lists them once each, in order, and only for the project asked about", () => {
    const items = [
      { kind: "branch", project_id: "p1", branch: "build/login" },
      { kind: "branch", project_id: "p1", branch: "main" },
      { kind: "branch", project_id: "p1", branch: "build/login" },
      { kind: "branch", project_id: "p2", branch: "other" },
      { kind: "issue", project_id: "p1", branch: null },
    ];
    expect(branchOptions(items, "p1")).toEqual(["build/login", "main"]);
    expect(branchOptions(items, "p3")).toEqual([]);
  });
});

// The box is about one machine — the one creation goes to — so it says which,
// both in the line it asks with and in what it promises about a capture it
// cannot send yet. An account whose device this client cannot name yet keeps
// the plain words.
describe("what the box says about the machine it sends to", () => {
  it("asks on the named device, and asks plainly when there is no name", () => {
    expect(composePlaceholder("Laptop")).toBe("Capture on Laptop");
    expect(composePlaceholder(null)).toBe("What do you want to get done?");
    expect(composePlaceholder("")).toBe("What do you want to get done?");
  });

  // Shut and open, it is the same question: the shut box asks it on the rail and
  // the open box asks it again in the field. One sentence, said once.
  it("asks the shut box's question when it cannot name a device", () => {
    expect(composePromptHtml()).toContain(composePlaceholder(null));
  });

  it("names the device it is holding a capture for", () => {
    expect(composeOfflineNote(0, "Laptop")).toBe("Laptop is away — this is kept here and sent when it is back.");
    expect(composeOfflineNote(1, "Laptop")).toBe("1 capture is waiting for Laptop.");
    expect(composeOfflineNote(3, "Laptop")).toBe("3 captures are waiting for Laptop.");
  });

  it("names the same device when the manual panel will not create on it", () => {
    expect(composeManualAwayNote("Laptop")).toBe(
      "Laptop is away — capture it instead and it will be routed when it is back.",
    );
    expect(composeManualAwayNote(null)).toBe(
      "Your device is away — capture it instead and it will be routed when it is back.",
    );
  });

  it("says your device when this client cannot name one", () => {
    expect(composeOfflineNote(0, null)).toBe("Your device is away — this is kept here and sent when it is back.");
    expect(composeOfflineNote(2, null)).toBe("2 captures are waiting for your device.");
  });
});
