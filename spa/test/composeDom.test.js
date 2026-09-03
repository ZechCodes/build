// @vitest-environment jsdom
// The global compose box: pinned at the inbox rail's top, opened from anywhere
// by `c`, capture-first on submit, queued while the device is away — and the
// advanced panel for the times the destination is already known.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

let feedItems = [];
let feedProjects = [];
let subscriber = null;
const refreshFeed = vi.fn(async () => subscriber && subscriber({ items: feedItems, projects: feedProjects }));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    fn({ items: feedItems, projects: feedProjects });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  primaryRunIdFor: () => null,
}));

let App;
let initCompose;
let flushCaptures;
let pendingCaptureRows;
let adoptCaptureRecord;
let forgetCaptureRecord;
let subscribePendingCaptures;
let CAPTURE_QUEUE_KEY;

const flush = () => new Promise((done) => setTimeout(done, 0));
const $ = (selector) => document.querySelector(selector);
const type = (selector, value) => {
  const control = $(selector);
  control.value = value;
  control.dispatchEvent(new Event("input", { bubbles: true }));
  return control;
};
const press = (key, target = document.body) => {
  const event = new KeyboardEvent("keydown", { key, bubbles: true });
  Object.defineProperty(event, "target", { value: target });
  document.dispatchEvent(event);
};

const captureRecord = (over = {}) => ({
  id: "capture-1",
  text: "fix the login redirect",
  created_at: "2026-08-14T10:00:00Z",
  state: "routing",
  routing: null,
  question: null,
  ...over,
});

beforeEach(async () => {
  vi.resetModules();
  localStorage.clear();
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  feedItems = [{ kind: "branch", project_id: "p1", project: "relaydb", branch: "build/login", run_id: "run-1" }];
  feedProjects = [
    { id: "p1", name: "relaydb" },
    { id: "p2", name: "dotfiles" },
  ];
  refreshFeed.mockClear();
  ({ App } = await import("../src/app.js"));
  ({ initCompose, flushCaptures, pendingCaptureRows, adoptCaptureRecord, forgetCaptureRecord, subscribePendingCaptures } =
    await import("../src/core/composeView.js"));
  ({ CAPTURE_QUEUE_KEY } = await import("../src/core/compose.js"));
  App.route = { name: "inbox" };
  App.gated = false;
  App.offline = false;
  App.modelCatalog = {
    default_provider: "claude",
    providers: [
      { id: "claude", label: "Claude Code", models: [{ id: "opus", label: "Opus", supports_effort: true }], efforts: ["low"] },
    ],
  };
  App.call = vi.fn(async (method) => {
    if (method === "capture.create") return captureRecord();
    if (method === "capture.get") return captureRecord();
    if (method === "issue.create") return { project_id: "p1", issue_id: "iss-3" };
    if (method === "branch.dispatch") return { project_id: "p1", branch: "build/login", run_id: "run-1", agent_id: "agent-1" };
    return { ok: true };
  });
  initCompose();
});

describe("where compose lives", () => {
  it("is pinned at the inbox rail's top, above the entries", () => {
    const rail = $("#inbox-rail");
    const host = $("#compose");
    expect(rail.contains(host)).toBe(true);
    expect(host.compareDocumentPosition($("#inbox-list")) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect($("#compose-open").textContent).toContain("What do you want to get done?");
  });

  it("survives the gate, because a capture is worth keeping before a device answers", () => {
    document.body.classList.add("gated");
    expect($("#compose-open")).toBeTruthy();
  });
});

describe("the c shortcut", () => {
  it("opens the box and puts the caret in it", () => {
    press("c");
    expect($("#compose-text")).toBeTruthy();
    expect(document.activeElement).toBe($("#compose-text"));
  });

  it("stays out of the way while the letter is being typed somewhere", () => {
    const field = document.createElement("input");
    document.body.appendChild(field);
    press("c", field);
    expect($("#compose-text")).toBeNull();
  });

  it("stays shut behind a modal, which is a question already in flight", async () => {
    const { confirmAction } = await import("../src/core/confirm.js");
    const answered = confirmAction({ title: "Done?", intro: "", actions: [], confirmLabel: "Done" });
    press("c");
    expect($("#compose-text")).toBeNull();
    document.querySelector("[data-confirm-cancel]").click();
    await answered;
    press("c");
    expect($("#compose-text")).toBeTruthy();
  });

  it("closes on Escape, keeping nothing that was not sent", () => {
    press("c");
    type("#compose-text", "half a thought");
    press("Escape", $("#compose-text"));
    expect($("#compose-text")).toBeNull();
    expect($("#compose-open")).toBeTruthy();
  });
});

describe("capture first", () => {
  it("hands the text to the daemon and decides nothing about it", async () => {
    press("c");
    type("#compose-text", "fix the login redirect");
    $("#compose-send").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.create", { text: "fix the login redirect" });
    expect(refreshFeed).toHaveBeenCalled();
    expect($("#compose-text")).toBeNull(); // the box is done with it
  });

  it("refuses to send nothing", async () => {
    press("c");
    type("#compose-text", "   ");
    $("#compose-send").click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("capture.create", expect.anything());
    expect($(".compose-error").hidden).toBe(false);
  });

  it("shows the capture on the inbox before the daemon has decided anything", async () => {
    press("c");
    type("#compose-text", "fix the login redirect");
    $("#compose-send").click();
    await flush();
    const rows = pendingCaptureRows();
    expect(rows.map((row) => [row.capture_id, row.state])).toEqual([["capture-1", "routing"]]);
  });

  it("sends on ⌘/ctrl+Enter, since a capture can be more than one line", async () => {
    press("c");
    const text = type("#compose-text", "ship it");
    text.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.create", { text: "ship it" });
  });
});

// A capture the client sent is watched until its route settles, and for two
// minutes after — the window where the routing is still visible and reversible.
describe("a route the client is watching", () => {
  const routedTo = (over) => ({ ...captureRecord({ state: "routed" }), ...over });

  /** Send one, then let the feed drop it: the route settled, so the client asks
   *  once where it went and keeps the row on screen. */
  async function settledCapture(record) {
    App.call = vi.fn(async (method) => {
      if (method === "capture.create") return captureRecord();
      if (method === "capture.get") return record;
      return { ok: true };
    });
    press("c");
    type("#compose-text", "add a CSV export");
    $("#compose-send").click();
    await flush();
    feedItems = [];
    await refreshFeed();
    await flush();
  }

  it("says where a settled capture went", async () => {
    await settledCapture(routedTo({ routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const [row] = pendingCaptureRows();
    expect(row.routing.kind).toBe("issue");
    expect(row.project).toBe("relaydb");
  });

  it("takes the new destination the moment the user reroutes it", async () => {
    await settledCapture(routedTo({ routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));

    // What `capture.reroute` answers with. Nothing else will ever correct this
    // row: the feed stopped carrying the capture when its route settled.
    adoptCaptureRecord(routedTo({ routing: { project_id: "p2", kind: "branch", target_id: "build/csv-export" } }));

    const [row] = pendingCaptureRows();
    expect(row.routing.kind).toBe("branch");
    expect(row.project).toBe("dotfiles");
    expect(row.branch).toBe("build/csv-export");
    expect(row.issue_id).toBeNull();
  });

  it("gives a rerouted row its two minutes back, so the new route is undoable too", async () => {
    await settledCapture(routedTo({ routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const { ROUTED_LINGER_MS } = await import("../src/core/compose.js");
    const settledAt = Date.now();
    const nearlyGone = settledAt + ROUTED_LINGER_MS - 10;
    const wouldHaveGone = settledAt + ROUTED_LINGER_MS + 10;
    expect(pendingCaptureRows(wouldHaveGone).length).toBe(0);

    // The user reroutes it just before it would have dropped off.
    await settledCapture(routedTo({ routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const clock = vi.spyOn(Date, "now").mockReturnValue(nearlyGone);
    adoptCaptureRecord(routedTo({ routing: { project_id: "p2", kind: "branch", target_id: "build/csv-export" } }));
    clock.mockRestore();

    expect(pendingCaptureRows(wouldHaveGone).length).toBe(1);
    expect(pendingCaptureRows(nearlyGone + ROUTED_LINGER_MS + 10).length).toBe(0);
  });

  it("forgets a capture the client is holding once it is cancelled", async () => {
    await settledCapture(routedTo({ routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const repaints = vi.fn();
    subscribePendingCaptures(repaints);
    expect(pendingCaptureRows().map((row) => row.capture_id)).toEqual(["capture-1"]);

    forgetCaptureRecord("capture-1");

    expect(pendingCaptureRows()).toEqual([]);
    expect(repaints).toHaveBeenCalledTimes(1);
  });

  it("leaves a capture the feed still carries to the feed, which is its record", async () => {
    press("c");
    type("#compose-text", "add a CSV export");
    $("#compose-send").click();
    await flush();

    adoptCaptureRecord(routedTo({ routing: { project_id: "p2", kind: "branch", target_id: "build/csv-export" } }));

    expect(pendingCaptureRows(Date.now() + 10).length).toBe(1);
  });
});

describe("while the device is away", () => {
  beforeEach(() => {
    App.offline = true;
  });

  it("keeps what was said on the device, and says so", async () => {
    press("c");
    type("#compose-text", "remember the redirect");
    $("#compose-send").click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("capture.create", expect.anything());
    const queued = JSON.parse(localStorage.getItem(CAPTURE_QUEUE_KEY));
    expect(queued.map((entry) => entry.text)).toEqual(["remember the redirect"]);
    expect(pendingCaptureRows().map((row) => row.state)).toEqual(["queued"]);
  });

  it("sends what it was holding the moment the device is back", async () => {
    press("c");
    type("#compose-text", "remember the redirect");
    $("#compose-send").click();
    await flush();

    App.offline = false;
    await flushCaptures();
    expect(App.call).toHaveBeenCalledWith("capture.create", { text: "remember the redirect" });
    expect(JSON.parse(localStorage.getItem(CAPTURE_QUEUE_KEY))).toEqual([]);
  });

  it("keeps holding a capture the device still will not take", async () => {
    press("c");
    type("#compose-text", "remember the redirect");
    $("#compose-send").click();
    await flush();

    App.offline = false;
    App.call = vi.fn(async () => {
      throw new Error("device offline");
    });
    await flushCaptures();
    expect(JSON.parse(localStorage.getItem(CAPTURE_QUEUE_KEY)).length).toBe(1);
    expect(pendingCaptureRows().map((row) => row.state)).toEqual(["queued"]);
  });
});

describe("the advanced panel", () => {
  const openAdvanced = () => {
    press("c");
    $("#compose-advanced").click();
  };

  it("offers the projects, the two things work can be, and the branches there are", () => {
    openAdvanced();
    expect([...document.querySelectorAll("#compose-project option")].map((option) => option.value)).toEqual(["p1", "p2"]);
    expect($('[data-compose-kind="issue"]')).toBeTruthy();
    expect($('[data-compose-kind="branch"]')).toBeTruthy();
    $('[data-compose-kind="branch"]').click();
    expect([...document.querySelectorAll("#compose-branches option")].map((option) => option.value)).toEqual(["build/login"]);
  });

  it("carries the harness picker — agent, model and effort — on the same panel", () => {
    openAdvanced();
    $("[data-agent-choice-toggle]").click();
    // Both agents, whatever the bridge has listed models for: dispatching needs
    // only a harness. The Claude Code entry is the carrier this account's
    // default names, so the carrier question is never put to the writer.
    const providers = [...document.querySelectorAll("#compose-choice-provider option")];
    expect(providers.map((option) => option.value)).toEqual(["claude", "codex"]);
    expect(providers.map((option) => option.textContent)).toEqual(["Claude Code", "Codex"]);
    expect($("#compose-choice-model")).toBeTruthy();
    expect($("#compose-choice-effort")).toBeTruthy();
  });

  it("files an inert issue and opens it, without troubling the router", async () => {
    openAdvanced();
    type("#compose-text", "add a /health endpoint");
    $("#compose-project").value = "p2";
    $("#compose-project").dispatchEvent(new Event("change", { bubbles: true }));
    $("#compose-manual-go").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("issue.create", { goal: "add a /health endpoint", project_id: "p2", dispatch: false });
    expect(App.call).not.toHaveBeenCalledWith("capture.create", expect.anything());
    expect(location.hash).toBe("#/project/p1/issue/iss-3");
  });

  it("dispatches a branch, with the harness the panel names", async () => {
    openAdvanced();
    type("#compose-text", "finish the redirect");
    $('[data-compose-kind="branch"]').click();
    type("#compose-branch", "build/login");
    $("[data-agent-choice-toggle]").click();
    $("#compose-choice-model").value = "opus";
    $("#compose-choice-model").dispatchEvent(new Event("change", { bubbles: true }));
    $("#compose-manual-go").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("branch.dispatch", {
      project_id: "p1",
      instruction: "finish the redirect",
      branch: "build/login",
      provider: "claude",
      model: "opus",
    });
    expect(location.hash).toBe("#/project/p1/branch/build%2Flogin/changes");
  });

  it("says what the daemon refused, and keeps what was typed", async () => {
    App.call = vi.fn(async () => {
      throw new Error("unknown project_id: p9");
    });
    openAdvanced();
    type("#compose-text", "add a /health endpoint");
    $("#compose-manual-go").click();
    await flush();
    expect($(".compose-error").textContent).toContain("unknown project_id");
    expect($("#compose-text").value).toBe("add a /health endpoint");
  });
});
