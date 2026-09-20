// @vitest-environment jsdom
// The capture decision page's wiring: the router's choices as one tap each, a
// destination named by hand, words typed in answer, and the way out — plus the
// rule every poll-driven surface here lives by, that a tick nobody asked for
// never takes the box being typed into.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

/** The one bridge this file's device answers through: a test that hands over
 *  a new `call` is that bridge answering differently, not another machine. */
const bridge = { call: null };

const feedProjects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "dotfiles" },
];
const feedItems = [{ kind: "branch", project_id: "p1", branch: "build/login" }];
/** The merge as the feed last delivered it — one device's worth by default, the
 *  way a single-device fixture reads. */
let feedSnapshot = { items: feedItems, projects: feedProjects };
const refreshFeed = vi.fn(async () => []);
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    fn(feedSnapshot);
    return () => {};
  },
  refreshFeed: (...args) => refreshFeed(...args),
  startFeed: () => {},
  stopFeed: () => {},
  dropFeedDevice: () => {},
}));

/** Another device that is connected too: nothing this page does may reach it. */
const awayCall = vi.fn(async () => ({}));

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({
  notifyError: (...args) => notifyError(...args),
  notifySuccess: () => {},
}));

let App;
let mountCaptureDecision;
let resetDeviceContexts;
/** What the home device answers. A capture is that device's (design §8), so
 *  every call this page makes has to land here and nowhere else. */
let homeCall;
let pendingIn;
let entryKeyOf;
let host;
let surface;

const flush = () => new Promise((done) => setTimeout(done, 0));

// Every destination the router can offer is a branch: filing an issue is
// retired (core/captureDecision.js drops an issue option that names no branch),
// so a fixture offering one would be asserting about a choice the page never
// paints.
const option = (over = {}) => ({
  id: "option-1",
  label: "Continue the login work",
  project_id: "p1",
  kind: "branch",
  branch: "build/login",
  ...over,
});

const capture = (over = {}) => ({
  id: "capture-1",
  text: "fix the login redirect",
  created_at: "2026-08-15T10:00:00Z",
  state: "unrouted",
  routing: null,
  question: null,
  ...over,
});

const asking = ({ question: asked, ...over } = {}) =>
  capture({
    ...over,
    question: {
      text: "Which project is the login redirect in?",
      asked_at: "2026-08-15T10:00:01Z",
      answer: null,
      options: [option(), option({ id: "option-2", label: "Start the CSV export", branch: "build/csv-export" })],
      chosen_option_id: null,
      ...(asked || {}),
    },
  });

/** What `capture.get` answers with next. */
let record = asking();

/** Answer the confirmation modal a destructive verb opens. */
async function answerConfirm(ok) {
  await flush();
  const scrim = document.getElementById("confirm-scrim");
  expect(scrim, "a confirmation was expected").toBeTruthy();
  scrim.querySelector(ok ? "[data-confirm-ok]" : "[data-confirm-cancel]").click();
  await flush();
}

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = '<main id="root"><div id="capture-page"></div></main>';
  location.hash = "#/capture/capture-1";
  ({ App } = await import("../src/app.js"));
  ({ mountCaptureDecision } = await import("../src/core/captureDecisionView.js"));
  ({ pendingIn } = await import("../src/core/optimistic.js"));
  ({ entryKeyOf } = await import("../src/core/inbox.js"));
  App.route = { name: "capture", id: "capture-1" };
  App.gated = false;
  record = asking();
  feedSnapshot = { items: feedItems, projects: feedProjects };
  refreshFeed.mockClear();
  notifyError.mockClear();
  awayCall.mockClear();
  homeCall = vi.fn(async (method) => {
    if (method === "capture.get") return record;
    return record;
  });
  // Home the way the running app names it: the account lists the device online
  // and the user's pick names it. The context's caller is read fresh on every
  // call, so a case can rescript `homeCall` after the page is mounted.
  const contexts = await import("../src/core/deviceContexts.js");
  resetDeviceContexts = contexts.resetDeviceContexts;
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  App.selectedDeviceId = "dev-1";
  contexts.adoptDeviceSession({
    deviceId: "dev-1",
    call: (...args) => homeCall(...args),
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });
  contexts.adoptDeviceSession({
    deviceId: "dev-2",
    call: awayCall,
    close: () => {},
    peer: () => {},
    onCarrier: () => {},
  });
  // The alias no surface on this page may read any more.
  bridge.call = vi.fn(async () => record);
  host = document.getElementById("capture-page");
  surface = mountCaptureDecision(host, "capture-1");
  await surface.load();
});

afterEach(() => {
  surface?.dispose();
  document.getElementById("confirm-scrim")?.remove();
  resetDeviceContexts();
});

const choices = () => [...host.querySelectorAll("[data-capture-option]")];

describe("the capture decision page", () => {
  it("states what was said, what the router asked, and the choices it offered", () => {
    expect(homeCall).toHaveBeenCalledWith("capture.get", { capture_id: "capture-1" });
    expect(host.textContent).toContain("fix the login redirect");
    expect(host.textContent).toContain("Which project is the login redirect in?");
    expect(choices().map((choice) => choice.dataset.captureOption)).toEqual(["option-1", "option-2"]);
    expect(choices()[0].textContent).toContain("relaydb · branch build/login");
    expect(host.textContent).toContain("Waiting for your answer");
  });

  it("answers with the choice that was tapped, and shows the router deciding again", async () => {
    record = capture({ state: "routing", question: { ...asking().question, answer: "…", chosen_option_id: "option-1" } });
    choices()[0].click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("capture.answer", { capture_id: "capture-1", option_id: "option-1" });
    expect(refreshFeed).toHaveBeenCalled();
    expect(host.textContent).toContain("Deciding where this goes");
  });

  it("goes back to the inbox once the router has routed it", async () => {
    record = capture({ state: "routed", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } });
    await surface.load();
    expect(location.hash).toBe("#/inbox");
  });

  it("answers in the router's own terms when the destination is named by hand", async () => {
    host.querySelector("[data-capture-kind='branch']").click();
    const project = host.querySelector("#capture-project");
    project.value = "p2";
    project.dispatchEvent(new Event("change"));
    host.querySelector("#capture-branch").value = "build/csv-export";
    host.querySelector("#capture-route").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("capture.answer", {
      capture_id: "capture-1",
      text: "Route this to project p2 as a branch, on the branch build/csv-export",
    });
  });

  it("offers the branches the chosen project already has", () => {
    host.querySelector("[data-capture-kind='branch']").click();
    expect([...host.querySelectorAll("#capture-branches option")].map((o) => o.value)).toEqual(["build/login"]);
  });

  it("routes by hand when there is no question to answer", async () => {
    record = capture({ state: "failed" });
    await surface.load();
    expect(host.querySelector("#capture-answer-send").disabled).toBe(true);
    host.querySelector("#capture-route").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("capture.reroute", {
      capture_id: "capture-1",
      project_id: "p1",
      kind: "branch",
    });
  });

  it("takes an answer in the user's own words", async () => {
    const box = host.querySelector("#capture-answer");
    box.value = "neither, it is the relay";
    box.dispatchEvent(new Event("input"));
    host.querySelector("#capture-answer-send").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("capture.answer", {
      capture_id: "capture-1",
      text: "neither, it is the relay",
    });
  });

  it("says nothing to the daemon when the answer box is empty", async () => {
    host.querySelector("#capture-answer-send").click();
    await flush();
    expect(homeCall).not.toHaveBeenCalledWith("capture.answer", expect.anything());
  });

  it("cancels the capture behind a confirmation, and leaves for the inbox", async () => {
    host.querySelector("#capture-cancel").click();
    await answerConfirm(true);
    expect(homeCall).toHaveBeenCalledWith("capture.cancel", { capture_id: "capture-1" });
    expect(refreshFeed).toHaveBeenCalled();
    expect(location.hash).toBe("#/inbox");
  });

  it("keeps the capture when the confirmation is declined", async () => {
    host.querySelector("#capture-cancel").click();
    await answerConfirm(false);
    expect(homeCall).not.toHaveBeenCalledWith("capture.cancel", expect.anything());
    expect(location.hash).toBe("#/capture/capture-1");
    expect(pendingIn("inbox")).toEqual([]);
  });

  it("leaves for the inbox the instant the cancel is confirmed, before capture.cancel answers", async () => {
    homeCall = vi.fn(async (method) => {
      if (method === "capture.cancel") return new Promise(() => {});
      return record;
    });
    host.querySelector("#capture-cancel").click();
    await answerConfirm(true);

    expect(location.hash).toBe("#/inbox");
    expect(homeCall).toHaveBeenCalledWith("capture.cancel", { capture_id: "capture-1" });
  });

  it("takes the capture's inbox row off under the key the inbox itself speaks", async () => {
    homeCall = vi.fn(async (method) => {
      if (method === "capture.cancel") return new Promise(() => {});
      return record;
    });
    host.querySelector("#capture-cancel").click();
    await answerConfirm(true);

    const held = pendingIn("inbox");
    expect(held).toHaveLength(1);
    expect(held[0].kind).toBe("remove");
    expect(held[0].key).toBe(entryKeyOf({ kind: "capture", capture_id: "capture-1" }));
  });

  it("puts the row back and says why when the cancel is refused", async () => {
    homeCall = vi.fn(async (method) => {
      if (method === "capture.cancel") throw new Error("that capture is already routed");
      return record;
    });
    host.querySelector("#capture-cancel").click();
    await answerConfirm(true);
    await flush();

    expect(pendingIn("inbox")).toEqual([]);
    expect(notifyError).toHaveBeenCalledTimes(1);
    expect(notifyError).toHaveBeenCalledWith("The capture could not be cancelled", "that capture is already routed");
    expect(location.hash).toBe("#/inbox");
  });

  it("answers, reroutes, cancels and reads the capture on the home device's call", async () => {
    // A capture was taken on the home device and is routed into that device's
    // projects, so all four of this page's calls go there — never through the
    // App alias, and never to the other device that happens to be connected.
    choices()[1].click();
    await flush();
    record = capture({ state: "failed" });
    await surface.load();
    host.querySelector("#capture-route").click();
    await flush();
    host.querySelector("#capture-cancel").click();
    await answerConfirm(true);

    const asked = homeCall.mock.calls.map(([method]) => method);
    expect(asked).toContain("capture.get");
    expect(asked).toContain("capture.answer");
    expect(asked).toContain("capture.reroute");
    expect(asked).toContain("capture.cancel");
    expect(bridge.call).not.toHaveBeenCalled();
    expect(awayCall).not.toHaveBeenCalled();
  });

  it("says on the page, and out loud, when the daemon refuses an answer", async () => {
    homeCall = vi.fn(async (method) => {
      if (method === "capture.answer") throw new Error("that question has already been answered");
      return record;
    });
    choices()[0].click();
    await flush();
    const error = host.querySelector(".capture-decide-error");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toContain("already been answered");
    expect(notifyError).toHaveBeenCalled();
  });
});

// A capture belongs to the machine it was taken on, and home moves. The merged
// inbox carries every device's captures and names the device each row came from,
// so the page reads, answers and offers destinations there — not on whichever
// machine creation goes to now.
describe("a capture taken on a device that is not home", () => {
  beforeEach(async () => {
    surface.dispose();
    feedSnapshot = {
      items: [{ kind: "capture", capture_id: "capture-1", deviceId: "dev-2" }],
      projects: [],
      devices: {
        "dev-1": { items: feedItems, projects: feedProjects },
        "dev-2": { items: [], projects: [{ id: "p9", name: "away notes" }] },
      },
    };
    awayCall.mockImplementation(async () => record);
    homeCall.mockClear();
    host.innerHTML = "";
    surface = mountCaptureDecision(host, "capture-1");
    await surface.load();
  });

  it("reads and answers the capture on the machine it is on", async () => {
    expect(awayCall).toHaveBeenCalledWith("capture.get", { capture_id: "capture-1" });

    choices()[1].click();
    await flush();

    expect(awayCall).toHaveBeenCalledWith("capture.answer", { capture_id: "capture-1", option_id: "option-2" });
    expect(homeCall).not.toHaveBeenCalled();
    expect(bridge.call).not.toHaveBeenCalled();
  });

  it("offers that machine's projects as the destinations", () => {
    const options = [...host.querySelectorAll("#capture-project option")].map((option) => option.textContent);
    expect(options).toEqual(["away notes"]);
  });
});

describe("while the page is being used", () => {
  it("paints nothing at all when the poll reads the same capture again", async () => {
    const before = host.querySelector("#capture-answer");
    await surface.load();
    expect(host.querySelector("#capture-answer")).toBe(before);
  });

  it("stands down while an answer is being typed, and catches up once the caret leaves", async () => {
    const box = host.querySelector("#capture-answer");
    box.focus();
    box.value = "it is the rel";
    box.dispatchEvent(new Event("input"));
    box.setSelectionRange(5, 5);

    record = asking({ question: { text: "Which project, really?" } });
    await surface.load();

    const now = host.querySelector("#capture-answer");
    expect(now, "the answer box was replaced by the poll").toBe(box);
    expect(now.value).toBe("it is the rel");
    expect(document.activeElement).toBe(now);
    expect(now.selectionStart).toBe(5);

    now.blur();
    await surface.load();
    expect(host.textContent).toContain("Which project, really?");
  });

  it("carries what was typed onto the page a repaint does rebuild", () => {
    const box = host.querySelector("#capture-answer");
    box.value = "it is the relay";
    box.dispatchEvent(new Event("input"));
    // Something the user did elsewhere on the page rebuilds it.
    host.querySelector("[data-capture-kind='branch']").click();
    expect(host.querySelector("#capture-answer").value).toBe("it is the relay");
  });
});
